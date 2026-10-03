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
#[derive(Deserialize)]
pub struct PublishBlock {
    id: String,
    org_id: Option<String>,
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
    let orgs = authenticated_session(&s, &headers)
        .await
        .map(|session| session.organizations())
        .unwrap_or_default();
    let term = q.q.unwrap_or_default().to_lowercase();
    let mut items: Vec<_> = s
        .blocks
        .read()
        .await
        .values()
        .filter(|entry| {
            matches!(entry.block.visibility, Visibility::Public)
                || orgs.contains(&entry.block.owner)
        })
        .filter(|entry| q.org.as_ref().is_none_or(|org| org == &entry.block.owner))
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
            org: None,
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
            .is_ok_and(|session| session.organizations().contains(&entry.block.owner))
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
    let previous = s.blocks.read().await.get(&input.id).cloned();
    let owner = choose_organization(
        &session,
        input
            .org_id
            .as_deref()
            .or_else(|| previous.as_ref().map(|e| e.block.owner.as_str())),
    )
    .map_err(|status| error(status, "choose an organization authorized through IAM"))?;
    if previous
        .as_ref()
        .is_some_and(|entry| entry.block.owner != owner)
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "only the owning organization can publish this block",
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
        let briefcase_entry = upload(&block, &bytes, &session)
            .await
            .map_err(|e| error(StatusCode::BAD_GATEWAY, e))?;
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
async fn upload(
    block: &Block,
    bytes: &[u8],
    session: &auth::IamTokens,
) -> Result<Option<Uuid>, String> {
    if std::env::var_os("BRIEFCASE_APP_SECRET").is_none()
        && std::env::var_os("STARTER_IAM_APP_SECRET").is_none()
    {
        return Ok(None);
    }
    let storage = briefcase::BriefcaseStorage::from_env(&session.access_token, &block.owner)?;
    let (kind, slug) = block.id.split_once(':').ok_or("invalid block id")?;
    let parent = storage
        .ensure_path(&["blocks", &block.owner, kind, slug, &block.version])
        .await?;
    let filename = if block.kind == BlockKind::Gene {
        "gene.md"
    } else {
        "block.zip"
    };
    let result = storage
        .upload_public_bundle(&parent, filename, bytes.to_vec())
        .await?;
    let entry = result
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or("Briefcase upload returned no entry id")?;
    if matches!(block.visibility, Visibility::Public) {
        storage.set_public_link(entry).await?;
    }
    Ok(Some(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn session(s: &AppState, org: &str) -> HeaderMap {
        super::super::auth_and_organization_tests::session(s, &[org]).await
    }
    fn gene(text: &str, visibility: Option<Visibility>) -> PublishBlock {
        PublishBlock {
            id: "gene:creativity".into(),
            org_id: None,
            name: None,
            description: None,
            visibility,
            text: Some(text.into()),
            archive_base64: None,
        }
    }
    #[tokio::test]
    async fn authenticated_versions_remain_immutable_private_and_searchable_after_restore() {
        let s = AppState::default();
        let owner = session(&s, "tos").await;
        let outsider = session(&s, "elsewhere").await;
        assert_eq!(
            publish(
                State(s.clone()),
                HeaderMap::new(),
                Json(gene("first", None))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::UNAUTHORIZED
        );
        let first = publish(
            State(s.clone()),
            owner.clone(),
            Json(gene("first", Some(Visibility::Private))),
        )
        .await
        .unwrap()
        .1
        .0;
        assert_eq!(
            publish(
                State(s.clone()),
                outsider.clone(),
                Json(gene("hijacked", None))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        for headers in [HeaderMap::new(), outsider] {
            assert_eq!(
                show(State(s.clone()), headers.clone(), Path(first.id.clone()))
                    .await
                    .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            assert!(
                list(
                    State(s.clone()),
                    headers.clone(),
                    Query(ListQuery {
                        q: None,
                        org: None,
                        visibility: None
                    })
                )
                .await
                .0
                .is_empty()
            );
            assert!(search_items(&s, &headers).await.is_empty());
            assert_eq!(
                download(
                    State(s.clone()),
                    headers,
                    Path(first.id.clone()),
                    Query(HashMap::new())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
        let second = publish(
            State(s.clone()),
            owner.clone(),
            Json(gene("second telescope", None)),
        )
        .await
        .unwrap()
        .1
        .0;
        assert_ne!(first.version, second.version);
        let _ = publish(
            State(s.clone()),
            owner.clone(),
            Json(gene("second telescope", None)),
        )
        .await
        .unwrap();
        assert_eq!(
            versions(State(s.clone()), owner.clone(), Path(first.id.clone()))
                .await
                .unwrap()
                .0
                .len(),
            2
        );
        let fetch = |id: String| {
            content(
                State(s.clone()),
                owner.clone(),
                Path(id),
                Query(HashMap::new()),
            )
        };
        assert_eq!(
            fetch(format!("{}@{}", first.id, first.version))
                .await
                .unwrap()
                .0["text"],
            "first"
        );
        assert_eq!(
            fetch(first.id.clone()).await.unwrap().0["text"],
            "second telescope"
        );
        assert_eq!(
            fetch(format!("{}@{}", first.id, "f".repeat(64)))
                .await
                .unwrap_err(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            search(
                State(s.clone()),
                owner.clone(),
                Query(SearchQuery {
                    q: Some("telescope".into())
                })
            )
            .await
            .0
            .len(),
            1
        );
        let restored = AppState::default();
        restore_state(&restored, json!({"blocks": *s.blocks.read().await}))
            .await
            .unwrap();
        let headers = session(&restored, "tos").await;
        assert_eq!(
            content(
                State(restored),
                headers,
                Path(format!("{}@{}", first.id, first.version)),
                Query(HashMap::new())
            )
            .await
            .unwrap()
            .0["text"],
            "first"
        );
        assert_eq!(
            publish(
                State(s.clone()),
                owner.clone(),
                Json(gene("second telescope", Some(Visibility::Public)))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let mut public = gene("visible", None);
        public.id = "gene:public".into();
        let _ = publish(State(s.clone()), owner, Json(public))
            .await
            .unwrap();
        assert!(
            download(
                State(s),
                HeaderMap::new(),
                Path("gene:public".into()),
                Query(HashMap::new())
            )
            .await
            .is_ok()
        );
    }
}
