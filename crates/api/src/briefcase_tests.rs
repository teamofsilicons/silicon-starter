use super::*;
use crate::{
    authority::ENDPOINTS,
    durable::FeatureStore,
    iam::{Iam, World},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path, path_regex},
};

const ACTOR: &str = "c:author";
const ORG: &str = "tos";
const BUNDLE: &[u8] = b"immutable git bundle prepared before permission";
struct Harness {
    dir: tempfile::TempDir,
    iam_server: MockServer,
    provider_server: MockServer,
    iam: Iam,
    store: FeatureStore,
    session: IamTokens,
    app: AppState,
    upload: Arc<Mutex<Option<Value>>>,
    manifest: Arc<Mutex<Option<Value>>>,
    lose_commit: Arc<AtomicBool>,
    auth_id: Uuid,
}
fn pair(endpoint: &str) -> Value {
    json!({"grant_id":operation(&json!(["grant",endpoint])),"access_token":format!("oba_initial_{}",endpoint.replace('.',"_")),"refresh_token":format!("obr_initial_{}",endpoint.replace('.',"_")),"token_type":"Bearer","expires_in":1800,"expires_at":"2090-01-01T00:00:00Z","audience":"briefcase","endpoint_id":endpoint,"org_id":ORG,"actor":{"type":"carbon","public_id":ACTOR},"scope":format!("obo:briefcase:{endpoint}")})
}
fn detail(id: Uuid, status: &str) -> Value {
    json!({"id":id,"app_id":"starter","app_name":"Starter","actor":{"type":"carbon","public_id":ACTOR},"org_id":ORG,"status":status,"version":1,"expires_at":"2090-01-01T00:00:00Z","endpoints":[],"authorization_url":format!("https://auth.iam.teamofsilicons.com/review/{id}")})
}
fn upstream(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error":{"code":code,"message":"fixture error"}}))
}
fn input() -> PublishRequest {
    PublishRequest {
        selector: "latest".into(),
        version: "1.0".into(),
        commit: None,
        notes: "Original release notes".into(),
    }
}
impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let iam_server = MockServer::start().await;
        let provider_server = MockServer::start().await;
        let iam = Iam::connect(&iam_server.uri(), "starter", "ask_fixture", None)
            .await
            .unwrap();
        let store = FeatureStore::open(&dir.path().join("features.sqlite"), &[9; 32]).unwrap();
        let session = IamTokens {
            access_token: "oat_original_session".into(),
            refresh_token: "ort_ordinary_refresh".into(),
            expires_in: 1800,
            expires_at: Some(4_000_000_000),
            actor: Some(json!({"type":"carbon","public_id":ACTOR})),
            org_id: Some(ORG.into()),
            org_ids: vec![ORG.into()],
            context_id: Uuid::new_v4().to_string(),
            world: World::default(),
        };
        let app = AppState::default();
        app.bundles
            .write()
            .await
            .insert("tos.example".into(), BUNDLE.to_vec());
        app.bundle_commits
            .write()
            .await
            .insert("tos.example".into(), "0123456789abcdef".into());
        let auth_id = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/api/v1/obo-access/authorizations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail(auth_id, "pending")))
            .mount(&iam_server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/obo-access/authorizations/{auth_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail(auth_id, "approved")))
            .mount(&iam_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/obo-access/tokens"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let items = if let Some(refresh) = body["refresh_token"].as_str() {
                    let endpoint = ENDPOINTS
                        .iter()
                        .find(|e| format!("obr_initial_{}", e.replace('.', "_")) == refresh)
                        .unwrap();
                    let mut p = pair(endpoint);
                    p["access_token"] = json!("oba_rotated_provider");
                    vec![p]
                } else {
                    ENDPOINTS.iter().map(|e| pair(e)).collect()
                };
                ResponseTemplate::new(200).set_body_json(json!({"items":items}))
            })
            .mount(&iam_server)
            .await;
        let mut operations: Vec<Value> = [
            "/obo/folders/create",
            "/obo/uploads/reserve",
            "/obo/uploads/status",
            "/obo/uploads/commit",
            "/obo/link-access",
        ]
        .map(|p| json!({"path":p,"method":"POST","version":"3.0.0"}))
        .into();
        operations.push(
            json!({"path":"/obo/uploads/{upload_id}/content","method":"PUT","version":"2.0.0"}),
        );
        Mock::given(method("GET")).and(path("/api/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"service":"silicon-briefcase","selected_api_version":"v1","operations":operations}))).mount(&provider_server).await;
        Mock::given(method("POST")).and(path("/api/v1/obo/folders/create")).respond_with(|request:&wiremock::Request|{let b:Value=serde_json::from_slice(&request.body).unwrap();ResponseTemplate::new(200).set_body_json(json!({"id":b["operation_id"],"path":format!("{}/{}",b["parent_path"].as_str().unwrap(),b["name"].as_str().unwrap())}))}).mount(&provider_server).await;
        let upload = Arc::new(Mutex::new(None::<Value>));
        let manifest = Arc::new(Mutex::new(None::<Value>));
        let lose_commit = Arc::new(AtomicBool::new(false));
        let u = upload.clone();
        Mock::given(method("POST"))
            .and(path("/api/v1/obo/uploads/status"))
            .respond_with(
                move |_: &wiremock::Request| match u.lock().unwrap().clone() {
                    Some(v) => ResponseTemplate::new(200).set_body_json(v),
                    None => upstream(404, "not_found"),
                },
            )
            .mount(&provider_server)
            .await;
        let u = upload.clone();
        let m = manifest.clone();
        Mock::given(method("POST")).and(path("/api/v1/obo/uploads/reserve")).respond_with(move|r:&wiremock::Request|{
            let b:Value=serde_json::from_slice(&r.body).unwrap();let mut manifest=m.lock().unwrap();if let Some(old)=manifest.as_ref(){assert_eq!(old,&b);}else{*manifest=Some(b.clone());}
            let mut u=u.lock().unwrap();let v=u.get_or_insert_with(||json!({"operation_id":b["operation_id"],"upload_id":Uuid::new_v4(),"state":"reserved","expires_at":"2090-01-01T00:00:00Z","published_entry_id":null}));let mut response=v.clone();response["capability"]=json!("upload_fixture_capability");ResponseTemplate::new(200).set_body_json(response)
        }).mount(&provider_server).await;
        let u = upload.clone();
        Mock::given(method("PUT"))
            .and(path_regex("^/api/v1/obo/uploads/[^/]+/content$"))
            .respond_with(move |r: &wiremock::Request| {
                assert_eq!(&r.body, BUNDLE);
                let mut u = u.lock().unwrap();
                let v = u.as_mut().unwrap();
                v["state"] = json!("staged");
                ResponseTemplate::new(200).set_body_json(v.clone())
            })
            .mount(&provider_server)
            .await;
        let u = upload.clone();
        let lost = lose_commit.clone();
        Mock::given(method("POST"))
            .and(path("/api/v1/obo/uploads/commit"))
            .respond_with(move |r: &wiremock::Request| {
                let b: Value = serde_json::from_slice(&r.body).unwrap();
                let mut u = u.lock().unwrap();
                let v = u.as_mut().unwrap();
                assert_eq!(b["operation_id"], v["operation_id"]);
                assert_eq!(b["upload_id"], v["upload_id"]);
                v["state"] = json!("committed");
                v["published_entry_id"] = json!("00000000-0000-4000-8000-000000000010");
                if lost.swap(false, Ordering::SeqCst) {
                    upstream(503, "lost_response")
                } else {
                    ResponseTemplate::new(200).set_body_json(v.clone())
                }
            })
            .mount(&provider_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/obo/link-access"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"enabled":true,"effective":true,"can_manage":true})),
            )
            .mount(&provider_server)
            .await;
        Self {
            dir,
            iam_server,
            provider_server,
            iam,
            store,
            session,
            app,
            upload,
            manifest,
            lose_commit,
            auth_id,
        }
    }
    fn feature(&self) -> Feature {
        Feature::open(self.iam.clone(), &self.store, &self.session).unwrap()
    }
    async fn approve(&self, key: &str) -> Value {
        let f = self.feature();
        let started = f.start(&self.session, key).await.unwrap();
        let id = Uuid::parse_str(started["request_id"].as_str().unwrap()).unwrap();
        f.complete(id, "manual-code").await.unwrap()
    }
    async fn publish(&self, key: &str) -> Result<Version> {
        let f = self.feature();
        publish_with_feature(
            &self.app,
            &f,
            "tos.example",
            &input(),
            key,
            &Provider::new(&self.provider_server.uri())?,
        )
        .await
    }
    async fn calls(&self, path: &str) -> Vec<wiremock::Request> {
        self.provider_server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == path)
            .collect()
    }
    async fn token_calls(&self) -> Vec<wiremock::Request> {
        self.iam_server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.url.path() == "/api/v1/obo-access/tokens")
            .collect()
    }
}

