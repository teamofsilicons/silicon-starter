use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use silicon_starter_core::{
    CreateDiscussion, CreateStarter, Discussion, SEED_YAML, Starter, Version, Visibility, valid_id,
    validate_silicon_yaml,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

mod auth;
mod telemetry;

#[derive(Clone, Default)]
pub struct AppState {
    pub starters: Arc<RwLock<HashMap<String, Starter>>>,
    pub versions: Arc<RwLock<HashMap<String, Vec<Version>>>>,
    pub discussions: Arc<RwLock<HashMap<String, Vec<Discussion>>>>,
    pub sessions: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub data_file: Option<String>,
    pub bundles: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pub auth: Arc<auth::AuthState>,
    pub telemetry: Arc<telemetry::Telemetry>,
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
#[derive(Deserialize)]
struct PushRequest {
    commit: String,
    bundle_base64: String,
    yaml: String,
    #[serde(default)]
    message: String,
}
#[derive(Deserialize)]
struct PublishRequest {
    selector: String,
    version: String,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    notes: String,
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
        auth: Arc::new(auth::AuthState::from_env()),
        telemetry: Arc::new(telemetry::Telemetry::from_env()),
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
        .route("/api/v1/starters/{id}/archive", get(archive))
        .route("/api/v1/starters/{id}/download", get(archive))
        .route("/api/v1/starters/{id}/push", post(push))
        .route("/api/v1/starters/{id}/publish", post(publish))
        .route("/api/v1/starters/{id}/commits", get(commits))
        .route(
            "/api/v1/starters/{id}/discussions",
            get(discussions).post(add_discussion),
        )
        .route("/auth/login", get(auth_login))
        .route(
            "/auth/callback",
            get(auth_callback).post(auth_callback_json),
        )
        .route("/auth/cli", post(auth_cli))
        .route("/auth/cli/status", get(auth_cli_status))
        .route("/auth/session", get(auth_session))
        .route("/auth/logout", post(auth_logout))
        .route("/webhooks/iam", post(iam_webhook))
        .with_state(state)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(
                    std::env::var("STARTER_FRONTEND_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
                        .parse::<HeaderValue>()
                        .expect("valid frontend origin"),
                )
                .allow_credentials(true)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","service":"silicon-starter","api_version":"v1"}))
}
async fn organizations() -> Json<serde_json::Value> {
    Json(json!({"items":[{"id":"tos","name":"teamofsilicons"},{"id":"lab","name":"lab"}]}))
}
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Json<Vec<Starter>> {
    let term = q.q.unwrap_or_default().to_lowercase();
    let authenticated = s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false);
    Json(
        s.starters
            .read()
            .await
            .values()
            .filter(|x| matches!(x.visibility, Visibility::Public) || authenticated)
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
async fn search(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Json<Vec<Starter>> {
    list(
        State(s),
        headers,
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
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Starter>, StatusCode> {
    let x = s
        .starters
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    if matches!(x.visibility, Visibility::Private)
        && !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
            .as_bool()
            .unwrap_or(false)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(x))
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
    headers: HeaderMap,
    Json(input): Json<CreateStarter>,
) -> Result<(StatusCode, Json<Starter>), (StatusCode, Json<serde_json::Value>)> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"authentication required"})),
        ));
    }
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
    s.telemetry.record(
        "backend",
        json!({"type":"starter.created","starter_id":x.id,"source":"api"}),
    );
    Ok((StatusCode::CREATED, Json(x)))
}
async fn star(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Starter>, StatusCode> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut m = s.starters.write().await;
    let x = m.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    x.stars += 1;
    Ok(Json(x.clone()))
}
async fn fork(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Starter>), StatusCode> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
async fn archive(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut starters = s.starters.write().await;
    let starter = starters.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    if matches!(starter.visibility, Visibility::Private)
        && !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
            .as_bool()
            .unwrap_or(false)
    {
        return Err(StatusCode::NOT_FOUND);
    }
    starter.downloads += 1;
    let bundles = s.bundles.read().await;
    let bundle = bundles.get(&id).cloned().unwrap_or_default();
    Ok(Json(json!({
        "bundle_base64": BASE64.encode(bundle),
        "commit": q.get("ref").or_else(|| q.get("version")).cloned().unwrap_or_else(|| "main".into())
    })))
}
async fn push(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PushRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"authentication required"})),
        ));
    }
    if headers.get("x-starter-mode").and_then(|v| v.to_str().ok()) == Some("download") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error":"downloaded starters are not pushable; use starter pull"})),
        ));
    }
    if let Err(e) = validate_silicon_yaml(&input.yaml) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error":e}))));
    }
    let bundle = BASE64.decode(input.bundle_base64.as_bytes()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":format!("invalid bundle_base64: {e}")})),
        )
    })?;
    let mut starters = s.starters.write().await;
    let starter = starters.get_mut(&id).ok_or((
        StatusCode::NOT_FOUND,
        Json(json!({"error":"starter not found"})),
    ))?;
    starter.yaml = input.yaml;
    starter.updated_at = Utc::now();
    starter.version = input.commit.chars().take(8).collect();
    s.bundles.write().await.insert(id.clone(), bundle);
    Ok(Json(
        json!({"id":id,"commit":input.commit,"message":input.message}),
    ))
}
async fn publish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PublishRequest>,
) -> Result<Json<Version>, StatusCode> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !s.starters.read().await.contains_key(&id) {
        return Err(StatusCode::NOT_FOUND);
    }
    let v = Version {
        version: input.version,
        commit: input.commit.unwrap_or(input.selector),
        notes: input.notes,
        published_at: Utc::now(),
    };
    s.versions
        .write()
        .await
        .entry(id)
        .or_default()
        .push(v.clone());
    Ok(Json(v))
}
async fn commits(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    if !s.starters.read().await.contains_key(&id) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(
        s.versions
            .read()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|v| json!({"commit":v.commit,"message":v.notes,"created_at":v.published_at}))
            .collect(),
    ))
}
async fn discussions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<Vec<Discussion>> {
    if let Some(starter) = s.starters.read().await.get(&id)
        && matches!(starter.visibility, Visibility::Private)
        && !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
            .as_bool()
            .unwrap_or(false)
    {
        return Json(Vec::new());
    }
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
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CreateDiscussion>,
) -> Result<(StatusCode, Json<Discussion>), StatusCode> {
    if !s.auth.status(session_id(&headers).as_deref()).await["authenticated"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
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
    let cookie_state = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("starter_login_state="))
        });
    match s
        .auth
        .login(
            slt,
            cookie_state,
            q.get("state").map(String::as_str),
            &app_id(),
            &app_secret(),
        )
        .await
    {
        Ok(session) => (
            [(
                "set-cookie",
                format!("starter_session={session}; HttpOnly; SameSite=Lax; Secure; Path=/"),
            )],
            Json(json!({"authenticated":true,"session_id":session})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error":e}))).into_response(),
    }
}
#[derive(Deserialize)]
struct AuthCallbackBody {
    slt: String,
    state: Option<String>,
}
async fn auth_callback_json(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<AuthCallbackBody>,
) -> impl IntoResponse {
    let expected = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("starter_login_state="))
        });
    match s
        .auth
        .login(
            &body.slt,
            expected,
            body.state.as_deref(),
            &app_id(),
            &app_secret(),
        )
        .await
    {
        Ok(session) => (
            [(
                "set-cookie",
                format!("starter_session={session}; HttpOnly; SameSite=Lax; Secure; Path=/"),
            )],
            Json(json!({"authenticated":true})),
        )
            .into_response(),
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
    match s
        .auth
        .login(slt, None, None, &app_id(), &app_secret())
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(json!({"authenticated":true,"session_id":session})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error":e}))).into_response(),
    }
}
async fn auth_cli_status(headers: HeaderMap, State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(s.auth.status(session_id(&headers).as_deref()).await)
}
async fn auth_session(headers: HeaderMap, State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(s.auth.status(session_id(&headers).as_deref()).await)
}
async fn auth_logout(headers: HeaderMap, State(s): State<AppState>) -> impl IntoResponse {
    if let Some(id) = session_id(&headers) {
        let _ = s.auth.remove(&id).await;
    }
    (
        [("set-cookie", "starter_session=; Max-Age=0; Path=/")],
        Json(json!({"authenticated":false})),
    )
}
fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-starter-session")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.split(';')
                        .find_map(|p| p.trim().strip_prefix("starter_session=").map(str::to_owned))
                })
        })
}
fn app_id() -> String {
    std::env::var("STARTER_IAM_APP_ID").unwrap_or_else(|_| "tos>starter".into())
}
fn app_secret() -> String {
    std::env::var("STARTER_IAM_APP_SECRET").unwrap_or_default()
}
async fn iam_webhook(headers: HeaderMap, body: axum::body::Bytes) -> impl IntoResponse {
    let Some(secret) = std::env::var_os("STARTER_IAM_WEBHOOK_SECRET") else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    if !auth::verify_webhook_now(&headers, &body, secret.to_string_lossy().as_bytes()) {
        return StatusCode::UNAUTHORIZED;
    }
    StatusCode::NO_CONTENT
}
pub async fn run(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let state = seeded_state();
    state.auth.load().await?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

#[cfg(test)]
mod webhook_tests {
    use super::*;

    #[test]
    fn rejects_stale_signatures() {
        assert!(!auth::verify_webhook_now(
            &HeaderMap::new(),
            b"{}",
            b"secret"
        ));
    }
}
