use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::Args;
use reqwest::Method;
use serde_json::json;
use silicon_starter_core::{blocks, local};
use std::{fs, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args, Default)]
pub struct PublishOptions {
    /// Retry the saved block bytes and metadata in this profile.
    #[arg(long, conflicts_with = "cancel")]
    retry: bool,
    /// Discard only the local pending retry; never deletes a published block.
    #[arg(long, conflicts_with = "retry")]
    cancel: bool,
    /// Publish gene Markdown directly, instead of reading a file.
    #[arg(long)]
    text: Option<String>,
    #[arg(long, value_parser = ["public", "private"])]
    visibility: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
}

pub fn is_block(id: &str) -> bool {
    ["gene:", "isi:", "function:"]
        .iter()
        .any(|prefix| id.starts_with(prefix))
}

pub async fn publish(
    api: &str,
    id: &str,
    file: Option<&str>,
    options: &PublishOptions,
) -> Result<()> {
    blocks::parse_id(id)?;
    let snapshot = super::session::current()?;
    let _lock = snapshot.selection.lock().await?;
    let context = snapshot.binding();
    let receipt_path = snapshot.selection.dir.join("block-publication.json");
    let pending = super::session::read_private(&receipt_path)?
        .map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes))
        .transpose()?;
    if let Some(receipt) = &pending
        && (receipt["api"] != api
            || receipt["body"]["id"] != id
            || !options.cancel && receipt["context"] != serde_json::to_value(&context)?)
    {
        return Err("Select the original API and block ID; retries also require the original account.".into());
    }
    if options.cancel {
        super::session::remove(&receipt_path)?;
        println!("Local block retry discarded; a completed publication is unchanged.");
        return Ok(());
    }
    if snapshot.saved.is_none() {
        return Err("publishing requires a saved login".into());
    }
    if options.retry {
        let receipt = pending.ok_or("no block publication is waiting to retry")?;
        let response = snapshot
            .request(
                "/api/v1/blocks",
                Method::POST,
                Some(receipt["body"].clone()),
                receipt["key"].as_str(),
                true,
            )
            .await?;
        snapshot.check_current()?;
        super::session::remove(&receipt_path)?;
        println!("{response}");
        return Ok(());
    }
    let bytes = match (file, &options.text) {
        (Some(file), None) => fs::read(file)?,
        (None, Some(text)) if id.starts_with("gene:") => text.as_bytes().to_vec(),
        _ => return Err("publish a block with one Markdown/ZIP file, or --text for a gene".into()),
    };
    blocks::validate_payload(id, &bytes)?;
    let mut body = json!({"id": id});
    for (key, value) in [
        ("visibility", &options.visibility),
        ("name", &options.name),
        ("description", &options.description),
    ] {
        if let Some(value) = value {
            body[key] = json!(value);
        }
    }
    if id.starts_with("gene:") {
        body["text"] = json!(String::from_utf8(bytes)?);
    } else {
        body["archive_base64"] = json!(B64.encode(bytes));
    }
    let receipt = if let Some(receipt) = pending {
        if receipt["api"] != api
            || receipt["context"] != serde_json::to_value(&context)?
            || receipt["body"] != body
        {
            return Err("a block publication is pending in this profile; retry its original ID, file and metadata before another publication".into());
        }
        receipt
    } else {
        json!({"api":api,"context":context,"body":body,"key":uuid::Uuid::new_v4().to_string()})
    };
    super::session::private_write(&receipt_path, &receipt)?;
    let response = snapshot
        .request(
            "/api/v1/blocks",
            Method::POST,
            Some(receipt["body"].clone()),
            receipt["key"].as_str(),
            true,
        )
        .await?;
    snapshot.check_current()?;
    super::session::remove(&receipt_path)?;
    println!("{response}");
    Ok(())
}

pub async fn history(api: &str, id: &str) -> Result<()> {
    blocks::parse_id(id)?;
    super::request(
        api,
        &format!("/api/v1/blocks/{id}/versions"),
        Method::GET,
        None,
    )
    .await
}

pub async fn download(api: &str, spec: &str, destination: Option<&Path>) -> Result<()> {
    let (id, version) = spec
        .split_once('@')
        .map_or((spec, "latest"), |(id, version)| (id, version));
    let (_, name) = blocks::parse_id(id)?;
    if version != "latest"
        && (version.len() != 64 || !version.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("block version must be latest or a full content hash".into());
    }
    let target = std::path::absolute(destination.unwrap_or_else(|| Path::new(name)))?;
    if target.exists() && (!target.is_dir() || fs::read_dir(&target)?.next().is_some()) {
        return Err(format!(
            "destination {} is occupied; choose another folder with --dir",
            target.display()
        )
        .into());
    }
    let response = super::request_value(
        api,
        &format!("/api/v1/blocks/{id}/download?version={version}"),
        Method::GET,
        None,
    )
    .await?;
    if response["id"].as_str() != Some(id) {
        return Err("block response returned a different ID".into());
    }
    let bytes = if id.starts_with("gene:") {
        response["text"]
            .as_str()
            .ok_or("gene response omitted text")?
            .as_bytes()
            .to_vec()
    } else {
        B64.decode(
            response["archive_base64"]
                .as_str()
                .ok_or("block response omitted archive_base64")?,
        )?
    };
    let hash = blocks::content_hash(&bytes);
    if response["version"].as_str() != Some(&hash) || (version != "latest" && version != hash) {
        return Err("downloaded block does not match its content hash".into());
    }
    blocks::validate_payload(id, &bytes)?;
    let parent = target
        .parent()
        .ok_or("destination has no parent directory")?;
    fs::create_dir_all(parent)?;
    let stage = super::templates::Temporary(parent.join(format!(
        ".starter-block-{}-{}",
        std::process::id(),
        local::unique()
    )));
    fs::create_dir(&stage.0)?;
    if id.starts_with("gene:") {
        fs::write(stage.0.join(format!("{name}.md")), bytes)?;
    } else {
        blocks::extract_archive(&bytes, &stage.0)?;
    }
    if target.exists() {
        fs::remove_dir(&target)?;
    }
    fs::rename(&stage.0, &target)?;
    println!("downloaded {id}@{hash} in {}", target.display());
    Ok(())
}
