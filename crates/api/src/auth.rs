//! Small, deliberately stateless IAM boundary helpers.
//! Secrets and refresh/session persistence belong to the application store.

use axum::http::HeaderMap;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IamTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub actor: Option<Value>,
    #[serde(default)]
    pub org_id: Option<String>,
    #[serde(default)]
    pub org_ids: Vec<String>,
}

impl IamTokens {
    pub fn organizations(&self) -> Vec<String> {
        let mut orgs = self.org_ids.clone();
        orgs.extend(self.org_id.iter().cloned());
        orgs.retain(|org| !org.is_empty());
        orgs.sort();
        orgs.dedup();
        orgs
    }
}

#[derive(Clone, Default)]
pub struct AuthState {
    sessions: Arc<RwLock<HashMap<String, IamTokens>>>,
    file: Option<PathBuf>,
}

impl AuthState {
    pub fn from_env() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            file: std::env::var_os("STARTER_AUTH_FILE").map(PathBuf::from),
        }
    }

    pub async fn load(&self) -> Result<(), String> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("cannot read session store: {error}")),
        };
        let sessions =
            serde_json::from_str(&text).map_err(|e| format!("session store is invalid: {e}"))?;
        *self.sessions.write().await = sessions;
        Ok(())
    }

    async fn persist(&self) -> Result<(), String> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_vec(&*self.sessions.read().await).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, text)
            .await
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::rename(tmp, path)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn insert(&self, tokens: IamTokens) -> Result<String, String> {
        let id = Uuid::now_v7().to_string();
        self.sessions.write().await.insert(id.clone(), tokens);
        self.persist().await?;
        Ok(id)
    }
    pub async fn login(
        &self,
        slt: &str,
        expected_state: Option<&str>,
        state: Option<&str>,
        app_id: &str,
        app_secret: &str,
    ) -> Result<String, String> {
        if !valid_login_state(expected_state, state) {
            return Err("login state does not match the initiating browser".into());
        }
        self.insert(exchange_slt(slt, app_id, app_secret).await?)
            .await
    }
    pub async fn get(&self, id: &str) -> Option<IamTokens> {
        self.sessions
            .read()
            .await
            .get(id)
            .filter(|tokens| {
                tokens
                    .expires_at
                    .is_some_and(|expiry| expiry > Utc::now().timestamp())
            })
            .cloned()
    }
    pub async fn remove(&self, id: &str) -> Result<bool, String> {
        let removed = self.sessions.write().await.remove(id).is_some();
        if removed {
            self.persist().await?;
        }
        Ok(removed)
    }
    pub async fn status(&self, id: Option<&str>) -> Value {
        match match id {
            Some(id) => self.get(id).await,
            None => None,
        } {
            Some(t) => {
                let orgs = t.organizations();
                serde_json::json!({"authenticated":true,"actor":t.actor,"org_id":if orgs.len() == 1 { orgs.first() } else { None },"org_ids":orgs})
            }
            None => serde_json::json!({"authenticated":false}),
        }
    }
}

pub fn valid_login_state(expected: Option<&str>, supplied: Option<&str>) -> bool {
    matches!((expected, supplied), (Some(expected), Some(supplied)) if !expected.is_empty() && expected == supplied)
}