#[tokio::test]
async fn permission_preserves_prepared_publication_and_capability_transfer_has_no_parent_authority()
{
    let h = Harness::new().await;
    assert_eq!(
        h.publish("prepared-publication-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(h.app.versions.read().await.is_empty());
    assert!(h.calls("/api/v1/obo/folders/create").await.is_empty());
    h.app.bundles.write().await.insert(
        "tos.example".into(),
        b"a later push must not retarget the pending publication".to_vec(),
    );
    let approval = h.approve("first-permission-key").await;
    assert_eq!(approval["completed"], true);
    assert_eq!(approval["roots"].as_array().unwrap().len(), 5);
    let published = h.publish("prepared-publication-key").await.unwrap();
    assert_eq!(published.commit, "0123456789abcdef");
    assert_eq!(h.app.versions.read().await["tos.example"].len(), 1);
    let replay = h.publish("prepared-publication-key").await.unwrap();
    assert_eq!(replay.published_at, published.published_at);
    assert_eq!(h.calls("/api/v1/obo/uploads/commit").await.len(), 1);
    assert_eq!(h.calls("/api/v1/obo/link-access").await.len(), 1);
    let transfer = h
        .provider_server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(
        transfer.headers["x-briefcase-upload-capability"],
        "upload_fixture_capability"
    );
    for secret_header in [
        "authorization",
        "x-app-id",
        "x-iam-obo-access-token",
        "x-iam-obo-access-proof",
    ] {
        assert!(!transfer.headers.contains_key(secret_header));
    }
    for request in h.calls("/api/v1/obo/folders/create").await {
        assert!(
            request.headers["x-iam-obo-access-token"]
                .to_str()
                .unwrap()
                .starts_with("oba_")
        );
        assert!(!request.headers.contains_key("authorization"));
    }
    assert_eq!(
        h.manifest.lock().unwrap().as_ref().unwrap()["sha256"],
        digest(BUNDLE)
    );
    assert_eq!(
        h.upload.lock().unwrap().as_ref().unwrap()["state"],
        "committed"
    );
    for file in ["features.sqlite", "features.sqlite-wal"] {
        let bytes = std::fs::read(h.dir.path().join(file)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for secret in [
            "oat_original_session",
            "oba_initial_",
            "obr_initial_",
            "manual-code",
            "immutable git bundle",
        ] {
            assert!(!text.contains(secret), "plaintext {secret} in {file}");
        }
    }
}

#[tokio::test]
async fn lost_commit_reply_reconciles_after_restart_without_duplicate_transfer_or_publication() {
    let mut h = Harness::new().await;
    h.approve("approve-lost-commit-key").await;
    h.lose_commit.store(true, Ordering::SeqCst);
    assert_eq!(
        h.publish("lost-commit-publish-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(h.app.versions.read().await.is_empty());
    h.store = FeatureStore::open(&h.dir.path().join("features.sqlite"), &[9; 32]).unwrap();
    assert_eq!(
        h.publish("lost-commit-publish-key").await.unwrap().version,
        "1.0"
    );
    assert_eq!(h.calls("/api/v1/obo/uploads/commit").await.len(), 1);
    assert_eq!(h.calls("/api/v1/obo/uploads/reserve").await.len(), 1);
    assert_eq!(h.calls("/api/v1/obo/uploads/status").await.len(), 2);
}

#[tokio::test]
async fn reapproval_cannot_move_an_existing_publication_to_another_provider_account() {
    let h = Harness::new().await;
    h.approve("approve-original-destination-key").await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/folders/create"))
        .respond_with(upstream(503, "uncertain"))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&h.provider_server)
        .await;
    assert_eq!(
        h.publish("pinned-destination-publish-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Mock::given(method("POST")).and(path("/api/v1/obo-access/tokens")).and(body_string_contains("authorization_code")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":ENDPOINTS.map(|endpoint|{let mut p=pair(endpoint);p["actor"]=json!({"type":"carbon","public_id":"c:other-owner"});p["org_id"]=json!("other");p})}))).with_priority(1).mount(&h.iam_server).await;
    h.approve("approve-new-destination-key").await;
    assert_eq!(
        h.publish("pinned-destination-publish-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(h.calls("/api/v1/obo/folders/create").await.len(), 1);
    assert!(h.app.versions.read().await.is_empty());
}

#[tokio::test]
async fn manual_code_recovery_uses_original_key_and_request_after_iam_marks_exchanged() {
    let h = Harness::new().await;
    let f = h.feature();
    let start = f.start(&h.session, "lost-manual-start-key").await.unwrap();
    let id = Uuid::parse_str(start["request_id"].as_str().unwrap()).unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .respond_with(upstream(503, "uncertain"))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&h.iam_server)
        .await;
    assert_eq!(
        f.complete(id, "same-manual-code").await.unwrap_err().status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/obo-access/authorizations/{}",
            h.auth_id
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail(h.auth_id, "exchanged")))
        .with_priority(1)
        .mount(&h.iam_server)
        .await;
    assert_eq!(
        f.complete(id, "same-manual-code").await.unwrap()["completed"],
        true
    );
    assert_eq!(
        f.complete(id, "different-manual-code")
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    let calls = h.token_calls().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].body, calls[1].body);
    assert_eq!(
        calls[0].headers["idempotency-key"],
        calls[1].headers["idempotency-key"]
    );
}

#[tokio::test]
async fn decline_and_graph_changes_leave_prepared_publication_and_ordinary_identity_intact() {
    for (state, status) in [("declined", 403), ("approved", 412)] {
        let h = Harness::new().await;
        let f = h.feature();
        let start = f
            .start(&h.session, "decline-graph-start-key")
            .await
            .unwrap();
        let id = Uuid::parse_str(start["request_id"].as_str().unwrap()).unwrap();
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/obo-access/authorizations/{}",
                h.auth_id
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail(h.auth_id, state)))
            .with_priority(1)
            .mount(&h.iam_server)
            .await;
        if status == 412 {
            Mock::given(method("POST"))
                .and(path("/api/v1/obo-access/tokens"))
                .respond_with(upstream(412, "obo_graph_changed"))
                .with_priority(1)
                .mount(&h.iam_server)
                .await;
        }
        assert_eq!(
            f.complete(id, "unused-declined-code")
                .await
                .unwrap_err()
                .status
                .as_u16(),
            status
        );
        assert!(
            f.lease
                .get::<Value>("root", ENDPOINTS[0])
                .unwrap()
                .is_none()
        );
        assert_eq!(h.session.actor.as_ref().unwrap()["public_id"], ACTOR);
    }
}

