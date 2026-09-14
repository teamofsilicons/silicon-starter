use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use silicon_starter_core::{
    CreateDiscussion, CreateStarter, Discussion, Starter, Version, Visibility, release_version,
    valid_id, validate_silicon_yaml,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::RwLock;
use uuid::Uuid;

mod auth;
mod briefcase;
mod files;
mod semantic;
mod store;
mod telemetry;

#[derive(Clone, Default)]
pub struct AppState {
    pub starters: Arc<RwLock<HashMap<String, Starter>>>,
    pub versions: Arc<RwLock<HashMap<String, Vec<Version>>>>,
    pub discussions: Arc<RwLock<HashMap<String, Vec<Discussion>>>>,
    pub sessions: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub data_file: Option<String>,
    pub bundles: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pub bundle_commits: Arc<RwLock<HashMap<String, String>>>,
    pub auth: Arc<auth::AuthState>,
    pub telemetry: Arc<telemetry::Telemetry>,
    pub store: Option<Arc<store::Store>>,
    pub iam_event_ids: Arc<RwLock<HashSet<String>>>,
    pub briefcase_entries: Arc<RwLock<HashMap<String, Uuid>>>,
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
    AppState {
        starters: Arc::new(RwLock::new(HashMap::new())),
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
        .route("/api/v1/starters/{id}/files", get(files))
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
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([
                    HeaderName::from_static("content-type"),
                    HeaderName::from_static("x-starter-session"),
                    HeaderName::from_static("x-starter-mode"),
                    HeaderName::from_static("x-silicon-iam-signature"),
                    HeaderName::from_static("x-silicon-iam-timestamp"),
                    HeaderName::from_static("x-silicon-iam-key-version"),
                ]),
        )
}
async fn persist_state(s: &AppState) {
    let Some(store) = &s.store else { return };
    let value = json!({
        "starters": *s.starters.read().await,
        "versions": *s.versions.read().await,
        "discussions": *s.discussions.read().await,
        "bundles": s.bundles.read().await.iter().map(|(k,v)| (k.clone(), BASE64.encode(v))).collect::<HashMap<_,_>>(),
        "bundle_commits": *s.bundle_commits.read().await,
        "iam_event_ids": *s.iam_event_ids.read().await,
        "briefcase_entries": *s.briefcase_entries.read().await,
    });
    if let Err(e) = store.save(value).await {
        eprintln!("starter state persistence failed: {e}");
    }
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","service":"silicon-starter","api_version":"v1"}))
}
async fn authenticated_session(
    s: &AppState,
    headers: &HeaderMap,
) -> Result<auth::IamTokens, StatusCode> {
    let id = session_id(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    s.auth.get(&id).await.ok_or(StatusCode::UNAUTHORIZED)
}
fn choose_organization(
    session: &auth::IamTokens,
    requested: Option<&str>,
) -> Result<String, StatusCode> {
    let orgs = session.organizations();
    match requested {
        Some(org) if orgs.iter().any(|allowed| allowed == org) => Ok(org.to_owned()),
        None if orgs.len() == 1 => Ok(orgs[0].clone()),
        _ => Err(StatusCode::FORBIDDEN),
    }
}
async fn organizations(State(s): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let orgs = authenticated_session(&s, &headers)
        .await
        .map(|session| session.organizations())
        .unwrap_or_default();
    Json(json!({"items":orgs.into_iter().map(|id| json!({"id":id,"name":id})).collect::<Vec<_>>()}))
}
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Json<Vec<Starter>> {
    let term = q.q.unwrap_or_default().to_lowercase();
    let orgs = authenticated_session(&s, &headers)
        .await
        .map(|session| session.organizations())
        .unwrap_or_default();
    Json(
        s.starters
            .read()
            .await
            .values()
            .filter(|x| matches!(x.visibility, Visibility::Public) || orgs.contains(&x.owner))
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
    let Json(mut items) = list(
        State(s),
        headers,
        Query(ListQuery {
            q: q.q.clone(),
            org: None,
            visibility: None,
        }),
    )
    .await;
    let Some(query) = q.q else { return Json(items) };
    if std::env::var_os("GEMINI_API_KEY").is_none() || query.trim().is_empty() {
        return Json(items);
    }
    let Ok(query_embedding) = semantic::embed(&query, true).await else {
        return Json(items);
    };
    let mut scored = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        let text = format!("{} {} {}", item.name, item.description, item.yaml);
        if let Ok(embedding) = semantic::embed(&text, false).await {
            scored.push((semantic::cosine(&query_embedding, &embedding), item));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    Json(scored.into_iter().map(|(_, item)| item).collect())
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
        && !authenticated_session(&s, &headers)
            .await
            .is_ok_and(|session| session.organizations().contains(&x.owner))
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(x))
}
async fn versions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<Version>>, StatusCode> {
    let _ = show(State(s.clone()), headers, Path(id.clone())).await?;
    Ok(Json(
        s.versions
            .read()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_default(),
    ))
}
async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateStarter>,
) -> Result<(StatusCode, Json<Starter>), (StatusCode, Json<serde_json::Value>)> {
    let session = authenticated_session(&s, &headers)
        .await
        .map_err(|status| (status, Json(json!({"error":"authentication required"}))))?;
    let requested_org = input
        .org_id
        .as_deref()
        .or_else(|| input.id.split_once('.').map(|(org, _)| org));
    let owner = choose_organization(&session, requested_org).map_err(|status| {
        (
            status,
            Json(json!({"error":"choose an organization authorized through IAM"})),
        )
    })?;
    if !valid_id(&input.id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid starter id"})),
        ));
    }
    if !input
        .id
        .strip_prefix(&format!("{owner}."))
        .is_some_and(|name| !name.is_empty())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":format!("starter id must begin with {owner}. followed by a name")}),
            ),
        ));
    }
    if let Err(e) = validate_silicon_yaml(&input.yaml) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error":e}))));
    }
    let x = Starter {
        id: input.id.clone(),
        name: input.name,
        description: input.description,
        owner,
        visibility: input.visibility,
        version: "0.1".into(),
        downloads: 0,
        stars: 0,
        updated_at: Utc::now(),
        tags: input.tags,
        yaml: input.yaml,
    };
    {
        let mut starters = s.starters.write().await;
        if starters.contains_key(&input.id) {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error":"starter id already exists"})),
            ));
        }
        starters.insert(input.id, x.clone());
    }
    s.telemetry.record(
        "backend",
        json!({"type":"starter.created","starter_id":x.id,"source":"api"}),
    );
    persist_state(&s).await;
    Ok((StatusCode::CREATED, Json(x)))
}
async fn star(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Starter>, StatusCode> {
    authenticated_session(&s, &headers).await?;
    let _ = show(State(s.clone()), headers, Path(id.clone())).await?;
    let mut m = s.starters.write().await;
    let x = m.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    x.stars += 1;
    let out = x.clone();
    drop(m);
    persist_state(&s).await;
    Ok(Json(out))
}
async fn fork(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<(StatusCode, Json<Starter>), StatusCode> {
    let session = authenticated_session(&s, &headers).await?;
    let owner = choose_organization(&session, q.get("org_id").map(String::as_str))?;
    let Json(src) = show(State(s.clone()), headers, Path(id.clone())).await?;
    let mut m = s.starters.write().await;
    let fork = Starter {
        id: format!("{owner}.fork-{}", Uuid::new_v4().simple()),
        name: format!("{} (fork)", src.name),
        owner,
        stars: 0,
        downloads: 0,
        updated_at: Utc::now(),
        ..src
    };
    m.insert(fork.id.clone(), fork.clone());
    drop(m);
    let bundle = s.bundles.read().await.get(&id).cloned();
    if let Some(bundle) = bundle {
        s.bundles.write().await.insert(fork.id.clone(), bundle);
    }
    let commit = s.bundle_commits.read().await.get(&id).cloned();
    if let Some(commit) = commit {
        s.bundle_commits
            .write()
            .await
            .insert(fork.id.clone(), commit);
    }
    persist_state(&s).await;
    Ok((StatusCode::CREATED, Json(fork)))
}
async fn archive(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = show(State(s.clone()), headers, Path(id.clone())).await?;
    let mut starters = s.starters.write().await;
    let starter = starters.get_mut(&id).ok_or(StatusCode::NOT_FOUND)?;
    starter.downloads += 1;
    let bundles = s.bundles.read().await;
    let bundle = bundles.get(&id).cloned().unwrap_or_default();
    if bundle.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    let requested = q.get("ref").or_else(|| q.get("version"));
    let commit = if let Some(r) = requested {
        s.versions
            .read()
            .await
            .get(&id)
            .and_then(|vs| {
                vs.iter()
                    .find(|v| &v.version == r)
                    .map(|v| v.commit.clone())
            })
            .unwrap_or_else(|| r.clone())
    } else {
        s.bundle_commits
            .read()
            .await
            .get(&id)
            .cloned()
            .unwrap_or_default()
    };
    if commit.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(json!({
        "bundle_base64": BASE64.encode(bundle),
        "commit": commit
    })))
}

/// Browse the stored commit without checking out repository content.
async fn files(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<files::RepositoryFiles>, (StatusCode, Json<serde_json::Value>)> {
    let Json(starter) = show(State(s.clone()), headers, Path(id.clone()))
        .await
        .map_err(|status| (status, Json(json!({"error":"starter not found"}))))?;
    let bundle = s.bundles.read().await.get(&id).cloned();
    let Some(bundle) = bundle else {
        return Ok(Json(files::draft(starter.yaml)));
    };
    let commit = s
        .bundle_commits
        .read()
        .await
        .get(&id)
        .cloned()
        .unwrap_or_default();
    tokio::task::spawn_blocking(move || files::read_bundle(bundle, commit))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"repository read failed"})),
            )
        })?
        .map(Json)
        .map_err(|error| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error":error})),
            )
        })
}
async fn push(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PushRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let session = authenticated_session(&s, &headers)
        .await
        .map_err(|status| (status, Json(json!({"error":"authentication required"}))))?;
    let owner = s
        .starters
        .read()
        .await
        .get(&id)
        .map(|starter| starter.owner.clone())
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"starter not found"})),
        ))?;
    choose_organization(&session, Some(&owner)).map_err(|status| {
        (
            status,
            Json(json!({"error":"only the owning organization can push this starter"})),
        )
    })?;
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
    if !(input.commit.len() == 40 || input.commit.len() == 64)
        || !input.commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"commit must be a full git object id"})),
        ));
    }
    let mut starters = s.starters.write().await;
    let starter = starters.get_mut(&id).ok_or((
        StatusCode::NOT_FOUND,
        Json(json!({"error":"starter not found"})),
    ))?;
    starter.yaml = input.yaml;
    starter.updated_at = Utc::now();
    starter.version = input.commit.chars().take(8).collect();
    s.bundles.write().await.insert(id.clone(), bundle);
    s.bundle_commits
        .write()
        .await
        .insert(id.clone(), input.commit.clone());
    drop(starters);
    persist_state(&s).await;
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
    let session = authenticated_session(&s, &headers).await?;
    let owner = s
        .starters
        .read()
        .await
        .get(&id)
        .map(|starter| starter.owner.clone())
        .ok_or(StatusCode::NOT_FOUND)?;
    choose_organization(&session, Some(&owner))?;
    let candidate = release_version(&input.version).map_err(|_| StatusCode::BAD_REQUEST)?;
    if s.versions
        .read()
        .await
        .get(&id)
        .into_iter()
        .flatten()
        .filter_map(|v| release_version(&v.version).ok())
        .any(|version| version >= candidate)
    {
        return Err(StatusCode::CONFLICT);
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
        .entry(id.clone())
        .or_default()
        .push(v.clone());
    persist_state(&s).await;
    if let Some(entry) = publish_to_briefcase(&s, &headers, &id, &v).await {
        s.briefcase_entries
            .write()
            .await
            .insert(format!("{id}:{}", v.commit), entry);
        persist_state(&s).await;
    }
    Ok(Json(v))
}

