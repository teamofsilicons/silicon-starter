//! Durable account-bound publication through Briefcase 4 and Silicon Accounts.
//! A capability transfers private bytes; only a separately authorized commit
//! and explicit public-link operation make the registry release publishable.
use crate::{
    AppState, PublishRequest,
    auth::SessionTokens,
    authority::{Destination, Error, Feature, Result, digest, encode},
};
use axum::http::StatusCode;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use silicon_starter_core::{Version, release_version};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
struct Publication {
    id: Uuid,
    context_id: String,
    starter: String,
    input_hash: String,
    version: Version,
    bundle_hash: String,
    destination: Option<Destination>,
    parent_path: String,
    next_folder: usize,
    upload_id: Option<Uuid>,
    entry_id: Option<Uuid>,
    linked: bool,
    cancelled: bool,
}
impl Publication {
    fn upload_operation(&self) -> Uuid {
        operation(&json!([self.id, "upload"]))
    }
    fn link_operation(&self) -> Uuid {
        operation(&json!([self.id, "public-link"]))
    }
    fn save(&self, feature: &Feature, key: &str) -> Result<()> {
        feature
            .lease
            .put("publication", key, self)
            .map_err(Into::into)
    }
}
fn operation(value: &Value) -> Uuid {
    let hash = sha2::Sha256::digest(value.to_string().as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 15) | 0x50;
    bytes[8] = (bytes[8] & 63) | 0x80;
    Uuid::from_bytes(bytes)
}
use sha2::Digest;

pub(crate) async fn publish(
    s: &AppState,
    session: &SessionTokens,
    id: &str,
    input: &PublishRequest,
    key: &str,
) -> Result<Version> {
    let feature = crate::feature_routes::feature(s, session).await?;
    let provider = Provider::from_env()?;
    publish_with_feature(s, &feature, id, input, key, &provider).await
}
async fn publish_with_feature(
    s: &AppState,
    feature: &Feature,
    id: &str,
    input: &PublishRequest,
    key: &str,
    provider: &Provider,
) -> Result<Version> {
    let stored_key = digest(&encode(&(feature.context_id.as_str(), id, key))?);
    let input_hash = digest(&encode(input)?);
    let mut row = if let Some(row) = feature
        .lease
        .get::<Publication>("publication", &stored_key)?
    {
        if row.context_id != feature.context_id || row.starter != id || row.input_hash != input_hash
        {
            return Err(Error::conflict(
                "Retry publication with the original context, version and notes",
            ));
        }
        row
    } else {
        let version = release_version(&input.version).map_err(|_| {
            Error::new(
                StatusCode::BAD_REQUEST,
                "invalid_version",
                "Choose a valid release version",
            )
        })?;
        if s.versions
            .read()
            .await
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|v| release_version(&v.version).ok())
            .any(|v| v >= version)
        {
            return Err(Error::conflict(
                "This release version is already published or superseded",
            ));
        }
        let bundle = s.bundles.read().await.get(id).cloned().ok_or_else(|| {
            Error::new(
                StatusCode::BAD_REQUEST,
                "bundle_required",
                "Push the committed starter bundle before publishing",
            )
        })?;
        let commit = s
            .bundle_commits
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    StatusCode::BAD_REQUEST,
                    "commit_required",
                    "Push the committed starter bundle before publishing",
                )
            })?;
        let selected = input.commit.as_deref().unwrap_or(&input.selector);
        if selected != "latest" && selected != commit {
            return Err(Error::conflict(
                "The selected commit is not the pushed bundle; push that commit before publishing",
            ));
        }
        let row = Publication {
            id: Uuid::new_v4(),
            context_id: feature.context_id.clone(),
            starter: id.into(),
            input_hash,
            version: Version {
                version: input.version.clone(),
                commit,
                notes: input.notes.clone(),
                published_at: chrono::Utc::now(),
            },
            bundle_hash: digest(&bundle),
            destination: None,
            parent_path: String::new(),
            next_folder: 0,
            upload_id: None,
            entry_id: None,
            linked: false,
            cancelled: false,
        };
        feature.lease.put_many(vec![
            ("publication".into(), stored_key.clone(), encode(&row)?),
            ("publication-bytes".into(), row.id.to_string(), bundle),
        ])?;
        row
    };
    let folders = [
        "starters".to_owned(),
        feature.account_uuid.clone(),
        id.into(),
        row.version.version.clone(),
    ];
    drive_upload(
        feature,
        &mut row,
        &stored_key,
        provider,
        &folders,
        "starter.git.bundle",
        true,
    )
    .await?;
    // Record the release only after both provider commit and explicit public
    // access are confirmed. The durable receipt can restore a lost local save.

    let mut versions = s.versions.write().await;
    let entries = versions.entry(id.into()).or_default();
    if let Some(existing) = entries.iter().find(|v| v.version == row.version.version) {
        if existing.commit != row.version.commit || existing.notes != row.version.notes {
            return Err(Error::conflict("Another release occupies this version"));
        }
    } else {
        entries.push(row.version.clone());
    }
    drop(versions);
    if let Some(entry) = row.entry_id {
        s.briefcase_entries
            .write()
            .await
            .insert(format!("{id}:{}", row.version.commit), entry);
    }
    crate::persist_state_checked(s)
        .await
        .map_err(Error::unavailable)?;
    Ok(row.version)
}

