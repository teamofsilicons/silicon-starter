use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::Args;
use reqwest::Method;
use serde_json::json;
use silicon_starter_core::{blocks, local};
use std::{fs, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args, Default)]
pub struct PublishOptions {
    /// Publish gene Markdown directly, instead of reading a file.
    #[arg(long)]
    text: Option<String>,
    /// IAM organization that owns this block.
    #[arg(long)]
    org: Option<String>,
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
    let bytes = match (file, &options.text) {
        (Some(file), None) => fs::read(file)?,
        (None, Some(text)) if id.starts_with("gene:") => text.as_bytes().to_vec(),
        _ => return Err("publish a block with one Markdown/ZIP file, or --text for a gene".into()),
    };
    blocks::validate_payload(id, &bytes)?;
    let mut body = json!({"id": id});
    for (key, value) in [
        ("org_id", &options.org),
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
    super::authed_request(api, "/api/v1/blocks", Method::POST, Some(body)).await
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
