//! Immutable, content-addressed interpreter blocks stored alongside starter metadata.
use super::*;
use serde::{Deserialize, Serialize};
use silicon_starter_core::blocks::{self as core, Block, BlockKind, BlockVersion};

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredBlock {
    pub block: Block,
    pub versions: Vec<StoredVersion>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredVersion {
    #[serde(flatten)]
    pub version: BlockVersion,
    pub content_base64: String,
    pub search_text: String,
    pub embedding: Option<Vec<f32>>,
    pub briefcase_entry: Option<Uuid>,
}
#[derive(Deserialize, Serialize)]
pub struct PublishBlock {
    id: String,
    name: Option<String>,
    description: Option<String>,
    visibility: Option<Visibility>,
    text: Option<String>,
    archive_base64: Option<String>,
}
type ApiError = (StatusCode, Json<serde_json::Value>);
fn error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (status, Json(json!({"error":message.into()})))
}

pub async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Json<Vec<Block>> {
    let account = authenticated_session(&s, &headers)
        .await
        .map(|session| session.account_uuid().to_owned())
        .unwrap_or_default();
    let term = q.q.unwrap_or_default().to_lowercase();
    let mut items: Vec<_> = s
        .blocks
        .read()
        .await
        .values()
        .filter(|entry| {
            matches!(entry.block.visibility, Visibility::Public)
                || !entry.block.owner_uuid.is_empty() && account == entry.block.owner_uuid
        })
        .filter(|entry| {
            q.owner
                .as_ref()
                .is_none_or(|owner| owner == &entry.block.owner)
        })
        .filter(|entry| {
            q.visibility.as_ref().is_none_or(|v| {
                std::mem::discriminant(v) == std::mem::discriminant(&entry.block.visibility)
            })
        })
        .filter(|entry| term.is_empty() || searchable(entry).to_lowercase().contains(&term))
        .map(|entry| entry.block.clone())
        .collect();
    items.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Json(items)
}
fn searchable(entry: &StoredBlock) -> String {
    let body = entry
        .versions
        .iter()
        .find(|v| v.version.version == entry.block.version)
        .map(|v| v.search_text.as_str())
        .unwrap_or_default();
    format!(
        "{} {} {} {}",
        entry.block.id, entry.block.name, entry.block.description, body
    )
}

pub async fn search_items(
    s: &AppState,
    headers: &HeaderMap,
) -> Vec<(serde_json::Value, String, Option<Vec<f32>>)> {
    let Json(visible) = list(
        State(s.clone()),
        headers.clone(),
        Query(ListQuery {
            q: None,
            owner: None,
            visibility: None,
        }),
    )
    .await;
    let entries = s.blocks.read().await;
    visible
        .into_iter()
        .filter_map(|block| {
            let entry = entries.get(&block.id)?;
            let embedding = entry
                .versions
                .iter()
                .find(|v| v.version.version == block.version)
                .and_then(|v| v.embedding.clone());
            Some((json!(block), searchable(entry), embedding))
        })
        .collect()
}