pub(crate) async fn retain_block_action(
    s: &AppState,
    session: &SessionTokens,
    key: &str,
    input: &impl Serialize,
) -> Result<()> {
    if provider_unconfigured() {
        return Ok(());
    }
    let feature = crate::feature_routes::feature(s, session).await?;
    retain_block_intent(&feature, key, input)
}
fn retain_block_intent(feature: &Feature, key: &str, input: &impl Serialize) -> Result<()> {
    let hash = digest(&encode(input)?);
    if feature
        .lease
        .get::<String>("block-action", key)?
        .is_some_and(|saved| saved != hash)
    {
        return Err(Error::conflict(
            "Retry the original block action unchanged or start a new action",
        ));
    }
    feature.lease.put("block-action", key, &hash)?;
    Ok(())
}
fn provider_unconfigured() -> bool {
    std::env::var_os("STARTER_ACCOUNTS_APP_SECRET").is_none()
        && std::env::var_os("ACCOUNTS_APP_SECRET").is_none()
}
pub(crate) async fn publish_block(
    s: &AppState,
    session: &SessionTokens,
    block: &silicon_starter_core::blocks::Block,
    bytes: &[u8],
    key: &str,
) -> Result<Option<Uuid>> {
    if provider_unconfigured() {
        // Preserve the existing explicitly unconfigured local registry mode.
        return Ok(None);
    }
    let feature = crate::feature_routes::feature(s, session).await?;
    let provider = Provider::from_env()?;
    publish_block_with_feature(&feature, block, bytes, key, &provider)
        .await
        .map(Some)
}
async fn publish_block_with_feature(
    feature: &Feature,
    block: &silicon_starter_core::blocks::Block,
    bytes: &[u8],
    key: &str,
    provider: &Provider,
) -> Result<Uuid> {
    if block.owner_uuid != feature.account_uuid || digest(bytes) != block.version {
        return Err(Error::conflict(
            "Block content or account does not match the selected action",
        ));
    }
    let input_hash = digest(&encode(&json!([
        block.id,
        block.owner,
        block.name,
        block.description,
        block.visibility,
        block.version
    ]))?);
    let stored_key = digest(&encode(&("block-v2", &feature.context_id, &block.id, key))?);
    let mut row = if let Some(row) = feature
        .lease
        .get::<Publication>("publication", &stored_key)?
    {
        if row.context_id != feature.context_id
            || row.starter != block.id
            || row.input_hash != input_hash
        {
            return Err(Error::conflict(
                "Retry the original block content and metadata in its original account",
            ));
        }
        row
    } else {
        let row = Publication {
            id: Uuid::new_v4(),
            context_id: feature.context_id.clone(),
            starter: block.id.clone(),
            input_hash,
            version: Version {
                version: block.version.clone(),
                commit: block.version.clone(),
                notes: String::new(),
                published_at: block.updated_at,
            },
            bundle_hash: digest(bytes),
            destination: None,
            parent_path: String::new(),
            next_folder: 0,
            upload_id: None,
            entry_id: None,
            linked: false,
            cancelled: false,
        };
        feature.lease.put_many(vec![
            ("publication".into(), stored_key.clone(), encode(&row)?),
            (
                "publication-bytes".into(),
                row.id.to_string(),
                bytes.to_vec(),
            ),
        ])?;
        row
    };
    let (kind, slug) = block
        .id
        .split_once(':')
        .ok_or_else(|| Error::conflict("Invalid block identity"))?;
    let folders = [
        "blocks".to_owned(),
        feature.account_uuid.clone(),
        kind.into(),
        slug.into(),
        block.version.clone(),
    ];
    let filename = if kind == "gene" {
        "gene.md"
    } else {
        "block.zip"
    };
    let public = matches!(block.visibility, silicon_starter_core::Visibility::Public);
    drive_upload(
        feature,
        &mut row,
        &stored_key,
        provider,
        &folders,
        filename,
        public,
    )
    .await?;

    row.entry_id
        .ok_or_else(|| Error::unavailable("Briefcase omitted the completed block entry"))
}
async fn drive_upload(
    feature: &Feature,
    row: &mut Publication,
    stored_key: &str,
    provider: &Provider,
    folders: &[String],
    filename: &str,
    public: bool,
) -> Result<()> {
    if row.cancelled {
        return Err(Error::conflict(
            "This publication was cancelled; create a new action",
        ));
    }
    if !row.linked {
        provider.check_contract().await?;
        // Bind the selected account before the first provider mutation. Later
        // endpoint roots, refreshes and newly granted families must match it.
        let selected = feature.destination();
        if row.destination.as_ref().is_some_and(|d| d != &selected) {
            return Err(Error::conflict(
                "This publication belongs to its original Briefcase destination; restore that permission",
            ));
        }
        if row.destination.is_none() {
            row.destination = Some(selected);
            row.save(feature, stored_key)?;
        }
        while row.next_folder < folders.len() {
            let name = &folders[row.next_folder];
            let op = operation(&json!([
                "starter-folders-v1",
                feature.accounts.app_id,
                row.destination,
                row.parent_path,
                name
            ]));
            let value = provider
                .json(
                    feature,
                    row,
                    "briefcase.folders.create",
                    "/api/v1/obo/folders/create",
                    &json!({"operation_id":op,"parent_path":row.parent_path,"name":name}),
                )
                .await?;
            row.parent_path = value["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::unavailable("Briefcase omitted the created folder path"))?
                .to_owned();
            row.next_folder += 1;
            row.save(feature, stored_key)?;
        }
        let operation_id = row.upload_operation();
        // Always reconcile before resending any bytes, including after a
        // response loss or backend restart. The operation UUID never changes.
        let status = provider
            .json(
                feature,
                row,
                "briefcase.uploads.status",
                "/api/v1/obo/uploads/status",
                &json!({"operation_id":operation_id}),
            )
            .await;
        let mut current = match status {
            Ok(v) => Some(v),
            Err(e) if e.status == StatusCode::NOT_FOUND => None,
            Err(e) => return Err(e),
        };
        if let Some(status) = &current {
            validate_status(status, operation_id, row.upload_id)?;
        }
        if current.as_ref().is_none_or(|v| v["state"] == "reserved") {
            let bytes = feature
                .lease
                .get_bytes("publication-bytes", &row.id.to_string())?
                .ok_or_else(|| {
                    Error::unavailable("The original publication bytes are unavailable")
                })?;
            if digest(&bytes) != row.bundle_hash {
                return Err(Error::conflict("The saved publication content changed"));
            }
            let reservation=provider.json(feature,row,"briefcase.uploads.reserve","/api/v1/obo/uploads/reserve",&json!({"operation_id":operation_id,"parent_path":row.parent_path,"name":filename,"content_type":"application/octet-stream","size":bytes.len(),"sha256":row.bundle_hash})).await?;
            let upload_id = validate_status(&reservation, operation_id, row.upload_id)?;
            row.upload_id = Some(upload_id);
            row.save(feature, stored_key)?;
            if reservation["state"] == "reserved" {
                let capability = reservation["capability"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        Error::unavailable("Upload reservation is awaiting a fresh capability")
                    })?;
                let transferred = provider.transfer(upload_id, capability, bytes).await?;
                validate_status(&transferred, operation_id, Some(upload_id))?;
                current = Some(transferred);
            } else {
                current = Some(reservation);
            }
        }
        let mut status =
            current.ok_or_else(|| Error::unavailable("Briefcase upload state is unavailable"))?;
        let upload_id = validate_status(&status, operation_id, row.upload_id)?;
        row.upload_id = Some(upload_id);
        row.save(feature, stored_key)?;
        if status["state"] == "staged" {
            status = provider
                .json(
                    feature,
                    row,
                    "briefcase.uploads.commit",
                    "/api/v1/obo/uploads/commit",
                    &json!({"operation_id":operation_id,"upload_id":upload_id}),
                )
                .await?;
            validate_status(&status, operation_id, Some(upload_id))?;
        }
        if status["state"] != "committed" {
            if matches!(status["state"].as_str(), Some("cancelled" | "expired")) {
                return Err(Error::conflict(
                    "The original Briefcase upload expired or was cancelled; start a new publication",
                ));
            }
            return Err(Error::unavailable(
                "Briefcase is reconciling the original upload; retry the same publication",
            ));
        }
        let entry = uuid(&status["published_entry_id"])?;
        if row.entry_id.is_some_and(|previous| previous != entry) {
            return Err(Error::conflict(
                "Briefcase returned a different published entry",
            ));
        }
        row.entry_id = Some(entry);
        row.save(feature, stored_key)?;
        if public {
            let link = provider
                .json(
                    feature,
                    row,
                    "briefcase.link_access.update",
                    "/api/v1/obo/link-access",
                    &json!({"operation_id":row.link_operation(),"entry_id":entry,"enabled":true}),
                )
                .await?;
            if link["enabled"] != true || link["effective"] != true {
                return Err(Error::unavailable(
                    "Briefcase did not confirm public-link access",
                ));
            }
        }
        row.linked = true;
        row.save(feature, stored_key)?;
    }
    Ok(())
}
fn uuid(value: &Value) -> Result<Uuid> {
    value
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .filter(|id| !id.is_nil())
        .ok_or_else(|| Error::unavailable("Briefcase returned an invalid upload identity"))
}
fn validate_status(value: &Value, operation: Uuid, expected: Option<Uuid>) -> Result<Uuid> {
    if uuid(&value["operation_id"])? != operation {
        return Err(Error::conflict(
            "Briefcase returned a different logical upload",
        ));
    }
    let id = uuid(&value["upload_id"])?;
    if expected.is_some_and(|e| e != id) {
        return Err(Error::conflict(
            "Briefcase returned a different upload reservation",
        ));
    }
    Ok(id)
}

