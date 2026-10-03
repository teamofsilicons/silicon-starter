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

mod auth;
mod auth_routes;
mod iam;
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
    pub(crate) world: Arc<iam::World>,
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
    pub iam_event_ids: Arc<RwLock<HashSet<String>>>,
    pub briefcase_entries: Arc<RwLock<HashMap<String, Uuid>>>,
    pub(crate) feature_store: Arc<tokio::sync::OnceCell<durable::FeatureStore>>,
    pub(crate) publication_lock: Arc<tokio::sync::Mutex<()>>,
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
        .route(
            "/api/v1/briefcase/authorization",
            post(feature_routes::start),
        )
        .route(
            "/api/v1/briefcase/authorizations/{id}",
            get(feature_routes::status),
        )
        .route(
            "/api/v1/briefcase/authorizations/{id}/complete",
            post(feature_routes::complete),
        )
        .route("/api/v1/organizations", get(organizations))
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
        .route(
            "/auth/callback",
            get(auth_callback).post(auth_callback_json),
        )
        .route("/auth/cli", post(auth_cli))
        .route("/auth/cli/status", get(auth_cli_status))
        .route("/auth/session", get(auth_session))
        .route("/auth/contexts", get(auth_contexts))
        .route("/auth/context", post(auth_context))
        .route("/auth/logout", post(auth_logout))
        .route("/webhooks/iam", post(iam_webhook))
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
                    HeaderName::from_static("x-silicon-iam-signature"),
                    HeaderName::from_static("x-silicon-iam-timestamp"),
                    HeaderName::from_static("x-silicon-iam-key-version"),
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
    if s.world.environment_id.is_some() && s.auth.iam().await?.world != *s.world {
        return Err("Testing catalog belongs to an earlier world generation".into());
    }
    let Some(store) = &s.store else { return Ok(()) };
    let value = json!({
        "world": *s.world,
        "blocks": *s.blocks.read().await,
        "starters": *s.starters.read().await,
        "versions": *s.versions.read().await,
        "discussions": *s.discussions.read().await,
        "bundles": s.bundles.read().await.iter().map(|(k,v)| (k.clone(), BASE64.encode(v))).collect::<HashMap<_,_>>(),
        "bundle_commits": *s.bundle_commits.read().await,
        "iam_event_ids": *s.iam_event_ids.read().await,
        "briefcase_entries": *s.briefcase_entries.read().await,
    });
    store.save(value).await.map_err(|_| {
        "Starter catalog persistence failed; retry the original publication".to_owned()
    })
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status":"ok","service":"silicon-starter","api_version":"v1"}))
}
async fn authenticated_session(
    s: &AppState,
    headers: &HeaderMap,
) -> Result<auth::IamTokens, StatusCode> {
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
) -> Json<Vec<serde_json::Value>> {
    // Build the authorized candidate set before ranking; semantic matches need not contain the query.
    let Json(starters) = list(
        State(s.clone()),
        headers.clone(),
        Query(ListQuery {
            q: None,
            org: None,
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
) -> authority::Result<Json<Version>> {
    let session = authenticated_session(&s, &headers).await?;
    let starter = s
        .starters
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    choose_organization(&session, Some(&starter.owner))?;
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
    std::env::var("STARTER_IAM_APP_ID").unwrap_or_else(|_| "starter".into())
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
    auth::validate_app_id(&app_id())?;
    if let Ok(id) = std::env::var("BRIEFCASE_APP_ID") {
        auth::validate_app_id(&id)?;
    }
    let mut state = seeded_state();
    state.auth.load().await?;
    if [
        "STARTER_IAM_APP_SECRET",
        "STARTER_IAM_TEST_APP_SECRET",
        "STARTER_TESTING_ENVIRONMENT_KEY",
    ]
    .iter()
    .any(|key| std::env::var_os(key).is_some())
    {
        state.world = Arc::new(state.auth.iam().await?.world);
    }
    if let Some(store) = store::Store::connect_from_env(&state.world).await? {
        let store = Arc::new(store);
        if let Some(payload) = store.load().await? {
            restore_state(&state, payload).await?;
        }
        state.store = Some(store);
    }
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

async fn restore_state(
    s: &AppState,
    payload: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let object = payload.as_object().ok_or("invalid starter snapshot")?;
    let saved_world: iam::World = match object.get("world") {
        Some(world) => serde_json::from_value(world.clone())?,
        None if s.world.environment_id.is_none() => iam::World::default(),
        None => return Err("Testing catalog snapshot omitted its world binding".into()),
    };
    if saved_world != *s.world {
        return Err("Catalog snapshot belongs to another world or testing generation".into());
    }
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
        if discussions.values().flatten().any(|discussion| {
            discussion.author != "authenticated-user"
                && !auth::valid_actor_id("carbon", &discussion.author)
                && !auth::valid_actor_id("silicon", &discussion.author)
        }) {
            return Err("legacy discussion authors require the offline IAM mapping migration; see docs/PUBLIC-IDENTIFIER-MIGRATION.md".into());
        }
        *s.discussions.write().await = discussions;
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

    #[tokio::test]
    async fn restored_discussions_require_canonical_authors_and_preserve_content() {
        let s = AppState::default();
        for author in [
            "alice",
            "assistant:tos",
            "c:alice",
            "si:assistant",
            "authenticated-user",
        ] {
            let discussion = json!({
                "id": "unchanged-id", "starter_id": "tos.example", "parent_id": null,
                "author": author, "body": "Keep assistant:tos and tos>starter in historical text.",
                "created_at": "2026-09-23T00:00:00Z"
            });
            let result = restore_state(
                &s,
                json!({"discussions": {"tos.example": [discussion.clone()]}}),
            )
            .await;
            if ["alice", "assistant:tos"].contains(&author) {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("offline IAM mapping")
                );
            } else {
                result.unwrap();
                assert_eq!(
                    serde_json::to_value(&s.discussions.read().await["tos.example"][0]).unwrap(),
                    discussion
                );
            }
        }
    }

    pub(super) async fn session(s: &AppState, orgs: &[&str]) -> HeaderMap {
        let id = s
            .auth
            .insert(auth::IamTokens {
                access_token: "test-access".into(),
                refresh_token: "test-refresh".into(),
                expires_in: 1800,
                expires_at: Some(Utc::now().timestamp() + 1800),
                actor: Some(json!({"type":"carbon","public_id":"c:test-user"})),
                org_id: (orgs.len() == 1).then(|| orgs[0].to_string()),
                org_ids: orgs.iter().map(|org| (*org).into()).collect(),
                context_id: "fixture-context".into(),
                world: iam::World::default(),
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-starter-session", id.parse().unwrap());
        headers.insert("x-starter-context", "fixture-context".parse().unwrap());
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
    async fn public_pulls_require_a_valid_session_but_downloads_do_not() {
        let s = AppState::default();
        let owner = session(&s, &["tos"]).await;
        let _ = create(
            State(s.clone()),
            owner.clone(),
            Json(CreateStarter {
                visibility: Visibility::Public,
                ..input("tos.public")
            }),
        )
        .await
        .unwrap();
        s.bundles
            .write()
            .await
            .insert("tos.public".into(), b"bundle".to_vec());
        s.bundle_commits
            .write()
            .await
            .insert("tos.public".into(), "a".repeat(40));
        for mode in ["pull", "dev"] {
            let q = HashMap::from([("mode".into(), mode.into())]);
            assert_eq!(
                archive(
                    State(s.clone()),
                    HeaderMap::new(),
                    Path("tos.public".into()),
                    Query(q.clone())
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
            assert!(
                archive(
                    State(s.clone()),
                    owner.clone(),
                    Path("tos.public".into()),
                    Query(q)
                )
                .await
                .is_ok()
            );
            let mut headers = HeaderMap::new();
            headers.insert("x-starter-mode", mode.parse().unwrap());
            assert_eq!(
                archive(
                    State(s.clone()),
                    headers,
                    Path("tos.public".into()),
                    Query(HashMap::new())
                )
                .await
                .unwrap_err(),
                StatusCode::UNAUTHORIZED
            );
        }
        for reference in ["9.9", "abcde", ""] {
            assert_eq!(
                archive(
                    State(s.clone()),
                    HeaderMap::new(),
                    Path("tos.public".into()),
                    Query(HashMap::from([("ref".into(), reference.into())]))
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
        assert!(
            archive(
                State(s),
                HeaderMap::new(),
                Path("tos.public".into()),
                Query(HashMap::new())
            )
            .await
            .is_ok()
        );
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
            StatusCode::UNAUTHORIZED
        );
        let owner = session(&s, &["tos"]).await;
        for id in ["tos.Upper", "tos.under_score", "tos.extra.dot"] {
            assert_eq!(
                create(State(s.clone()), owner.clone(), Json(input(id)))
                    .await
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
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
            .unwrap_err()
            .status,
            StatusCode::FORBIDDEN
        );
        let multi = session(&s, &["tos", "lab"]).await;
        for id in ["unselected.example", "lab.inferred"] {
            assert_eq!(
                create(State(s.clone()), multi.clone(), Json(input(id)))
                    .await
                    .unwrap_err()
                    .0,
                StatusCode::UNAUTHORIZED
            );
        }
        let selected = session(&s, &["lab"]).await;
        assert_eq!(
            create(State(s.clone()), selected, Json(input("lab.example")))
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