async fn readable(s: &AppState, headers: &HeaderMap, id: &str) -> Result<StoredBlock, StatusCode> {
    core::parse_id(id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let entry = s
        .blocks
        .read()
        .await
        .get(id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    if matches!(entry.block.visibility, Visibility::Private)
        && !authenticated_session(s, headers)
            .await
            .is_ok_and(|session| require_owner(&session, &entry.block.owner_uuid).is_ok())
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(entry)
}
pub async fn show(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Block>, StatusCode> {
    Ok(Json(readable(&s, &headers, &id).await?.block))
}
pub async fn versions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<BlockVersion>>, StatusCode> {
    Ok(Json(
        readable(&s, &headers, &id)
            .await?
            .versions
            .into_iter()
            .rev()
            .map(|v| v.version)
            .collect(),
    ))
}
async fn payload(
    s: &AppState,
    headers: &HeaderMap,
    reference: &str,
    query: &HashMap<String, String>,
) -> Result<(String, serde_json::Value), StatusCode> {
    let (id, suffix) = reference
        .split_once('@')
        .map_or((reference, None), |(id, version)| (id, Some(version)));
    let entry = readable(s, headers, id).await?;
    let requested = query
        .get("version")
        .or_else(|| query.get("ref"))
        .map(String::as_str)
        .or(suffix)
        .unwrap_or("latest");
    let version = if requested == "latest" {
        &entry.block.version
    } else {
        requested
    };
    let stored = entry
        .versions
        .iter()
        .find(|v| v.version.version == version)
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut value = json!({"id":id,"kind":entry.block.kind,"version":stored.version.version});
    if entry.block.kind == BlockKind::Gene {
        let bytes = BASE64
            .decode(&stored.content_base64)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        value["text"] =
            json!(String::from_utf8(bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    } else {
        value["archive_base64"] = json!(stored.content_base64);
    }
    Ok((id.into(), value))
}
pub async fn content(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    payload(&s, &headers, &id, &q)
        .await
        .map(|(_, value)| Json(value))
}
pub async fn download(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let (id, value) = payload(&s, &headers, &id, &q).await?;
    if let Some(entry) = s.blocks.write().await.get_mut(&id) {
        entry.block.downloads += 1;
    }
    persist_state(&s).await;
    Ok(Json(value))
}

pub async fn publish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PublishBlock>,
) -> Result<(StatusCode, Json<Block>), ApiError> {
    let session = authenticated_session(&s, &headers)
        .await
        .map_err(|status| error(status, "authentication required"))?;
    let action_key = feature_routes::key(&headers).map_err(|e| (e.status, Json(e.value())))?;
    let (kind, slug) = core::parse_id(&input.id).map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let bytes = match (kind, &input.text, &input.archive_base64) {
        (BlockKind::Gene, Some(text), None) => text.as_bytes().to_vec(),
        (BlockKind::Isi | BlockKind::Function, None, Some(encoded)) => {
            if encoded.len() > core::MAX_ARCHIVE_BYTES.div_ceil(3) * 4 {
                return Err(error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "ZIP exceeds the 8 MiB limit",
                ));
            }
            BASE64
                .decode(encoded)
                .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid archive_base64"))?
        }
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "submit text for genes or archive_base64 for ISIs and functions",
            ));
        }
    };
    let search_text =
        core::validate_payload(&input.id, &bytes).map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let hash = core::content_hash(&bytes);
    // ponytail: serialize publication at the current 100/day target; shard by block if throughput grows.
    let _publication = s.block_publish_lock.lock().await;
    briefcase::retain_block_action(&s, &session, action_key, &input)
        .await
        .map_err(|e| (e.status, Json(e.value())))?;
    let previous = s.blocks.read().await.get(&input.id).cloned();
    let owner = session.actor_id().to_owned();
    if previous
        .as_ref()
        .is_some_and(|entry| require_owner(&session, &entry.block.owner_uuid).is_err())
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "only the owning account can publish this block",
        ));
    }
    if previous
        .as_ref()
        .zip(input.visibility.as_ref())
        .is_some_and(|(entry, visibility)| {
            std::mem::discriminant(&entry.block.visibility) != std::mem::discriminant(visibility)
        })
    {
        return Err(error(
            StatusCode::CONFLICT,
            "block visibility cannot change after publication",
        ));
    }
    let now = Utc::now();
    let block = Block {
        id: input.id.clone(),
        kind,
        name: input.name.unwrap_or_else(|| {
            previous
                .as_ref()
                .map(|e| e.block.name.clone())
                .unwrap_or_else(|| slug.into())
        }),
        description: input.description.unwrap_or_else(|| {
            previous
                .as_ref()
                .map(|e| e.block.description.clone())
                .unwrap_or_default()
        }),
        owner,
        owner_uuid: session.account_uuid().into(),
        visibility: input.visibility.unwrap_or_else(|| {
            previous
                .as_ref()
                .map(|e| e.block.visibility.clone())
                .unwrap_or_default()
        }),
        version: hash.clone(),
        downloads: previous.as_ref().map_or(0, |e| e.block.downloads),
        updated_at: now,
    };
    let mut versions = previous
        .as_ref()
        .map(|e| e.versions.clone())
        .unwrap_or_default();
    if !versions.iter().any(|v| v.version.version == hash) {
        let briefcase_entry = briefcase::publish_block(&s, &session, &block, &bytes, action_key)
            .await
            .map_err(|e| (e.status, Json(e.value())))?;
        let embedding = if std::env::var_os("GEMINI_API_KEY").is_some() {
            semantic::embed(
                &format!(
                    "{} {} {} {}",
                    block.id, block.name, block.description, search_text
                ),
                false,
            )
            .await
            .ok()
        } else {
            None
        };
        versions.push(StoredVersion {
            version: BlockVersion {
                version: hash,
                published_at: now,
            },
            content_base64: BASE64.encode(bytes),
            search_text,
            embedding,
            briefcase_entry,
        });
    }
    let _snapshot = s.persist_lock.lock().await;
    s.blocks.write().await.insert(
        input.id.clone(),
        StoredBlock {
            block: block.clone(),
            versions,
        },
    );
    if let Err(message) = save_state_unlocked(&s).await {
        let mut blocks = s.blocks.write().await;
        if let Some(previous) = previous {
            blocks.insert(input.id, previous);
        } else {
            blocks.remove(&input.id);
        }
        return Err(error(StatusCode::SERVICE_UNAVAILABLE, message));
    }
    Ok((StatusCode::CREATED, Json(block)))
}