async fn publish_to_briefcase(
    s: &AppState,
    headers: &HeaderMap,
    id: &str,
    version: &Version,
) -> Option<Uuid> {
    let starter = s.starters.read().await.get(id)?.clone();
    if matches!(starter.visibility, Visibility::Private) {
        return None;
    }
    let session_id = session_id(headers)?;
    let session = s.auth.get(&session_id).await?;
    let Ok(storage) = briefcase::BriefcaseStorage::from_env(session.access_token) else {
        return None;
    };
    let bundle = s.bundles.read().await.get(id).cloned()?;
    let org = starter.owner.as_str();
    let base = format!("public/starters/{org}/{id}");
    let parent = storage
        .ensure_release_path(org, id, &version.version)
        .await
        .unwrap_or_else(|_| base.clone());
    let Ok(result) = storage
        .upload_public_bundle(&parent, "starter.git.bundle", bundle)
        .await
    else {
        return None;
    };
    if let Some(entry) = result
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        let _ = storage.set_public_link(entry).await;
        return Some(entry);
    }
    None
}
async fn commits(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let _ = show(State(s.clone()), headers, Path(id.clone())).await?;
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
    if show(State(s.clone()), headers, Path(id.clone()))
        .await
        .is_err()
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
    let session = authenticated_session(&s, &headers).await?;
    let _ = show(State(s.clone()), headers, Path(id.clone())).await?;
    if input.body.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let d = Discussion {
        id: Uuid::now_v7().to_string(),
        starter_id: id.clone(),
        parent_id: input.parent_id,
        author: session
            .actor
            .as_ref()
            .and_then(|actor| actor["public_id"].as_str())
            .unwrap_or("authenticated-user")
            .into(),
        body: input.body,
        created_at: Utc::now(),
    };
    s.discussions
        .write()
        .await
        .entry(id)
        .or_default()
        .push(d.clone());
    persist_state(&s).await;
    Ok((StatusCode::CREATED, Json(d)))
}
async fn auth_login() -> impl IntoResponse {
    let state = Uuid::new_v4().to_string();
    // IAM preserves redirect_uri's query, but does not forward an outer state parameter.
    let callback = format!("{}/auth/callback?state={state}", frontend_url());
    let url = format!(
        "https://auth.iam.teamofsilicons.com/login?app_id={}&redirect_uri={}",
        urlencoding::encode(&app_id()),
        urlencoding::encode(&callback)
    );
    (
        [(
            "set-cookie",
            format!(
                "starter_login_state={state}; HttpOnly; SameSite=Lax{}; Max-Age=600; Path=/",
                secure_cookie()
            ),
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
    match browser_login(&s, &headers, slt, q.get("state").map(String::as_str)).await {
        Ok(cookies) => (cookies, Redirect::to(&frontend_url())).into_response(),
        Err(response) => response.into_response(),
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
    match browser_login(&s, &headers, &body.slt, body.state.as_deref()).await {
        Ok(cookies) => (cookies, Json(json!({"authenticated":true}))).into_response(),
        Err(response) => response.into_response(),
    }
}
async fn browser_login(
    s: &AppState,
    headers: &HeaderMap,
    slt: &str,
    state: Option<&str>,
) -> Result<HeaderMap, (StatusCode, Json<serde_json::Value>)> {
    let expected = cookie_value(headers, "starter_login_state");
    if !auth::valid_login_state(expected, state) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"login state does not match the initiating browser"})),
        ));
    }
    let session = s
        .auth
        .login(slt, expected, state, &app_id(), &app_secret())
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, Json(json!({"error":error}))))?;
    let mut cookies = HeaderMap::new();
    cookies.append(
        "set-cookie",
        format!(
            "starter_session={session}; HttpOnly; SameSite=Lax{}; Path=/",
            secure_cookie()
        )
        .parse()
        .unwrap(),
    );
    cookies.append(
        "set-cookie",
        format!(
            "starter_login_state=; HttpOnly; SameSite=Lax{}; Max-Age=0; Path=/",
            secure_cookie()
        )
        .parse()
        .unwrap(),
    );
    cookies.insert("cache-control", HeaderValue::from_static("no-store"));
    Ok(cookies)
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
    let login = async {
        s.auth
            .insert(auth::exchange_slt(slt, &app_id(), &app_secret()).await?)
            .await
    };
    match login.await {
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
        .or_else(|| cookie_value(headers, "starter_session").map(str::to_owned))
}
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers
        .get_all("cookie")
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter_map(|(key, value)| (key == name).then_some(value));
    let value = values.next()?;
    values.next().is_none().then_some(value)
}
fn frontend_url() -> String {
    std::env::var("STARTER_FRONTEND_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3000".into())
        .trim_end_matches('/')
        .to_owned()
}
fn app_id() -> String {
    std::env::var("STARTER_IAM_APP_ID").unwrap_or_else(|_| "tos>starter".into())
}
fn secure_cookie() -> &'static str {
    if std::env::var("STARTER_FRONTEND_URL")
        .ok()
        .is_some_and(|v| v.starts_with("https://"))
    {
        "; Secure"
    } else {
        ""
    }
}
fn app_secret() -> String {
    std::env::var("STARTER_IAM_APP_SECRET").unwrap_or_default()
}
async fn iam_webhook(
    headers: HeaderMap,
    State(s): State<AppState>,
    body: axum::body::Bytes,
) -> StatusCode {
    let Some(secret) = std::env::var_os("STARTER_IAM_WEBHOOK_SECRET") else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    if !auth::verify_webhook_now(&headers, &body, secret.to_string_lossy().as_bytes()) {
        return StatusCode::UNAUTHORIZED;
    }
    let Some(event_id) = auth::webhook_event_id(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let inserted = {
        let mut ids = s.iam_event_ids.write().await;
        if ids.contains(&event_id) {
            false
        } else {
            if ids.len() >= 4096 {
                // Keep memory bounded; snapshots preserve the retained IDs across restarts.
                if let Some(oldest) = ids.iter().next().cloned() {
                    ids.remove(&oldest);
                }
            }
            ids.insert(event_id)
        }
    };
    if inserted {
        persist_state(&s).await;
    }
    StatusCode::NO_CONTENT
}
pub async fn run(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = seeded_state();
    if let Some(store) = store::Store::connect_from_env().await? {
        let store = Arc::new(store);
        if let Some(payload) = store.load().await? {
            restore_state(&state, payload).await?;
        }
        state.store = Some(store);
    }
    state.auth.load().await?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn restore_state(
    s: &AppState,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let object = payload.as_object().ok_or("invalid starter snapshot")?;
    if let Some(value) = object.get("starters") {
        *s.starters.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("versions") {
        *s.versions.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("discussions") {
        *s.discussions.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("bundle_commits") {
        *s.bundle_commits.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("iam_event_ids") {
        *s.iam_event_ids.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("briefcase_entries") {
        *s.briefcase_entries.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("bundles") {
        let map: HashMap<String, String> = serde_json::from_value(value.clone())?;
        let decoded = map
            .into_iter()
            .map(|(key, value)| BASE64.decode(value).map(|bytes| (key, bytes)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        *s.bundles.write().await = decoded;
    }
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

#[cfg(test)]
mod auth_and_organization_tests {
    use super::*;

    async fn session(s: &AppState, orgs: &[&str]) -> HeaderMap {
        let id = s
            .auth
            .insert(auth::IamTokens {
                access_token: "test-access".into(),
                refresh_token: "test-refresh".into(),
                expires_in: 1800,
                expires_at: Some(Utc::now().timestamp() + 1800),
                actor: Some(json!({"public_id":"test-user"})),
                org_id: None,
                org_ids: orgs.iter().map(|org| (*org).into()).collect(),
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-starter-session", id.parse().unwrap());
        headers
    }

    fn input(id: &str) -> CreateStarter {
        CreateStarter {
            id: id.into(),
            org_id: None,
            name: "Example".into(),
            description: String::new(),
            visibility: Visibility::Private,
            tags: Vec::new(),
            yaml: silicon_starter_core::SEED_YAML.into(),
        }
    }

    #[tokio::test]
    async fn organization_creation_and_private_access_are_enforced() {
        let s = AppState::default();
        assert!(s.starters.read().await.is_empty());
        assert_eq!(
            create(
                State(s.clone()),
                HeaderMap::new(),
                Json(input("tos.example"))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::UNAUTHORIZED
        );
        let unscoped = session(&s, &[]).await;
        assert_eq!(
            create(
                State(s.clone()),
                unscoped.clone(),
                Json(input("tos.example"))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        let owner = session(&s, &["tos"]).await;
        assert_eq!(
            create(State(s.clone()), owner.clone(), Json(input("lab.example")))
                .await
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            create(State(s.clone()), owner.clone(), Json(input("tos.example")))
                .await
                .unwrap()
                .0,
            StatusCode::CREATED
        );
        assert_eq!(
            create(State(s.clone()), owner.clone(), Json(input("tos.example")))
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        let outsider = session(&s, &["lab"]).await;
        for headers in [HeaderMap::new(), unscoped, outsider.clone()] {
            assert_eq!(
                show(
                    State(s.clone()),
                    headers.clone(),
                    Path("tos.example".into())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                versions(
                    State(s.clone()),
                    headers.clone(),
                    Path("tos.example".into())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                commits(
                    State(s.clone()),
                    headers.clone(),
                    Path("tos.example".into())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            let Json(items) = list(
                State(s.clone()),
                headers,
                Query(ListQuery {
                    q: None,
                    org: None,
                    visibility: None,
                }),
            )
            .await;
            assert!(items.is_empty());
        }
        assert!(
            show(State(s.clone()), owner.clone(), Path("tos.example".into()))
                .await
                .is_ok()
        );
        assert!(
            versions(State(s.clone()), owner, Path("tos.example".into()))
                .await
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            push(
                State(s.clone()),
                outsider.clone(),
                Path("tos.example".into()),
                Json(PushRequest {
                    commit: "a".repeat(40),
                    bundle_base64: String::new(),
                    yaml: silicon_starter_core::SEED_YAML.into(),
                    message: String::new(),
                })
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            publish(
                State(s.clone()),
                outsider,
                Path("tos.example".into()),
                Json(PublishRequest {
                    selector: "main".into(),
                    version: "1.0".into(),
                    commit: None,
                    notes: String::new(),
                })
            )
            .await
            .unwrap_err(),
            StatusCode::FORBIDDEN
        );
        let multi = session(&s, &["tos", "lab"]).await;
        assert_eq!(
            create(
                State(s.clone()),
                multi.clone(),
                Json(input("unselected.example"))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            create(State(s.clone()), multi.clone(), Json(input("lab.inferred")))
                .await
                .unwrap()
                .1
                .0
                .owner,
            "lab"
        );
        let mut selected = input("lab.example");
        selected.org_id = Some("lab".into());
        assert_eq!(
            create(State(s.clone()), multi, Json(selected))
                .await
                .unwrap()
                .1
                .0
                .owner,
            "lab"
        );
        assert!(restore_state(&s, json!({"starters":[]})).await.is_err());
        assert!(
            restore_state(&s, json!({"bundles":{"x":"invalid base64"}}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn browser_callbacks_reject_missing_wrong_and_duplicate_state() {
        let response = auth_login().await.into_response();
        let location =
            reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let redirect = location
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let redirect = reqwest::Url::parse(&redirect).unwrap();
        let state = redirect
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        assert!(
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains(&format!("starter_login_state={state};"))
        );
        for (cookie, query_state) in [
            (None, None),
            (Some("starter_login_state=expected"), None),
            (None, Some("expected")),
            (Some("starter_login_state=expected"), Some("wrong")),
            (
                Some("starter_login_state=expected-prefix"),
                Some("expected"),
            ),
            (
                Some("starter_login_state=expected; starter_login_state=expected"),
                Some("expected"),
            ),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(cookie) = cookie {
                headers.insert("cookie", cookie.parse().unwrap());
            }
            let mut query = HashMap::from([("slt".into(), "oac_test".into())]);
            if let Some(state) = query_state {
                query.insert("state".into(), state.into());
            }
            let response = auth_callback(headers.clone(), Query(query), State(AppState::default()))
                .await
                .into_response();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let response = auth_callback_json(
                headers,
                State(AppState::default()),
                Json(AuthCallbackBody {
                    slt: "oac_test".into(),
                    state: query_state.map(str::to_owned),
                }),
            )
            .await
            .into_response();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
