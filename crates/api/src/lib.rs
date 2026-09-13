use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use silicon_starter_core::{
    CreateDiscussion, CreateStarter, Discussion, SEED_YAML, Starter, Version, Visibility, valid_id,
    validate_silicon_yaml,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;
#[derive(Clone, Default)]
pub struct AppState {
    pub starters: Arc<RwLock<HashMap<String, Starter>>>,
    pub versions: Arc<RwLock<HashMap<String, Vec<Version>>>>,
    pub discussions: Arc<RwLock<HashMap<String, Vec<Discussion>>>>,
    pub sessions: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub data_file: Option<String>,
}
#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    org: Option<String>,
    visibility: Option<Visibility>,
}
#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

pub fn seeded_state() -> AppState {
    let mut map = HashMap::new();
    let now = Utc::now();
    for (id, name, desc, tags) in [
        (
            "tos.agents",
            "Agents",
            "Composable building blocks for reliable silicon agents.",
            vec!["agents", "core"],
        ),
        (
            "tos.vision",
            "Vision Lab",
            "A production-ready vision pipeline with memory and tools.",
            vec!["vision", "multimodal"],
        ),
        (
            "tos.knowledge",
            "Knowledge Graph",
            "Turn source material into a queryable silicon memory.",
            vec!["memory", "search"],
        ),
        (
            "lab.orbit",
            "Orbit",
            "A minimal coordinator for multi-silicon workflows.",
            vec!["orchestration"],
        ),
    ] {
        map.insert(
            id.into(),
            Starter {
                id: id.into(),
                name: name.into(),
                description: desc.into(),
                owner: id.split('.').next().unwrap_or("tos").into(),
                visibility: Visibility::Public,
                version: "1.0".into(),
                downloads: 0,
                stars: 0,
                updated_at: now,
                tags: tags.into_iter().map(str::to_string).collect(),
                yaml: SEED_YAML.into(),
            },
        );
    }
    AppState {
        starters: Arc::new(RwLock::new(map)),
        ..Default::default()
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/organizations", get(organizations))
        .route("/api/v1/starters", get(list).post(create))
        .route("/api/v1/search", get(search))
        .route("/api/v1/starters/{id}", get(show))
        .route("/api/v1/starters/{id}/versions", get(versions))
        .route("/api/v1/starters/{id}/star", post(star))
        .route("/api/v1/starters/{id}/fork", post(fork))
        .route(
            "/api/v1/starters/{id}/discussions",
            get(discussions).post(add_discussion),
        )
        .route("/auth/login", get(auth_login))
        .route("/auth/callback", get(auth_callback))
        .route("/auth/cli", post(auth_cli))
        .route("/auth/cli/status", get(auth_cli_status))
        .route("/webhooks/iam", post(iam_webhook))
        .with_state(state)
        .layer(tower_http::cors::CorsLayer::permissive())
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","service":"silicon-starter","api_version":"v1"}))
}
async fn organizations() -> Json<serde_json::Value> {
    Json(json!({"items":[{"id":"tos","name":"teamofsilicons"},{"id":"lab","name":"lab"}]}))
}
async fn list(State(s): State<AppState>, Query(q): Query<ListQuery>) -> Json<Vec<Starter>> {
    let term = q.q.unwrap_or_default().to_lowercase();
    Json(
        s.starters
            .read()
            .await
            .values()
            .filter(|x| q.org.as_ref().is_none_or(|o| &x.owner == o))
            .filter(|x| {
                q.visibility.as_ref().is_none_or(|v| {
                    std::mem::discriminant(&x.visibility) == std::mem::discriminant(v)
                })
            })
            .filter(|x| {
                term.is_empty()
                    || format!("{} {} {}", x.name, x.description, x.tags.join(" "))
                        .to_lowercase()
                        .contains(&term)
            })
            .cloned()
            .collect(),
    )
}
async fn search(State(s): State<AppState>, Query(q): Query<SearchQuery>) -> Json<Vec<Starter>> {
    list(
        State(s),
        Query(ListQuery {
            q: q.q,
            org: None,
            visibility: None,
        }),
    )
    .await
}
async fn show(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Starter>, StatusCode> {
    s.starters
        .read()
        .await
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
async fn versions(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Version>>, StatusCode> {
    if !s.starters.read().await.contains_key(&id) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(
        s.versions
            .read()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_else(|| {
                vec![Version {
                    version: "1.0".into(),
                    commit: "seed".into(),
                    notes: "Initial release".into(),
                    published_at: Utc::now(),
                }]
            }),
    ))
}
async fn create(
    State(s): State<AppState>,
    Json(input): Json<CreateStarter>,
) -> Result<(StatusCode, Json<Starter>), (StatusCode, Json<serde_json::Value>)> {
    if !valid_id(&input.id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid starter id"})),
        ));
    }
    if let Err(e) = validate_silicon_yaml(&input.yaml) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error":e}))));
    }
    let x = Starter {
        id: input.id.clone(),
        name: input.name,
        description: input.description,
        owner: "local".into(),
        visibility: input.visibility,
        version: "0.1".into(),
        downloads: 0,
        stars: 0,
        updated_at: Utc::now(),
        tags: input.tags,
        yaml: input.yaml,
    };
    s.starters.write().await.insert(input.id, x.clone());
    Ok((StatusCode::CREATED, Json(x)))
}
async fn star(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Starter>, StatusCode> {
    let mut m = s.starters.write().await;
    let x = m.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    x.stars += 1;
    Ok(Json(x.clone()))
}
async fn fork(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Starter>), StatusCode> {
    let mut m = s.starters.write().await;
    let src = m.get(&id).ok_or(StatusCode::NOT_FOUND)?.clone();
    let fork = Starter {
        id: format!("{}.fork-{}", src.id, &Uuid::now_v7().to_string()[..8]),
        name: format!("{} (fork)", src.name),
        owner: "local".into(),
        updated_at: Utc::now(),
        ..src
    };
    m.insert(fork.id.clone(), fork.clone());
    Ok((StatusCode::CREATED, Json(fork)))
}
async fn discussions(State(s): State<AppState>, Path(id): Path<String>) -> Json<Vec<Discussion>> {
    Json(
        s.discussions
            .read()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_default(),
    )
}
async fn add_discussion(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<CreateDiscussion>,
) -> Result<(StatusCode, Json<Discussion>), StatusCode> {
    if input.body.trim().is_empty() || !s.starters.read().await.contains_key(&id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let d = Discussion {
        id: Uuid::now_v7().to_string(),
        starter_id: id.clone(),
        parent_id: input.parent_id,
        author: "authenticated-user".into(),
        body: input.body,
        created_at: Utc::now(),
    };
    s.discussions
        .write()
        .await
        .entry(id)
        .or_default()
        .push(d.clone());
    Ok((StatusCode::CREATED, Json(d)))
}
async fn auth_login() -> impl IntoResponse {
    let state = Uuid::new_v4().to_string();
    let callback = std::env::var("STARTER_FRONTEND_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
        + "/auth/callback";
    let url = format!(
        "https://auth.iam.teamofsilicons.com/login?app_id=tos%3Estarter&redirect_uri={}&state={state}",
        urlencoding::encode(&callback)
    );
    (
        [(
            "set-cookie",
            format!("starter_login_state={state}; HttpOnly; SameSite=Lax; Secure; Path=/"),
        )],
        Redirect::temporary(&url),
    )
}
async fn auth_callback(
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    State(s): State<AppState>,
) -> impl IntoResponse {
    let Some(slt) = q.get("slt") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"IAM callback requires a short-lived token"})),
        )
            .into_response();
    };
    if let Some(expected) = q.get("state") {
        let cookie = headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !cookie.contains(&format!("starter_login_state={expected}")) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"login state does not match the initiating browser"})),
            )
                .into_response();
        }
    }
    match exchange_slt(slt).await {
        Ok((session, token)) => {
            s.sessions.write().await.insert(session.clone(), token);
            (
                [(
                    "set-cookie",
                    format!("starter_session={session}; HttpOnly; SameSite=Lax; Secure; Path=/"),
                )],
                Json(json!({"authenticated":true,"session_id":session})),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error":e}))).into_response(),
    }
}
async fn auth_cli(
    State(s): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(slt) = body.get("slt").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"send only an IAM short-lived token as slt"})),
        )
            .into_response();
    };
    match exchange_slt(slt).await {
        Ok((session, token)) => {
            s.sessions.write().await.insert(session.clone(), token);
            (
                StatusCode::OK,
                Json(json!({"authenticated":true,"session_id":session})),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error":e}))).into_response(),
    }
}
async fn auth_cli_status(headers: HeaderMap, State(s): State<AppState>) -> Json<serde_json::Value> {
    let session = headers
        .get("x-starter-session")
        .and_then(|v| v.to_str().ok());
    let authenticated = session.is_some_and(|id| s.sessions.blocking_read().contains_key(id));
    Json(json!({"authenticated":authenticated}))
}
async fn exchange_slt(slt: &str) -> Result<(String, serde_json::Value), String> {
    let secret = std::env::var("STARTER_IAM_APP_SECRET")
        .map_err(|_| "STARTER_IAM_APP_SECRET is not configured".to_string())?;
    let app_id = std::env::var("STARTER_IAM_APP_ID").unwrap_or_else(|_| "tos>starter".into());
    let response = reqwest::Client::new()
        .post("https://backend.iam.teamofsilicons.com/api/v1/app-auth/tokens")
        .basic_auth(&app_id, Some(secret))
        .header("Idempotency-Key", Uuid::now_v7().to_string())
        .form(&[("app_id", app_id.as_str()), ("slt", slt)])
        .send()
        .await
        .map_err(|e| format!("IAM token exchange failed: {e}"))?;
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("IAM returned invalid JSON: {e}"))?;
    if !status.is_success() {
        return Err(format!("IAM token exchange returned {status}: {body}"));
    }
    Ok((Uuid::now_v7().to_string(), body))
}
async fn iam_webhook(headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    let Some(secret) = std::env::var_os("STARTER_IAM_WEBHOOK_SECRET") else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let Some(sig) = headers
        .get("x-silicon-iam-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::UNAUTHORIZED;
    };
    let ts = headers
        .get("x-silicon-iam-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_iam_signature(secret.to_string_lossy().as_bytes(), sig, ts, &body) {
        return StatusCode::UNAUTHORIZED;
    }
    StatusCode::NO_CONTENT
}
fn verify_iam_signature(secret: &[u8], signature: &str, timestamp: &str, body: &[u8]) -> bool {
    let Ok(ts) = timestamp.parse::<i64>() else {
        return false;
    };
    if (Utc::now().timestamp() - ts).abs() > 300 {
        return false;
    }
    let Some(encoded) = signature.strip_prefix("v1=") else {
        return false;
    };
    let Ok(expected) = hex::decode(encoded) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(format!("{timestamp}.").as_bytes());
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod webhook_tests {
    use super::*;

    #[test]
    fn rejects_stale_signatures() {
        assert!(!verify_iam_signature(b"secret", "v1=00", "1", b"{}"));
    }
}
pub async fn run(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(seeded_state())).await?;
    Ok(())
}