struct Provider {
    http: Client,
    base: String,
}
impl Provider {
    fn from_env() -> Result<Self> {
        Self::new(
            &std::env::var("BRIEFCASE_URL")
                .unwrap_or_else(|_| "https://api.briefcase.teamofsilicons.com".into()),
        )
    }
    fn new(base: &str) -> Result<Self> {
        crate::accounts::validate_url(base)?;
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|_| Error::unavailable("Could not configure Briefcase transport"))?,
            base: base.trim_end_matches('/').into(),
        })
    }
    async fn check_contract(&self) -> Result<()> {
        let response = self
            .http
            .get(format!("{}/api/version", self.base))
            .send()
            .await
            .map_err(|_| Error::unavailable("Briefcase version check failed"))?;
        let value = read_json(response).await?;
        if value["service"] != "silicon-briefcase"
            || value["selected_api_version"] != "v1"
            || value["contract_version"] != "4.0.0"
        {
            return Err(Error::unavailable(
                "Briefcase requires the Silicon Accounts receiver rollout",
            ));
        }
        let operations = value["operations"]
            .as_array()
            .ok_or_else(|| Error::unavailable("Briefcase omitted its operation contract"))?;
        for (method, path, revision) in [
            ("POST", "/obo/folders/create", "3.0.0"),
            ("POST", "/obo/uploads/reserve", "3.0.0"),
            ("POST", "/obo/uploads/status", "3.0.0"),
            ("POST", "/obo/uploads/commit", "3.0.0"),
            ("POST", "/obo/link-access", "3.0.0"),
            ("PUT", "/obo/uploads/{upload_id}/content", "2.1.0"),
        ] {
            if !operations
                .iter()
                .any(|op| op["path"] == path && op["method"] == method && op["version"] == revision)
            {
                return Err(Error::unavailable(
                    "Briefcase has not activated delegated publication operations",
                ));
            }
        }
        Ok(())
    }
    async fn json(
        &self,
        feature: &Feature,
        row: &Publication,
        endpoint: &str,
        path: &str,
        body: &Value,
    ) -> Result<Value> {
        if row.destination.as_ref() != Some(&feature.destination()) {
            return Err(Error::conflict(
                "This publication belongs to a different Briefcase account",
            ));
        }
        // Briefcase consumes proofs: every request, including reconciliation,
        // gets a fresh proof while the durable operation manifest stays fixed.
        let proof = feature.proof(endpoint).await?;
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header("authorization", format!("Proof {proof}"))
            .json(body)
            .send()
            .await
            .map_err(|_| {
                Error::unavailable(
                    "Briefcase response is uncertain; retry the original publication",
                )
            })?;
        read_json(response).await
    }
    async fn transfer(&self, id: Uuid, capability: &str, bytes: Vec<u8>) -> Result<Value> {
        // The upload capability alone transfers bytes into the bound reservation.
        let request = self
            .http
            .put(format!("{}/api/v1/obo/uploads/{id}/content", self.base))
            .header("x-briefcase-upload-capability", capability)
            .header("content-type", "application/octet-stream")
            .body(bytes);
        read_json(request.send().await.map_err(|_|Error::unavailable("Upload transfer response is uncertain; retry the original publication to check its status"))?).await
    }
}
async fn read_json(mut response: Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            404 => Error::new(
                StatusCode::NOT_FOUND,
                "provider_not_found",
                "Briefcase has no matching resource",
            ),
            403 => Error::new(
                StatusCode::FORBIDDEN,
                "briefcase_forbidden",
                "The approved Briefcase account cannot perform this action",
            ),
            409 => Error::conflict("Briefcase reports a conflict with this original operation"),
            412 => {
                let mut e = Error::permission();
                e.status = StatusCode::PRECONDITION_FAILED;
                e
            }
            _ => Error::unavailable(
                "Briefcase could not finish this operation; retain its original identity for retry",
            ),
        });
    }
    let mut body = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::unavailable("Briefcase response was interrupted"))?
    {
        if body.len() + chunk.len() > 128 * 1024 {
            return Err(Error::unavailable("Briefcase response exceeded its bound"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| Error::unavailable("Briefcase returned an unreadable response"))
}