#[tokio::test]
async fn owner_context_and_world_isolation_hold_across_independent_store_connections() {
    let h = Harness::new().await;
    let f = h.feature();
    let start = f
        .start(&h.session, "owned-consent-request-key")
        .await
        .unwrap();
    let id = Uuid::parse_str(start["request_id"].as_str().unwrap()).unwrap();
    let other_store = FeatureStore::open(&h.dir.path().join("features.sqlite"), &[9; 32]).unwrap();
    assert!(Feature::open(h.iam.clone(), &other_store, &h.session).is_err());
    drop(f);
    let mut different = h.session.clone();
    different.context_id = Uuid::new_v4().to_string();
    let f = Feature::open(h.iam.clone(), &other_store, &different).unwrap();
    assert_eq!(
        f.status(id).await.unwrap_err().status,
        StatusCode::NOT_FOUND
    );
    drop(f);
    different.actor = Some(json!({"type":"carbon","public_id":"c:another"}));
    let f = Feature::open(h.iam.clone(), &other_store, &different).unwrap();
    assert_eq!(
        f.status(id).await.unwrap_err().status,
        StatusCode::NOT_FOUND
    );
    drop(f);
    different = h.session.clone();
    different.world.key_generation = 99;
    assert!(Feature::open(h.iam.clone(), &other_store, &different).is_err());
}

