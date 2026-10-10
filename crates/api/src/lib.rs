use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::IntoResponse,
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
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

mod accounts;
mod auth;
mod auth_routes;
use auth_routes::*;
mod authority;
mod blocks;
mod briefcase;
mod durable;
mod feature_routes;
mod files;
mod semantic;
mod store;
mod telemetry;

#[derive(Clone, Default)]
pub struct AppState {
    pub blocks: Arc<RwLock<HashMap<String, blocks::StoredBlock>>>,
    pub block_publish_lock: Arc<Mutex<()>>,
    pub persist_lock: Arc<Mutex<()>>,
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
    pub account_event_ids: Arc<RwLock<HashSet<String>>>,
    pub account_profile_updated_at: Arc<RwLock<HashMap<String, chrono::DateTime<Utc>>>>,
    pub briefcase_entries: Arc<RwLock<HashMap<String, Uuid>>>,
    pub(crate) feature_store: Arc<tokio::sync::OnceCell<durable::FeatureStore>>,
    pub(crate) publication_lock: Arc<tokio::sync::Mutex<()>>,
}
#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    owner: Option<String>,
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
#[derive(Clone, Deserialize, serde::Serialize)]
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
        .route("/api/v1/starters", get(list).post(create))
        .route("/api/v1/search", get(search))
        .route("/api/v1/blocks", get(blocks::list).post(blocks::publish))
        .route("/api/v1/blocks/{id}", get(blocks::show))
        .route("/api/v1/blocks/{id}/versions", get(blocks::versions))
        .route("/api/v1/blocks/{id}/content", get(blocks::content))
        .route("/api/v1/blocks/{id}/download", get(blocks::download))
        .layer(axum::extract::DefaultBodyLimit::max(12 * 1024 * 1024))
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
        .route("/auth/attempt", post(auth_attempt))
        .route(
            "/auth/callback",
            get(auth_callback).post(auth_callback_json),
        )
        .route("/auth/cli", post(auth_cli))
        .route("/auth/cli/start", post(auth_cli_start))
        .route("/auth/cli/complete", post(auth_cli_complete))
        .route("/auth/cli/status", get(auth_cli_status))
        .route("/auth/session", get(auth_session))
        .route("/auth/contexts", get(auth_contexts))
        .route("/auth/context", post(auth_context))
        .route("/auth/logout", post(auth_logout))
        .route("/webhooks/accounts", post(accounts_webhook))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_routes::context_guard,
        ))
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
                    HeaderName::from_static("x-starter-context"),
                    HeaderName::from_static("idempotency-key"),
                    HeaderName::from_static("x-starter-mode"),
                    HeaderName::from_static("x-accounts-signature"),
                    HeaderName::from_static("x-accounts-timestamp"),
                ]),
        )
}
async fn persist_state(s: &AppState) {
    if let Err(error) = persist_state_checked(s).await {
        eprintln!("starter state persistence failed: {error}");
    }
}
async fn persist_state_checked(s: &AppState) -> Result<(), String> {
    let _snapshot = s.persist_lock.lock().await;
    save_state_unlocked(s).await
}
async fn save_state_unlocked(s: &AppState) -> Result<(), String> {
    let Some(store) = &s.store else { return Ok(()) };
    let value = json!({
        "blocks": *s.blocks.read().await,
        "starters": *s.starters.read().await,
        "versions": *s.versions.read().await,
        "discussions": *s.discussions.read().await,
        "bundles": s.bundles.read().await.iter().map(|(k,v)| (k.clone(), BASE64.encode(v))).collect::<HashMap<_,_>>(),
        "bundle_commits": *s.bundle_commits.read().await,
        "account_event_ids": *s.account_event_ids.read().await,
        "account_profile_updated_at": *s.account_profile_updated_at.read().await,
        "briefcase_entries": *s.briefcase_entries.read().await,
    });
    store.save(value).await.map_err(|_| {
        "Starter catalog persistence failed; retry the original publication".to_owned()
    })
}
async fn health() -> Json<serde_json::Value> {
    Json(
        json!({"status":"ok","service":"silicon-starter","api_version":"v1","version":env!("CARGO_PKG_VERSION"),"accounts_url":"https://accounts.teamofsilicons.com"}),
    )
}
async fn authenticated_session(
    s: &AppState,
    headers: &HeaderMap,
) -> Result<auth::SessionTokens, StatusCode> {
    let id = session_id(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let tokens = s
        .auth
        .get(&id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if headers
        .get("x-starter-context")
        .and_then(|v| v.to_str().ok())
        != Some(tokens.context_id.as_str())
    {
        return Err(StatusCode::CONFLICT);
    }
    Ok(tokens)
}
fn require_owner(session: &auth::SessionTokens, owner_uuid: &str) -> Result<(), StatusCode> {
    if !owner_uuid.is_empty() && session.account_uuid() == owner_uuid {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Json<Vec<Starter>> {
    let term = q.q.unwrap_or_default().to_lowercase();
    let account = authenticated_session(&s, &headers)
        .await
        .map(|session| session.account_uuid().to_owned())
        .unwrap_or_default();
    Json(
        s.starters
            .read()
            .await
            .values()
            .filter(|x| {
                matches!(x.visibility, Visibility::Public)
                    || !x.owner_uuid.is_empty() && account == x.owner_uuid
            })
            .filter(|x| q.owner.as_ref().is_none_or(|o| &x.owner == o))
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
) -> Json<Vec<serde_json::Value>> {
    // Build the authorized candidate set before ranking; semantic matches need not contain the query.
    let Json(starters) = list(
        State(s.clone()),
        headers.clone(),
        Query(ListQuery {
            q: None,
            owner: None,
            visibility: None,
        }),
    )
    .await;
    let mut items: Vec<_> = starters
        .into_iter()
        .map(|item| {
            let text = format!(
                "{} {} {} {} {}",
                item.id,
                item.name,
                item.description,
                item.tags.join(" "),
                item.yaml
            );
            (json!(item), text, None)
        })
        .collect();
    items.extend(blocks::search_items(&s, &headers).await);
    let query = q.q.unwrap_or_default();
    let term = query.trim().to_lowercase();
    if term.is_empty() {
        return Json(items.into_iter().map(|(item, _, _)| item).collect());
    }
    let query_embedding = if std::env::var_os("GEMINI_API_KEY").is_some() {
        semantic::embed(&query, true).await.ok()
    } else {
        None
    };
    let mut scored = Vec::new();
    for (item, text, cached) in items {
        let lexical = text.to_lowercase().contains(&term);
        let score = if let Some(query_embedding) = &query_embedding {
            let embedding = match cached {
                Some(embedding) => Some(embedding),
                None => semantic::embed(&text, false).await.ok(),
            };
            embedding.map(|embedding| semantic::cosine(query_embedding, &embedding))
        } else {
            None
        };
        if let Some(score) = score {
            scored.push((score, item));
        } else if lexical {
            scored.push((0.0, item));
        }
    }
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1["id"].as_str().cmp(&b.1["id"].as_str()))
    });
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
            .is_ok_and(|session| require_owner(&session, &x.owner_uuid).is_ok())
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
    let owner = session.actor_id().to_owned();
    let handle = owner.split_once(':').map(|(_, handle)| handle).ok_or((
        StatusCode::FORBIDDEN,
        Json(json!({"error":"invalid account"})),
    ))?;
    if !valid_id(&input.id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid starter id"})),
        ));
    }
    if !input
        .id
        .strip_prefix(&format!("{handle}."))
        .is_some_and(|name| !name.is_empty())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":format!("starter id must begin with {handle}. followed by a name")}),
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
        owner_uuid: session.account_uuid().into(),
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
) -> Result<(StatusCode, Json<Starter>), StatusCode> {
    let session = authenticated_session(&s, &headers).await?;
    let owner = session.actor_id().to_owned();
    let handle = owner
        .split_once(':')
        .map(|(_, handle)| handle)
        .ok_or(StatusCode::FORBIDDEN)?;
    let Json(src) = show(State(s.clone()), headers, Path(id.clone())).await?;
    let mut m = s.starters.write().await;
    let fork = Starter {
        id: format!("{handle}.fork-{}", Uuid::new_v4().simple()),
        name: format!("{} (fork)", src.name),
        owner,
        owner_uuid: session.account_uuid().into(),
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
    if matches!(
        headers.get("x-starter-mode").and_then(|v| v.to_str().ok()),
        Some("pull" | "dev")
    ) || q
        .get("mode")
        .is_some_and(|mode| mode == "pull" || mode == "dev")
    {
        authenticated_session(&s, &headers).await?;
    }
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
    if !silicon_starter_core::local::is_hex_commit(&commit) {
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
        .map(|starter| starter.owner_uuid.clone())
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"starter not found"})),
        ))?;
    require_owner(&session, &owner).map_err(|status| {
        (
            status,
            Json(json!({"error":"only the owning account can push this starter"})),
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
) -> authority::Result<Json<Version>> {
    let session = authenticated_session(&s, &headers).await?;
    let starter = s
        .starters
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    require_owner(&session, &starter.owner_uuid)?;
    let key = feature_routes::key(&headers)?;
    let _publication = s.publication_lock.lock().await;
    if !matches!(starter.visibility, Visibility::Private) {
        return briefcase::publish(&s, &session, &id, &input, key)
            .await
            .map(Json);
    }
    // Private registry releases do not require a public Briefcase feature.
    let candidate = release_version(&input.version).map_err(|_| StatusCode::BAD_REQUEST)?;
    let commit = input.commit.clone().unwrap_or(input.selector.clone());
    let v = {
        let mut saved = s.versions.write().await;
        let versions = saved.entry(id.clone()).or_default();
        if let Some(existing) = versions
            .iter()
            .find(|v| v.version == input.version && v.commit == commit && v.notes == input.notes)
        {
            return Ok(Json(existing.clone()));
        }
        if versions
            .iter()
            .filter_map(|v| release_version(&v.version).ok())
            .any(|v| v >= candidate)
        {
            return Err(authority::Error::conflict(
                "This release version is already published or superseded",
            ));
        }
        let v = Version {
            version: input.version,
            commit,
            notes: input.notes,
            published_at: Utc::now(),
        };
        versions.push(v.clone());
        v
    };
    persist_state_checked(&s)
        .await
        .map_err(authority::Error::unavailable)?;
    Ok(Json(v))
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
        author: session.actor_id().into(),
        author_uuid: session.account_uuid().into(),
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
    std::env::var("STARTER_ACCOUNTS_APP_ID").unwrap_or_else(|_| "starter".into())
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
async fn accounts_webhook(
    headers: HeaderMap,
    State(s): State<AppState>,
    body: axum::body::Bytes,
) -> StatusCode {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let Ok(secret) = std::env::var("STARTER_ACCOUNTS_WEBHOOK_SECRET") else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let header = |name: &str| {
        let mut values = headers.get_all(name).iter();
        let value = values.next()?.to_str().ok()?;
        if values.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    let (Some(timestamp), Some(signature)) = (
        header("x-accounts-timestamp"),
        header("x-accounts-signature"),
    ) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Ok(ts) = timestamp.parse::<i64>() else {
        return StatusCode::UNAUTHORIZED;
    };
    if Utc::now().timestamp().abs_diff(ts) > 300 {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(&body);
    if !signature
        .split(',')
        .filter_map(|part| part.trim().strip_prefix("v1="))
        .filter_map(|sig| hex::decode(sig).ok())
        .any(|sig| mac.clone().verify_slice(&sig).is_ok())
    {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(event) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if event["app_id"].as_str() != Some(app_id().as_str()) {
        return StatusCode::FORBIDDEN;
    }
    let Some(event_id) = event["event_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
    else {
        return StatusCode::BAD_REQUEST;
    };
    let _snapshot = s.persist_lock.lock().await;
    let mut ids = s.account_event_ids.write().await;
    if ids.contains(event_id) {
        return StatusCode::NO_CONTENT;
    }
    let data = &event["data"];
    // Session access is introspected on every request; delayed revocation notices cannot end a newer login.
    if matches!(
        event["type"].as_str(),
        Some("account.id_changed" | "account.updated")
    ) {
        let Some(account_uuid) = data["uuid"].as_str().filter(|id| {
            Uuid::parse_str(id).is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == *id)
        }) else {
            return StatusCode::BAD_REQUEST;
        };
        let Some(occurred_at) = event["occurred_at"]
            .as_str()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|time| time.with_timezone(&Utc))
        else {
            return StatusCode::BAD_REQUEST;
        };
        let public_id = data["new_id"]
            .as_str()
            .or_else(|| data["account"]["id"].as_str());
        if let Some(public_id) = public_id.filter(|id| valid_account_id(id)) {
            let mut updated = s.account_profile_updated_at.write().await;
            if updated
                .get(account_uuid)
                .is_none_or(|last| occurred_at > *last)
            {
                for starter in s
                    .starters
                    .write()
                    .await
                    .values_mut()
                    .filter(|x| x.owner_uuid == account_uuid)
                {
                    starter.owner = public_id.into();
                }
                for entry in s
                    .blocks
                    .write()
                    .await
                    .values_mut()
                    .filter(|x| x.block.owner_uuid == account_uuid)
                {
                    entry.block.owner = public_id.into();
                }
                for discussion in s
                    .discussions
                    .write()
                    .await
                    .values_mut()
                    .flatten()
                    .filter(|x| x.author_uuid == account_uuid)
                {
                    discussion.author = public_id.into();
                }
                updated.insert(account_uuid.into(), occurred_at);
            }
        }
    }
    if ids.len() >= 4096
        && let Some(old) = ids.iter().next().cloned()
    {
        ids.remove(&old);
    }
    ids.insert(event_id.into());
    drop(ids);
    if save_state_unlocked(&s).await.is_err() {
        s.account_event_ids.write().await.remove(event_id);
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    StatusCode::NO_CONTENT
}

pub async fn run(bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    auth::validate_app_id(&app_id())?;
    if let Ok(id) = std::env::var("BRIEFCASE_APP_ID") {
        auth::validate_app_id(&id)?;
    }
    let mut state = seeded_state();
    state.auth.load().await?;
    if let Some(store) = store::Store::connect_from_env().await? {
        let store = Arc::new(store);
        if let Some(payload) = store.load().await? {
            restore_state(&state, payload).await?;
        }
        state.store = Some(store);
        persist_state_checked(&state).await?;
    }
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

fn valid_account_id(id: &str) -> bool {
    id.strip_prefix("c:")
        .or_else(|| id.strip_prefix("si:"))
        .is_some_and(|handle| {
            (3..=30).contains(&handle.len())
                && handle
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        })
}

async fn restore_state(
    s: &AppState,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = payload;
    if !payload.is_object() {
        return Err("invalid starter snapshot".into());
    }
    // Explicit operator mapping only: never infer ownership from a mutable account handle.
    let mapping: HashMap<String, serde_json::Value> = serde_json::from_str(
        &std::env::var("STARTER_OWNER_MIGRATION").unwrap_or_else(|_| "{}".into()),
    )?;
    for (old, account) in &mapping {
        let uuid = account["uuid"]
            .as_str()
            .ok_or("owner mapping requires uuid")?;
        let parsed = Uuid::parse_str(uuid)?;
        if parsed.is_nil() || parsed.to_string() != uuid {
            return Err("owner mapping requires a canonical nonempty account UUID".into());
        }
        let id = account["id"]
            .as_str()
            .filter(|id| valid_account_id(id))
            .ok_or("owner mapping requires a canonical Carbon or Silicon id")?;
        if let Some(entries) = payload["starters"].as_object_mut() {
            for item in entries.values_mut().filter(|item| {
                item["owner"] == *old && item["owner_uuid"].as_str().is_none_or(str::is_empty)
            }) {
                item["owner_uuid"] = json!(uuid);
                item["owner"] = json!(id);
            }
        }
        if let Some(entries) = payload["blocks"].as_object_mut() {
            for entry in entries.values_mut() {
                let item = &mut entry["block"];
                if item["owner"] == *old && item["owner_uuid"].as_str().is_none_or(str::is_empty) {
                    item["owner_uuid"] = json!(uuid);
                    item["owner"] = json!(id);
                }
            }
        }
    }
    let object = payload.as_object().ok_or("invalid starter snapshot")?;
    if let Some(value) = object.get("blocks") {
        *s.blocks.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("starters") {
        *s.starters.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("versions") {
        *s.versions.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("discussions") {
        let discussions: HashMap<String, Vec<Discussion>> = serde_json::from_value(value.clone())?;
        *s.discussions.write().await = discussions;
    }
    if let Some(value) = object.get("bundle_commits") {
        *s.bundle_commits.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("account_event_ids") {
        *s.account_event_ids.write().await = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = object.get("account_profile_updated_at") {
        *s.account_profile_updated_at.write().await = serde_json::from_value(value.clone())?;
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
