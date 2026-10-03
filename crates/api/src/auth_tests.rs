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
    let (nonce, group) = state
        .begin_browser_login(None, "carbon", "http://127.0.0.1:3000/".into(), false)
        .await
        .unwrap();
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
        .resume_browser_login(&nonce, &group)
        .await
        .unwrap();
    assert!(
        restarted.inner.lock().await.states[&hash(&nonce)]
            .slt
            .is_none()
    );
    assert_eq!(
        restarted
            .resume_browser_login(&nonce, &group)
            .await
            .unwrap(),
        id
    );
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
    let other = restarted
        .begin_browser_login(None, "carbon", "http://127.0.0.1:3000/".into(), false)
        .await
        .unwrap();
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
fn state_requires_exact_unambiguous_security_values() {
    let nonce = random_secret();
    assert!(valid_login_state(Some(&nonce), Some(&nonce)));
    assert!(!valid_login_state(Some(&nonce), Some("bad")));
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
                    .uri("/auth/login?identity_kind=carbon")
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

#[tokio::test]
async fn browser_kind_is_proved_by_introspection_even_when_exchange_omits_actor() {
    let server = MockServer::start().await;
    let auth = setup(None, &server).await;
    let mut reply = pair("oat_login", "ort_login");
    reply["actor"] = Value::Null;
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .expect(2)
        .mount(&server)
        .await;
    allow_proof(&server).await;
    for kind in ["silicon", "carbon"] {
        let (nonce, group) = auth
            .begin_browser_login(None, kind, "http://127.0.0.1:3000/".into(), true)
            .await
            .unwrap();
        let result = auth
            .login(&format!("oac_{kind}"), Some((&nonce, &group)), None, None)
            .await;
        if kind == "silicon" {
            assert!(result.unwrap_err().contains("kind does not match"));
            assert!(
                auth.contexts(Some(&group), None).await.unwrap()["contexts"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        } else {
            let session = auth.get(&result.unwrap()).await.unwrap().unwrap();
            assert_eq!(session.actor.unwrap()["type"], "carbon");
            assert_eq!(session.org_id.as_deref(), Some("tos"));
        }
    }
    // Neither absent identity nor an inactive token can establish the chosen kind.
    for (field, value) in [
        ("actor_type", Value::Null),
        ("public_id", Value::Null),
        ("active", json!(false)),
    ] {
        let mut seen = proof();
        seen[field] = value;
        let mut token = tokens();
        token.actor = None;
        assert!(!matches!(
            validate_proof(
                &serde_json::from_value(seen).unwrap(),
                "starter",
                &mut token
            ),
            Ok(true)
        ));
    }
}

#[tokio::test]
async fn popup_callback_returns_status_only_and_fullpage_uses_stored_safe_return() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let server = MockServer::start().await;
    let auth = setup(None, &server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pair("oat_secret", "ort_secret")))
        .expect(2)
        .mount(&server)
        .await;
    allow_proof(&server).await;
    let app = crate::router(crate::AppState {
        auth: Arc::new(auth),
        ..Default::default()
    });
    for popup in [true, false] {
        let request = if popup {
            Request::builder().method("POST").uri("/auth/attempt")
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:3000")
                .body(Body::from(json!({"identity_kind":"carbon","return_to":"/starters?from=login","popup":true}).to_string())).unwrap()
        } else {
            Request::builder()
                .uri("/auth/login?identity_kind=carbon&return_to=%2Fstarters%3Ffrom%3Dlogin")
                .body(Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), if popup { 200 } else { 307 });
        let cookies = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        let (url, attempt_id) = if popup {
            let body: Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(body["iam_origin"], "https://auth.iam.teamofsilicons.com");
            (
                body["login_url"].as_str().unwrap().to_string(),
                body["attempt_id"].as_str().unwrap().to_string(),
            )
        } else {
            (
                response.headers()["location"].to_str().unwrap().to_string(),
                String::new(),
            )
        };
        let url = url::Url::parse(&url).unwrap();
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "identity_kind" && v == "carbon")
        );
        assert_eq!(
            url.query_pairs()
                .any(|(k, v)| k == "display" && v == "popup"),
            popup
        );
        assert!(!url.query_pairs().any(|(k, _)| k == "org_id"));
        let callback = url
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .unwrap()
            .1
            .to_string();
        let mut callback = url::Url::parse(&callback).unwrap();
        callback.query_pairs_mut().append_pair(
            "slt",
            if popup {
                "oac_popup_secret"
            } else {
                "oac_fullpage_secret"
            },
        );
        if popup {
            Mock::given(method("POST"))
                .and(path("/api/v1/oauth/introspect"))
                .respond_with(ResponseTemplate::new(503))
                .with_priority(1)
                .up_to_n_times(1)
                .mount(&server)
                .await;
            let failed = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{}?{}", callback.path(), callback.query().unwrap()))
                        .header("cookie", &cookies)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(!failed.headers().contains_key("set-cookie"));
            let page = String::from_utf8(
                failed
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap();
            assert!(page.contains("Retry this login"));
            assert!(page.contains("\"status\":\"error\""));
            assert!(page.contains("if(false)window.close()"));
            for secret in ["oac_popup_secret", "oat_secret", "ort_secret"] {
                assert!(!page.contains(secret));
            }
            // Retry carries state only; the original SLT and mutation receipt stay on the server.
            let state = callback
                .query_pairs()
                .find(|(k, _)| k == "state")
                .unwrap()
                .1
                .to_string();
            callback.set_query(None);
            callback.query_pairs_mut().append_pair("state", &state);
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{}?{}", callback.path(), callback.query().unwrap()))
                    .header("cookie", cookies)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.headers().contains_key("set-cookie"));
        if popup {
            assert_eq!(response.status(), 200);
            let page = String::from_utf8(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap();
            assert!(page.contains(&attempt_id));
            assert!(page.contains("\"status\":\"complete\""));
            assert!(page.contains("\"http://127.0.0.1:3000\""));
            for secret in [
                "oac_popup_secret",
                "oat_secret",
                "ort_secret",
                "starter_session",
            ] {
                assert!(!page.contains(secret));
            }
        } else {
            assert_eq!(response.status(), 303);
            assert_eq!(
                response.headers()["location"],
                "http://127.0.0.1:3000/starters?from=login"
            );
        }
    }
    for destination in [
        "//evil.test/path",
        "https://evil.test",
        "https://name:secret@127.0.0.1:3000/",
        "/\\evil.test",
        "/\nredirect",
    ] {
        assert!(
            crate::auth_routes::validated_return(Some(destination)).is_err(),
            "{destination}"
        );
    }
    assert!(crate::auth_routes::validated_return(Some("/starters?q=hello#top")).is_ok());
}

#[tokio::test]
async fn anonymous_boot_expires_obsolete_browser_cookie_without_weakening_context_fencing() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = crate::router(crate::AppState::default());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/session")
                .header("cookie", "starter_session=stale-before-iam5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let expired = response.headers()["set-cookie"].to_str().unwrap();
    assert!(expired.starts_with("starter_session=;"));
    assert!(expired.contains("Max-Age=0"));
    assert!(expired.contains("Path=/"));
    let status: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(status, json!({"authenticated":false}));
    // The browser applies the expiration before its anonymous catalog request.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/starters")
                .header("x-starter-context", "anonymous")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    for cookie in [None, Some("starter_session=stale-before-iam5")] {
        let mut request = Request::builder()
            .uri("/api/v1/starters")
            .header("x-starter-context", "old-account-context");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/cli/status")
                .header("x-starter-session", "stale-cli-session")
                .header("cookie", "starter_session=browser-context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("set-cookie"));
}

#[tokio::test]
async fn webhook_world_boundary_rejects_cross_plane_wrong_key_and_prior_clean_events() {
    use axum::http::StatusCode;
    use std::sync::Mutex as StdMutex;
    const SECRET: &str = "test-signing-key-with-at-least-32-characters";
    const KEY: &str = "0123456789abcdefghijklmnopqrstuv";
    let event_id = Uuid::new_v4();
    let environment_id = Uuid::new_v4();
    let now = Utc::now();
    let production = json!({"spec_version":"1.0","event_id":event_id,"event_type":"organization.membership.updated.v1","occurred_at":now.to_rfc3339(),"organization_id":null,"aggregate":{"type":"membership","id":Uuid::new_v4(),"version":1},"data":{}});
    let mut metadata = production.clone();
    metadata.as_object_mut().unwrap().remove("data");
    metadata["environment_id"] = json!(environment_id);
    metadata["generation"] = json!(2);
    let testing = json!({"test":{"testing_key":KEY,"metadata":metadata,"data":{}}});
    let send = |state: crate::AppState, body: Value| async move {
        let raw = serde_json::to_vec(&body).unwrap();
        let stamp = Utc::now().timestamp().to_string();
        let mut mac = <HmacSha256 as Mac>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(stamp.as_bytes());
        mac.update(b".");
        mac.update(&raw);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-silicon-iam-event-id",
            event_id.to_string().parse().unwrap(),
        );
        headers.insert("x-silicon-iam-key-version", "1".parse().unwrap());
        headers.insert("x-silicon-iam-timestamp", stamp.parse().unwrap());
        headers.insert(
            "x-silicon-iam-signature",
            format!("v1={}", hex::encode(mac.finalize().into_bytes()))
                .parse()
                .unwrap(),
        );
        crate::receive_iam_webhook(&state, &headers, &raw, SECRET).await
    };
    let prod_state = crate::AppState::default();
    assert_eq!(
        send(prod_state.clone(), testing.clone()).await,
        StatusCode::FORBIDDEN
    );
    let mixed = json!({"test":testing["test"],"metadata":{"event_id":event_id},"data":{}});
    assert_eq!(
        send(prod_state.clone(), mixed.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(prod_state.iam_event_ids.read().await.is_empty());
    assert_eq!(
        send(prod_state.clone(), production.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(prod_state.clone(), production.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(prod_state.iam_event_ids.read().await.len(), 1);

    let server = MockServer::start().await;
    let context = Arc::new(StdMutex::new(
        json!({"environment_id":environment_id,"application":{"app_id":"starter","base_url":"https://example.test","app_scope":{"iam":[],"external":[]},"webhook_scope":[],"testing_idle_days":30},"environment":{"environment_id":environment_id,"org_id":"tos","name":"Webhook isolation","version":2,"key_generation":1,"cleaned_at":(now-chrono::Duration::minutes(1)).to_rfc3339(),"created_at":(now-chrono::Duration::minutes(5)).to_rfc3339(),"creator_type":"carbon","creator_id":"c:author"}}),
    ));
    let reported = context.clone();
    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(reported.lock().unwrap().clone())
        })
        .mount(&server)
        .await;
    let iam = Iam::connect(
        &server.uri(),
        "starter",
        "",
        Some((
            "ask_testSecrettestSecrettestSecrettestSecrettes".into(),
            KEY.into(),
        )),
    )
    .await
    .unwrap();
    let world = Arc::new(iam.world.clone());
    let auth = AuthState {
        fixture: false,
        ..Default::default()
    };
    assert!(auth.iam.set(iam).is_ok());
    let state = crate::AppState {
        auth: Arc::new(auth),
        world,
        ..Default::default()
    };
    let mut wrong_key = testing.clone();
    wrong_key["test"]["testing_key"] = json!("ABCDEFGHIJKLMNOPQRSTUVWXYZ012345");
    let mut wrong_world = testing.clone();
    wrong_world["test"]["metadata"]["environment_id"] = json!(Uuid::new_v4());
    let mut old = testing.clone();
    old["test"]["metadata"]["occurred_at"] =
        json!((now - chrono::Duration::minutes(2)).to_rfc3339());
    for (name, rejected) in [
        ("production", production),
        ("key", wrong_key),
        ("world", wrong_world),
        ("old", old),
    ] {
        assert_eq!(
            send(state.clone(), rejected).await,
            StatusCode::FORBIDDEN,
            "{name}"
        );
        assert!(state.iam_event_ids.read().await.is_empty());
    }
    assert_eq!(send(state.clone(), mixed).await, StatusCode::UNAUTHORIZED);
    assert!(state.iam_event_ids.read().await.is_empty());
    assert_eq!(
        send(state.clone(), testing.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(state.iam_event_ids.read().await.len(), 1);
    // A fresh signature cannot revive this listener after the verified world is cleaned/rotated.
    state.iam_event_ids.write().await.clear();
    context.lock().unwrap()["environment"]["key_generation"] = json!(2);
    assert_eq!(
        send(state.clone(), testing).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(state.iam_event_ids.read().await.is_empty());
    assert!(state.starters.read().await.is_empty());
}
