//! Explicit, durable Briefcase feature approval; never invoked by ordinary login.
use crate::session::{self, Result, Snapshot};
use clap::Subcommand;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read},
};
use uuid::Uuid;

pub const ENDPOINTS: [&str; 5] = [
    "briefcase.folders.create",
    "briefcase.uploads.reserve",
    "briefcase.uploads.commit",
    "briefcase.uploads.status",
    "briefcase.link_access.update",
];
#[derive(Subcommand)]
pub enum PermissionCommand {
    /// Start or recover Briefcase permission review. Open the returned IAM HTTPS link.
    Authorize,
    /// Inspect the review; an uncertain start is recovered with its original key.
    Status,
    /// Complete IAM's manual code, or omit the file to retry a saved uncertain completion.
    Complete {
        #[arg(long)]
        code_file: Option<String>,
    },
    /// Discard this local review and retained code; publication drafts stay saved.
    Cancel,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    context_id: String,
    start_key: String,
    complete_key: String,
    request: Option<Value>,
    code: Option<String>,
    code_started_at: Option<u64>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub async fn run(command: PermissionCommand) -> Result<()> {
    let snapshot = session::current()?;
    let saved = snapshot
        .saved
        .as_ref()
        .ok_or("log in before reviewing Briefcase permission")?;
    let _lock = snapshot.selection.lock().await?;
    snapshot.check_current()?;
    let path = snapshot.selection.dir.join("briefcase-permission.json");
    let mut receipt: Option<Receipt> = session::read_private(&path)?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?;
    if matches!(command, PermissionCommand::Cancel) {
        session::remove(&path)?;
        println!("Local permission review discarded; pending publication retained.");
        return Ok(());
    }
    if receipt
        .as_ref()
        .is_some_and(|r| r.context_id != saved.context_id)
    {
        return Err("saved permission belongs to another login; use permission cancel before starting a fresh review".into());
    }
    if receipt.is_none() && matches!(command, PermissionCommand::Status) {
        println!("{}", json!({"context_id":saved.context_id,"request":null}));
        return Ok(());
    }
    if receipt.is_none() && matches!(command, PermissionCommand::Authorize) {
        receipt = Some(Receipt {
            context_id: saved.context_id.clone(),
            start_key: Uuid::new_v4().to_string(),
            complete_key: Uuid::new_v4().to_string(),
            request: None,
            code: None,
            code_started_at: None,
        });
    }
    let mut receipt = receipt.ok_or("start review with starter permission authorize")?;
    let previous = receipt.request.clone();
    if let PermissionCommand::Complete { code_file } = &command {
        let request = previous
            .as_ref()
            .ok_or("recover the permission start before entering a code")?;
        if request["completed"] == true {
            println!("{request}");
            return Ok(());
        }
        if receipt.code.is_none() && expiry(request)? <= now() {
            session::remove(&path)?;
            return Err("permission review expired; start a fresh review; pending publication remains saved".into());
        }
        if matches!(
            request["authorization"]["status"].as_str(),
            Some("declined" | "expired")
        ) || receipt
            .code_started_at
            .is_some_and(|t| now().saturating_sub(t) > 600)
        {
            session::remove(&path)?;
            return Err("permission declined or expired; start a fresh review. Pending publication is retained".into());
        }
        if let Some(file) = code_file {
            let mut code = String::new();
            if file == "-" {
                io::stdin().take(8193).read_to_string(&mut code)?;
            } else {
                code = fs::read_to_string(file)?;
            }
            let code = code.trim().to_owned();
            if code.is_empty() || code.len() > 8192 {
                return Err("approval code must be nonempty and at most 8192 bytes".into());
            }
            if receipt.code.as_ref().is_some_and(|old| old != &code) {
                return Err(
                    "another code is pending; recover without --code-file or cancel this review"
                        .into(),
                );
            }
            receipt.code = Some(code);
            receipt.code_started_at.get_or_insert_with(now);
        }
        if receipt.code.is_none() {
            return Err(
                "provide --code-file <file|->, or recover an already saved completion".into(),
            );
        }
    }
    session::private_write(&path, &receipt)?;
    let result = match &command {
        PermissionCommand::Authorize | PermissionCommand::Status if previous.is_none() => {
            snapshot
                .request(
                    "/api/v1/briefcase/authorization",
                    Method::POST,
                    Some(json!({})),
                    Some(&receipt.start_key),
                    true,
                )
                .await
        }
        PermissionCommand::Authorize | PermissionCommand::Status => {
            snapshot
                .request(
                    &format!(
                        "/api/v1/briefcase/authorizations/{}",
                        request_id(previous.as_ref().ok_or("review missing")?)?
                    ),
                    Method::GET,
                    None,
                    None,
                    true,
                )
                .await
        }
        PermissionCommand::Complete { .. } => {
            snapshot
                .request(
                    &format!(
                        "/api/v1/briefcase/authorizations/{}/complete",
                        request_id(previous.as_ref().ok_or("review missing")?)?
                    ),
                    Method::POST,
                    Some(json!({"code":receipt.code})),
                    Some(&receipt.complete_key),
                    true,
                )
                .await
        }
        PermissionCommand::Cancel => unreachable!(),
    };
    let response = match result {
        Ok(response) => response,
        Err(error)
            if error
                .downcast_ref::<session::HttpError>()
                .is_some_and(|error| error.status == 412) =>
        {
            session::remove(&path)?;
            return Err("permission terms changed (HTTP 412); start a fresh review. Pending publication is retained".into());
        }
        Err(error)
            if error
                .downcast_ref::<session::HttpError>()
                .is_some_and(|error| {
                    error.status == 403 && error.value["error"]["code"] == "reconsent_required"
                }) =>
        {
            session::remove(&path)?;
            return Err("permission declined or expired; start a fresh review. Pending publication is retained".into());
        }
        Err(error) => return Err(error),
    };
    validate(&response, snapshot, previous.as_ref())?;
    if matches!(command, PermissionCommand::Complete { .. }) && response["completed"] != true {
        return Err("completion was not confirmed; recover the same request".into());
    }
    let response = public_response(&response);
    receipt.request = Some(response.clone());
    if response["completed"] == true {
        receipt.code = None;
        receipt.code_started_at = None;
    }
    session::private_write(&path, &receipt)?;
    println!("{response}");
    if response["completed"] == true {
        eprintln!("Permission approved. Retry publication explicitly with starter publish retry.");
    }
    Ok(())
}
fn expiry(value: &Value) -> Result<u64> {
    let time = chrono::DateTime::parse_from_rfc3339(
        value["authorization"]["expires_at"]
            .as_str()
            .ok_or("permission expiry missing")?,
    )?;
    Ok(u64::try_from(time.timestamp()).unwrap_or_default())
}
fn request_id(value: &Value) -> Result<Uuid> {
    Ok(Uuid::parse_str(
        value["request_id"]
            .as_str()
            .ok_or("permission response omitted request_id")?,
    )?)
}
pub fn validate(response: &Value, snapshot: &Snapshot, previous: Option<&Value>) -> Result<()> {
    let saved = snapshot
        .saved
        .as_ref()
        .ok_or("permission requires authentication")?;
    let detail = &response["authorization"];
    expiry(response)?;
    if !response["completed"].is_boolean() {
        return Err("permission response omitted completion state".into());
    }
    let request = request_id(response)?;
    let auth = Uuid::parse_str(detail["id"].as_str().ok_or("approval ID missing")?)?;
    if request.is_nil()
        || auth.is_nil()
        || response["context_id"] != saved.context_id
        || detail["app_id"] != "starter"
        || detail["actor"] != serde_json::to_value(&saved.actor)?
        || detail["org_id"] != saved.org_id
        || !detail["version"].as_u64().is_some_and(|v| v > 0)
    {
        return Err(
            "permission response does not match the original account, organization and request"
                .into(),
        );
    }
    if !matches!(
        detail["status"].as_str(),
        Some("pending" | "approved" | "declined" | "expired" | "exchanged")
    ) || response["completed"] == true
        && !matches!(detail["status"].as_str(), Some("approved" | "exchanged"))
    {
        return Err("permission response has an invalid state".into());
    }
    if let Some(url) = detail["authorization_url"].as_str() {
        let url = reqwest::Url::parse(url)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("permission review URL is unsafe".into());
        }
    }
    if previous.is_some_and(|old| {
        old["request_id"] != response["request_id"]
            || old["authorization"]["id"] != detail["id"]
            || old["authorization"]["state"] != detail["state"]
    }) {
        return Err("permission response replaced the initiating request or state".into());
    }
    Ok(())
}
fn public_response(response: &Value) -> Value {
    let detail = &response["authorization"];
    json!({"request_id":response["request_id"],"context_id":response["context_id"],"completed":response["completed"],"roots":response["roots"],
        "authorization":{"id":detail["id"],"app_id":detail["app_id"],"actor":detail["actor"],"org_id":detail["org_id"],"status":detail["status"],"version":detail["version"],"expires_at":detail["expires_at"],"state":detail["state"],"authorization_url":detail["authorization_url"]}})
}
