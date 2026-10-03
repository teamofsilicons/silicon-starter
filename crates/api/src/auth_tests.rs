use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

fn pair(access: &str, refresh: &str) -> Value {
    json!({"access_token":access,"refresh_token":refresh,"token_type":"Bearer","expires_in":1800,"scope":"identity:read","org_id":"tos","actor":{"type":"carbon","public_id":"c:alice"}})
}
fn proof() -> Value {
    json!({"active":true,"actor_type":"carbon","public_id":"c:alice","client_id":"starter","audience":"starter","org_id":"tos","membership_id":"c:alice[tos]","expires_at":Utc::now().timestamp()+1800,"authorization_epoch":1,"scope":"identity:read","authorization":{"actor_type":"carbon","public_id":"c:alice","org_id":"tos","organization_id":"11111111-1111-4111-8111-111111111111","membership_id":"c:alice[tos]","membership_version":1,"authorization_epoch":1,"audience":"starter","testing_environment_id":null,"scopes":["identity:read"],"org_role":null,"tags":null}})
}
fn tokens() -> IamTokens {
    tokens_of(
        serde_json::from_value(pair("oat_original", "ort_original")).unwrap(),
        &World::default(),
    )
    .unwrap()
}
async fn setup(path: Option<PathBuf>, server: &MockServer) -> AuthState {
    let iam = Iam::connect(&server.uri(), "starter", "secret", None)
        .await
        .unwrap();
    let state = AuthState {
        file: path,
        key: [71; 32],
        fixture: false,
        ..AuthState::default()
    };
    assert!(state.iam.set(iam).is_ok());
    state
}
async fn allow_proof(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(proof()))
        .mount(server)
        .await;
}
#[test]
fn ordinary_proof_rejects_legacy_multiple_org_wrong_identity_and_obo() {
    let valid = proof();
    let mut token = tokens();
    assert!(
        validate_proof(
            &serde_json::from_value(valid.clone()).unwrap(),
            "starter",
            &mut token
        )
        .unwrap()
    );
    for (pointer, value) in [
        ("/org_id", json!("lab")),
        ("/actor_type", json!("silicon")),
        ("/public_id", json!("c:bobby")),
        ("/client_id", json!("briefcase")),
        ("/audience", json!("briefcase")),
        ("/membership_id", json!("c:alice[lab]")),
        ("/authorization/org_id", json!("lab")),
        ("/authorization/public_id", json!("c:bobby")),
        ("/authorization/audience", json!("briefcase")),
        ("/authorization/membership_version", json!(0)),
        ("/authorization/authorization_epoch", json!(2)),
        (
            "/authorization/testing_environment_id",
            json!(Uuid::new_v4()),
        ),
        ("/authorization/scopes", json!(["obo:root"])),
        ("/scope", json!("obo:root")),
        ("/expires_at", json!(Utc::now().timestamp() - 1)),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            validate_proof(
                &serde_json::from_value(invalid).unwrap(),
                "starter",
                &mut token
            )
            .is_err(),
            "{pointer}"
        );
    }
    let mut legacy = valid.clone();
    legacy["authorizations"] = json!([valid["authorization"].clone()]);
    assert!(
        validate_proof(
            &serde_json::from_value(legacy).unwrap(),
            "starter",
            &mut token
        )
        .is_err()
    );
    let mut legacy = valid;
    legacy["authorization"] = Value::Null;
    assert!(
        validate_proof(
            &serde_json::from_value(legacy).unwrap(),
            "starter",
            &mut token
        )
        .is_err()
    );
    for mut invalid in [pair("oat", "ort"); 1] {
        invalid["org_id"] = Value::Null;
        assert!(tokens_of(serde_json::from_value(invalid).unwrap(), &World::default()).is_err());
    }
}
#[tokio::test]
async fn durable_login_recovers_after_introspection_failure_without_redeeming_slt_twice() {
    let server = MockServer::start().await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sessions");
    let state = setup(Some(file.clone()), &server).await;
    let (nonce, group) = state.begin_browser_login(None).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("oat_login", "ort_login")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        state
            .login("oac_private", Some((&nonce, &group)), None, None)
            .await
            .is_err()
    );
    let raw = std::fs::read(&file).unwrap();
    for secret in ["oac_private", "oat_login", "ort_login", "c:alice"] {
        assert!(!raw.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    server.verify().await;
    server.reset().await;
    allow_proof(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let restarted = setup(Some(file.clone()), &server).await;
    let id = restarted
        .login("oac_private", Some((&nonce, &group)), None, None)
        .await
        .unwrap();
    assert_eq!(
        restarted
            .login("oac_private", Some((&nonce, &group)), None, None)
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        restarted.get(&id).await.unwrap().unwrap().actor.unwrap()["public_id"],
        "c:alice"
    );
    let other = restarted.begin_browser_login(None).await.unwrap();
    assert!(
        restarted
            .login("oac_private", Some((&other.0, &other.1)), None, None)
            .await
            .is_err()
    );
    let contexts = restarted.contexts(Some(&group), Some(&id)).await.unwrap();
    let context = contexts["contexts"][0]["context_id"].as_str().unwrap();
    assert_eq!(restarted.select(Some(&group), context).await.unwrap(), id);
    assert!(restarted.select(Some(&other.1), context).await.is_err());
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/revoke"))
        .and(body_string_contains("ort_login"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert!(restarted.remove(&id).await.unwrap());
    assert!(restarted.get(&id).await.unwrap().is_none());
    assert!(
        restarted
            .login("oac_private", Some((&nonce, &group)), None, None)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn cross_process_refresh_is_serialized_and_keeps_context_immutable() {
    let server = MockServer::start().await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sessions");
    let first = setup(Some(file.clone()), &server).await;
    let second = setup(Some(file), &server).await;
    let mut original = tokens();
    original.expires_at = Some(0);
    let context = original.context_id.clone();
    let id = first.insert(original).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .and(body_string_contains("refresh_token=ort_original"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("oat_rotated", "ort_rotated")))
        .expect(1)
        .mount(&server)
        .await;
    allow_proof(&server).await;
    let (left, right) = tokio::join!(first.get(&id), second.get(&id));
    for result in [left, right] {
        let row = result.unwrap().unwrap();
        assert_eq!(row.access_token, "oat_rotated");
        assert_eq!(row.context_id, context);
    }
}
#[tokio::test]
async fn rotated_pair_survives_a_restart_before_proof_and_uses_original_expiry() {
    let server = MockServer::start().await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("sessions");
    let state = setup(Some(file.clone()), &server).await;
    let mut original = tokens();
    original.expires_at = Some(0);
    let id = state.insert(original).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("oat_rotated", "ort_rotated")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    assert!(state.get(&id).await.is_err());
    server.verify().await;
    server.reset().await;
    let restarted = setup(Some(file), &server).await;
    allow_proof(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let loaded = restarted.get(&id).await.unwrap().unwrap();
    assert_eq!(loaded.refresh_token, "ort_rotated");
    assert!(loaded.expires_at.unwrap() <= Utc::now().timestamp() + 1800);
}
#[tokio::test]
async fn refresh_cannot_change_actor_or_selected_org() {
    for field in ["actor", "org_id"] {
        let server = MockServer::start().await;
        let state = setup(None, &server).await;
        let mut original = tokens();
        original.expires_at = Some(0);
        let id = state.insert(original).await.unwrap();
        let mut wrong = pair("oat_wrong", "ort_wrong");
        wrong[field] = if field == "actor" {
            json!({"type":"carbon","public_id":"c:bobby"})
        } else {
            json!("lab")
        };
        Mock::given(method("POST"))
            .and(path("/api/v1/app-auth/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_json(wrong))
            .mount(&server)
            .await;
        assert!(state.get(&id).await.unwrap_err().contains("immutable"));
    }
}
#[tokio::test]
async fn legacy_store_is_not_adopted_and_invalid_sessions_never_gain_authority() {
    let server = MockServer::start().await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sessions");
    std::fs::write(&path, b"{\"identifier_schema\":1}").unwrap();
    let state = setup(Some(path.clone()), &server).await;
    assert!(state.load().await.unwrap_err().contains("Legacy"));
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"identifier_schema\":1}");
    let state = AuthState::default();
    for orgs in [vec![], vec!["tos".into(), "lab".into()]] {
        let mut invalid = tokens();
        invalid.org_ids = orgs;
        invalid.expires_at = Some(Utc::now().timestamp() + 60);
        let id = state.insert(invalid).await.unwrap();
        assert!(state.get(&id).await.unwrap().is_none());
    }
}
#[tokio::test]
async fn stale_tab_and_wrong_browser_cannot_mutate_or_reuse_private_reads() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let server = MockServer::start().await;
    allow_proof(&server).await;
    let auth = setup(None, &server).await;
    let mut t = tokens();
    t.expires_at = Some(Utc::now().timestamp() + 1800);
    let marker = t.context_id.clone();
    let id = auth.insert(t).await.unwrap();
    let app = crate::router(crate::AppState {
        auth: Arc::new(auth),
        ..Default::default()
    });
    for (method, path, context) in [
        ("GET", "/api/v1/starters", Some("stale")),
        ("POST", "/auth/logout", Some("stale")),
        ("GET", "/api/v1/starters", None),
        ("POST", "/api/v1/starters", Some("anonymous")),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("cookie", format!("starter_session={id}"));
        if let Some(context) = context {
            req = req.header("x-starter-context", context);
        }
        let response = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/starters")
                .header("x-starter-session", id)
                .header("x-starter-context", marker)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}
#[test]
fn state_and_webhook_require_exact_unambiguous_security_values() {
    let nonce = random_secret();
    assert!(valid_login_state(Some(&nonce), Some(&nonce)));
    assert!(!valid_login_state(Some(&nonce), Some("bad")));
    let mut mac = <HmacSha256 as Mac>::new_from_slice(b"secret").unwrap();
    mac.update(b"1000.{}");
    let sig = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
    let mut h = HeaderMap::new();
    h.insert("x-silicon-iam-timestamp", "1000".parse().unwrap());
    h.insert("x-silicon-iam-key-version", "1".parse().unwrap());
    h.insert("x-silicon-iam-signature", sig.parse().unwrap());
    assert!(verify_webhook(&h, b"{}", b"secret", 1000));
    h.append("x-silicon-iam-signature", sig.parse().unwrap());
    assert!(!verify_webhook(&h, b"{}", b"secret", 1000));
}

#[tokio::test]
async fn browser_callback_saves_two_independent_accounts_and_selects_only_its_group() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let server = MockServer::start().await;
    for (slt, access, actor, org) in [
        ("oac_alice", "oat_alice", "c:alice", "tos"),
        ("oac_bobby", "oat_bobby", "c:bobby", "lab"),
    ] {
        let mut reply = pair(access, &format!("ort_{actor}"));
        reply["actor"]["public_id"] = json!(actor);
        reply["org_id"] = json!(org);
        Mock::given(method("POST"))
            .and(path("/api/v1/app-auth/tokens"))
            .and(body_string_contains(slt))
            .respond_with(ResponseTemplate::new(200).set_body_json(reply))
            .expect(1)
            .mount(&server)
            .await;
        let mut reply = proof();
        reply["public_id"] = json!(actor);
        reply["org_id"] = json!(org);
        reply["membership_id"] = json!(format!("{actor}[{org}]"));
        reply["authorization"]["public_id"] = json!(actor);
        reply["authorization"]["org_id"] = json!(org);
        reply["authorization"]["membership_id"] = json!(format!("{actor}[{org}]"));
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .and(body_string_contains(access))
            .respond_with(ResponseTemplate::new(200).set_body_json(reply))
            .mount(&server)
            .await;
    }
    let auth = setup(None, &server).await;
    let app = crate::router(crate::AppState {
        auth: Arc::new(auth),
        ..Default::default()
    });
    let mut cookies: HashMap<String, String> = HashMap::new();
    let mut contexts = Vec::new();
    for (slt, actor) in [("oac_alice", "c:alice"), ("oac_bobby", "c:bobby")] {
        let cookie_header = cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login")
                    .header("cookie", cookie_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 307);
        for cookie in response.headers().get_all("set-cookie") {
            let (k, v) = cookie
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .split_once('=')
                .unwrap();
            cookies.insert(k.into(), v.into());
        }
        let uri = url::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let callback = uri
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let callback = url::Url::parse(&callback).unwrap();
        assert!(
            callback
                .query_pairs()
                .any(|(key, v)| key == "state" && v == cookies["starter_login_state"])
        );
        let cookie_header = cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/callback")
                    .header("cookie", &cookie_header)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"slt":slt,"state":cookies["starter_login_state"]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        for cookie in response.headers().get_all("set-cookie") {
            let (k, v) = cookie
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .split_once('=')
                .unwrap();
            cookies.insert(k.into(), v.into());
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/session")
                    .header(
                        "cookie",
                        format!("starter_session={}", cookies["starter_session"]),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let status: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(status["actor"]["public_id"], actor);
        contexts.push(status["context_id"].as_str().unwrap().to_string());
    }
    let cookie_header = cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/contexts")
                .header("cookie", &cookie_header)
                .header("x-starter-context", &contexts[1])
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let listed: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(listed["contexts"].as_array().unwrap().len(), 2);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("cookie", &cookie_header)
                .header("x-starter-context", &contexts[0])
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/context")
                .header("cookie", &cookie_header)
                .header("x-starter-context", &contexts[1])
                .header("content-type", "application/json")
                .body(Body::from(json!({"context_id":contexts[0]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .starts_with("starter_session=")
    );
}

#[tokio::test]
async fn cli_login_receipt_binds_input_and_expires_instead_of_redeeming_an_old_slt() {
    let server = MockServer::start().await;
    let state = setup(None, &server).await;
    let key = Uuid::new_v4().to_string();
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("oat_login", "ort_login")))
        .expect(1)
        .mount(&server)
        .await;
    allow_proof(&server).await;
    let id = state
        .login("oac_private", None, Some(&key), Some("tos"))
        .await
        .unwrap();
    assert_eq!(
        state
            .login("oac_private", None, Some(&key), Some("tos"))
            .await
            .unwrap(),
        id
    );
    for (slt, org) in [("oac_other", "tos"), ("oac_private", "lab")] {
        assert!(
            state
                .login(slt, None, Some(&key), Some(org))
                .await
                .unwrap_err()
                .contains("different input")
        );
    }
    {
        let mut data = state.inner.lock().await;
        for receipt in data.logins.values_mut() {
            receipt.expires_at = Utc::now().timestamp() - 1;
        }
    }
    assert!(
        state
            .login("oac_private", None, Some(&key), Some("tos"))
            .await
            .unwrap_err()
            .contains("expired")
    );
    assert!(state.get(&id).await.unwrap().is_some());
}