#[tokio::test]
async fn uncertain_refresh_is_durable_and_retries_only_the_same_family() {
    let mut h = Harness::new().await;
    Mock::given(method("POST")).and(path("/api/v1/obo-access/tokens")).and(body_string_contains("authorization_code")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":ENDPOINTS.map(|endpoint|{let mut p=pair(endpoint);if endpoint==ENDPOINTS[0]{p["expires_at"]=json!((chrono::Utc::now()+chrono::Duration::seconds(30)).to_rfc3339());}p})}))).with_priority(1).mount(&h.iam_server).await;
    h.approve("refresh-recovery-start-key").await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .and(body_string_contains("refresh_token"))
        .respond_with(upstream(503, "lost_refresh_reply"))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&h.iam_server)
        .await;
    assert_eq!(
        h.publish("refresh-recovery-publish-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    h.store = FeatureStore::open(&h.dir.path().join("features.sqlite"), &[9; 32]).unwrap();
    h.publish("refresh-recovery-publish-key").await.unwrap();
    let refresh: Vec<_> = h
        .token_calls()
        .await
        .into_iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("refresh_token"))
        .collect();
    assert_eq!(refresh.len(), 2);
    assert_eq!(refresh[0].body, refresh[1].body);
    assert_eq!(
        refresh[0].headers["idempotency-key"],
        refresh[1].headers["idempotency-key"]
    );
}

#[tokio::test]
async fn incompatible_receiver_gets_no_authority_or_bytes() {
    let h = Harness::new().await;
    h.approve("receiver-contract-start-key").await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"service":"silicon-briefcase","selected_api_version":"v1","operations":[]}),
        ))
        .with_priority(1)
        .mount(&h.provider_server)
        .await;
    assert_eq!(
        h.publish("incompatible-publish-key")
            .await
            .unwrap_err()
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for r in h.provider_server.received_requests().await.unwrap() {
        assert_eq!(r.url.path(), "/api/version");
        assert!(!r.headers.contains_key("x-iam-obo-access-token"));
    }
    assert!(h.app.versions.read().await.is_empty());
}