fn authorized_organizations(value: &Value, app_id: &str) -> Result<Vec<String>, String> {
    if value["active"] != true || value["client_id"].as_str() != Some(app_id) {
        return Err("IAM session is inactive or belongs to another application".into());
    }
    let mut orgs: Vec<String> = value
        .get("authorizations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(value.get("authorization"))
        .filter_map(|authorization| authorization.get("org_id").and_then(Value::as_str))
        .filter(|org| !org.is_empty())
        .map(str::to_owned)
        .collect();
    orgs.sort();
    orgs.dedup();
    Ok(orgs)
}

/// Exchange an IAM SLT. The SLT is never persisted or returned by this helper.
pub async fn exchange_slt(slt: &str, app_id: &str, app_secret: &str) -> Result<IamTokens, String> {
    exchange_slt_with_key(slt, app_id, app_secret, &Uuid::now_v7().to_string()).await
}

/// Variant for recovering an uncertain exchange: persist and reuse this key with identical input.
pub async fn exchange_slt_with_key(
    slt: &str,
    app_id: &str,
    app_secret: &str,
    idempotency_key: &str,
) -> Result<IamTokens, String> {
    if !slt.starts_with("oac_") {
        return Err("credential is not an IAM short-lived token".into());
    }
    let response = reqwest::Client::new()
        .post("https://backend.iam.teamofsilicons.com/api/v1/app-auth/tokens")
        .basic_auth(app_id, Some(app_secret))
        .header("Idempotency-Key", idempotency_key)
        .form(&[("app_id", app_id), ("slt", slt)])
        .send()
        .await
        .map_err(|e| format!("IAM token exchange failed: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("IAM response read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("IAM token exchange returned {status}: {body}"));
    }
    let mut tokens: IamTokens =
        serde_json::from_str(&body).map_err(|e| format!("IAM returned invalid token JSON: {e}"))?;
    let authorization = introspect(&tokens.access_token, app_id, app_secret, None).await?;
    tokens.org_ids = authorized_organizations(&authorization, app_id)?;
    tokens.expires_at = Some(
        authorization["expires_at"]
            .as_i64()
            .filter(|expiry| *expiry > Utc::now().timestamp())
            .ok_or("IAM returned an expired session or no token expiry")?,
    );
    // IAM's current multi-organization flow omits the legacy token org_id.
    tokens.org_id = (tokens.org_ids.len() == 1).then(|| tokens.org_ids[0].clone());
    Ok(tokens)
}

#[allow(dead_code)]
pub async fn refresh_tokens(
    refresh_token: &str,
    app_id: &str,
    app_secret: &str,
    idempotency_key: &str,
) -> Result<IamTokens, String> {
    let response = reqwest::Client::new()
        .post("https://backend.iam.teamofsilicons.com/api/v1/app-auth/tokens")
        .basic_auth(app_id, Some(app_secret))
        .header("Idempotency-Key", idempotency_key)
        .form(&[("app_id", app_id), ("refresh_token", refresh_token)])
        .send()
        .await
        .map_err(|e| format!("IAM refresh failed: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("IAM response read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("IAM refresh returned {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("IAM returned invalid refresh JSON: {e}"))
}

pub async fn introspect(
    access_token: &str,
    app_id: &str,
    app_secret: &str,
    org: Option<&str>,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let mut request = client
        .post("https://backend.iam.teamofsilicons.com/api/v1/oauth/introspect")
        .basic_auth(app_id, Some(app_secret))
        .form(&[("token", access_token), ("token_type_hint", "access_token")]);
    if let Some(org) = org {
        request = request.header("X-Org-ID", org);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("IAM introspection failed: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("IAM response read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("IAM introspection returned {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("IAM returned invalid introspection JSON: {e}"))
}

/// Verify IAM's signature over the exact, unparsed request body.
/// `now` is injectable so replay-window tests do not depend on wall clock.
pub fn verify_webhook(headers: &HeaderMap, body: &[u8], secret: &[u8], now: i64) -> bool {
    if !single_header(headers, "x-silicon-iam-key-version")
        .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let ts = match single_header(headers, "x-silicon-iam-timestamp")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(v) => v,
        None => return false,
    };
    if now.abs_diff(ts) > 300 {
        return false;
    }
    let signature = match single_header(headers, "x-silicon-iam-signature") {
        Some(v) => v,
        None => return false,
    };
    let digest = match signature.strip_prefix("v1=") {
        Some(v)
            if v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            v
        }
        _ => return false,
    };
    let expected = match hex::decode(digest) {
        Ok(v) if v.len() == 32 => v,
        _ => return false,
    };
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(v) => v,
        Err(_) => return false,
    };
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

pub fn verify_webhook_now(headers: &HeaderMap, body: &[u8], secret: &[u8]) -> bool {
    verify_webhook(headers, body, secret, Utc::now().timestamp())
}

/// Extract the authenticated event ID after signature verification. Test envelopes nest metadata.
pub fn webhook_event_id(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .pointer("/metadata/event_id")
        .or_else(|| value.pointer("/test/metadata/event_id"))?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_and_legacy_sessions_require_a_new_login() {
        let state = AuthState::default();
        for expires_at in [
            None,
            Some(Utc::now().timestamp() - 1),
            Some(Utc::now().timestamp() + 1800),
        ] {
            let tokens = IamTokens {
                access_token: "test".into(),
                refresh_token: "test".into(),
                expires_in: 1800,
                expires_at,
                actor: None,
                org_id: None,
                org_ids: vec!["tos".into()],
            };
            let id = state.insert(tokens).await.unwrap();
            assert_eq!(
                state.get(&id).await.is_some(),
                expires_at.is_some_and(|expiry| expiry > Utc::now().timestamp())
            );
        }
    }
    #[test]
    fn browser_state_requires_both_values_and_an_exact_match() {
        assert!(valid_login_state(
            Some("random-state"),
            Some("random-state")
        ));
        for (expected, supplied) in [
            (None, None),
            (Some("state"), None),
            (None, Some("state")),
            (Some(""), Some("")),
            (Some("state"), Some("state-prefix")),
        ] {
            assert!(!valid_login_state(expected, supplied));
        }
    }

    #[test]
    fn organizations_come_from_active_application_authorizations() {
        let mut response = serde_json::json!({
            "active":true,"client_id":"tos>starter",
            "authorizations":[{"org_id":"tos"},{"org_id":"lab"},{"org_id":"tos"}]
        });
        assert_eq!(
            authorized_organizations(&response, "tos>starter").unwrap(),
            ["lab", "tos"]
        );
        assert!(authorized_organizations(&response, "another>app").is_err());
        response["active"] = false.into();
        assert!(authorized_organizations(&response, "tos>starter").is_err());
        response = serde_json::json!({"active":true,"client_id":"tos>starter","authorization":{"org_id":"tos"}});
        assert_eq!(
            authorized_organizations(&response, "tos>starter").unwrap(),
            ["tos"]
        );
        response["authorization"] = Value::Null;
        response["org_id"] = "unverified".into();
        assert!(
            authorized_organizations(&response, "tos>starter")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn accepts_exact_signature_and_rejects_duplicate_security_headers() {
        let ts = "1000";
        let body = br#"{}"#;
        let mut mac = HmacSha256::new_from_slice(b"secret").unwrap();
        mac.update(b"1000.{}");
        let sig = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
        let mut h = HeaderMap::new();
        h.insert("x-silicon-iam-timestamp", ts.parse().unwrap());
        h.insert("x-silicon-iam-key-version", "1".parse().unwrap());
        h.insert("x-silicon-iam-signature", sig.parse().unwrap());
        assert!(verify_webhook(&h, body, b"secret", 1000));
        h.append(
            "x-silicon-iam-signature",
            "v1=0000000000000000000000000000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
        );
        assert!(!verify_webhook(&h, body, b"secret", 1000));
    }

    #[test]
    fn signature_requires_lowercase_and_rejects_far_future_without_overflow() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-silicon-iam-timestamp",
            "-9223372036854775808".parse().unwrap(),
        );
        h.insert("x-silicon-iam-signature", "v1=00".parse().unwrap());
        assert!(!verify_webhook(&h, b"{}", b"secret", 0));
    }

    #[test]
    fn extracts_production_and_testing_event_ids() {
        assert_eq!(
            webhook_event_id(br#"{"metadata":{"event_id":"e1"}}"#).as_deref(),
            Some("e1")
        );
        assert_eq!(
            webhook_event_id(br#"{"test":{"metadata":{"event_id":"e2"}}}"#).as_deref(),
            Some("e2")
        );
    }
}
