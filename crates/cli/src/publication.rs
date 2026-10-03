//! Saved publication intent is immutable until explicit retry or local cancellation.
use crate::session::{self, Result};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use silicon_starter_core::{
    local::{self, CheckoutContext, Mode},
    release_version,
};
use uuid::Uuid;
#[derive(Serialize, Deserialize)]
struct Receipt {
    api: String,
    context: CheckoutContext,
    path: String,
    key: String,
    body: Value,
}
pub async fn run(selector: &str, version: Option<&str>, notes: Option<&str>) -> Result<()> {
    let root = local::repo_root()?;
    let binding = local::load_binding(&root)?;
    if binding.mode != Mode::Development {
        return Err("only pulled developer starters may publish".into());
    }
    let selected = session::current()?;
    let _lock = selected.selection.lock().await?;
    let path = root.join(".git/starter-publication.json");
    let existing = session::read_private(&path)?
        .map(|bytes| serde_json::from_slice::<Receipt>(&bytes))
        .transpose()?;
    if selector == "cancel" {
        if let Some(receipt) = existing
            && (receipt.api != selected.selection.api
                || receipt.context.profile != selected.selection.profile
                || receipt.context.world != selected.selection.world)
        {
            return Err(
                "select the publication's original API, profile and world before cancelling it"
                    .into(),
            );
        }
        session::remove(&path)?;
        println!("Local publication retry discarded; this does not undo a completed publication.");
        return Ok(());
    }
    let snapshot = selected.for_checkout(&binding)?;
    let context = snapshot.binding();
    if snapshot.saved.is_none() {
        return Err("publishing requires a saved login; explicitly use starter context bind for this checkout".into());
    }
    let request_path = format!("/api/v1/starters/{}/publish", binding.id);
    let body = if selector == "retry" {
        None
    } else {
        let version = version.ok_or("publish requires version Y.X")?;
        release_version(version).map_err(|e| format!("invalid release version: {e}"))?;
        let commit = if selector == "latest" {
            local::head(&root)?
        } else {
            local::run_git(&root, &["rev-parse", &format!("{selector}^{{commit}}")])?
        };
        Some(
            json!({"selector":selector,"version":version,"commit":commit,"notes":notes.unwrap_or("")}),
        )
    };
    let receipt = match (existing, body) {
        (Some(receipt), body) => {
            if receipt.api != snapshot.selection.api
                || receipt.context != context
                || receipt.path != request_path
            {
                return Err("pending publication belongs to another login or starter; return to its original context".into());
            }
            if body.is_some_and(|body| body != receipt.body) {
                return Err("another publication is pending; use publish retry to preserve its exact commit/notes, or publish cancel".into());
            }
            receipt
        }
        (None, Some(body)) => Receipt {
            api: snapshot.selection.api.clone(),
            context,
            path: request_path,
            key: Uuid::new_v4().to_string(),
            body,
        },
        (None, None) => return Err("there is no interrupted publication to retry".into()),
    };
    session::private_write(&path, &receipt)?;
    let response = snapshot
        .request(
            &receipt.path,
            Method::POST,
            Some(receipt.body.clone()),
            Some(&receipt.key),
            true,
        )
        .await?;
    snapshot.check_current()?;
    session::remove(&path)?;
    println!("{response}");
    Ok(())
}