fn block(visibility: silicon_starter_core::Visibility) -> silicon_starter_core::blocks::Block {
    silicon_starter_core::blocks::Block {
        id: "gene:creativity".into(),
        kind: silicon_starter_core::blocks::BlockKind::Gene,
        name: "Creativity".into(),
        description: "Original block description".into(),
        owner: ORG.into(),
        visibility,
        version: digest(BUNDLE),
        downloads: 0,
        updated_at: chrono::Utc::now(),
    }
}
#[tokio::test]
async fn private_block_reconciles_lost_commit_after_restart_without_public_link() {
    let mut h = Harness::new().await;
    let original = block(silicon_starter_core::Visibility::Private);
    let provider = Provider::new(&h.provider_server.uri()).unwrap();
    assert_eq!(
        publish_block_with_feature(&h.feature(), &original, BUNDLE, &provider)
            .await
            .unwrap_err()
            .status,
        StatusCode::FORBIDDEN
    );
    assert!(h.calls("/api/v1/obo/uploads/reserve").await.is_empty());
    h.approve("approve-original-private-block").await;
    h.lose_commit.store(true, Ordering::SeqCst);
    assert_eq!(
        publish_block_with_feature(&h.feature(), &original, BUNDLE, &provider)
            .await
            .unwrap_err()
            .status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    h.store = FeatureStore::open(&h.dir.path().join("features.sqlite"), &[9; 32]).unwrap();
    let entry = publish_block_with_feature(&h.feature(), &original, BUNDLE, &provider)
        .await
        .unwrap();
    assert_eq!(
        publish_block_with_feature(&h.feature(), &original, BUNDLE, &provider)
            .await
            .unwrap(),
        entry
    );
    assert_eq!(h.calls("/api/v1/obo/uploads/reserve").await.len(), 1);
    assert_eq!(h.calls("/api/v1/obo/uploads/commit").await.len(), 1);
    assert!(h.calls("/api/v1/obo/link-access").await.is_empty());
    {
        let manifest = h.manifest.lock().unwrap();
        assert_eq!(manifest.as_ref().unwrap()["name"], "gene.md");
        assert_eq!(
            manifest.as_ref().unwrap()["parent_path"],
            format!("/blocks/{ORG}/gene/creativity/{}", original.version)
        );
    }
    let mut changed = original.clone();
    changed.visibility = silicon_starter_core::Visibility::Public;
    assert_eq!(
        publish_block_with_feature(&h.feature(), &changed, BUNDLE, &provider)
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    changed = original.clone();
    changed.owner = "another".into();
    assert_eq!(
        publish_block_with_feature(&h.feature(), &changed, BUNDLE, &provider)
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn public_block_requires_explicit_link_and_unchanged_metadata_on_replay() {
    let h = Harness::new().await;
    h.approve("approve-original-public-block").await;
    let original = block(silicon_starter_core::Visibility::Public);
    let provider = Provider::new(&h.provider_server.uri()).unwrap();
    publish_block_with_feature(&h.feature(), &original, BUNDLE, &provider)
        .await
        .unwrap();
    assert_eq!(h.calls("/api/v1/obo/link-access").await.len(), 1);
    let mut changed = original.clone();
    changed.updated_at = chrono::Utc::now();
    publish_block_with_feature(&h.feature(), &changed, BUNDLE, &provider)
        .await
        .unwrap();
    assert_eq!(h.calls("/api/v1/obo/link-access").await.len(), 1);
    changed.description = "changed publication intent".into();
    assert_eq!(
        publish_block_with_feature(&h.feature(), &changed, BUNDLE, &provider)
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
}
