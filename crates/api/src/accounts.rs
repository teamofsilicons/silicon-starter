//! Silicon Accounts' documented OAuth and User verification endpoints.
use reqwest::Client;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct Accounts {
    http: Client,
    pub app_id: String,
    pub base_url: String,
    secret: String,
}
#[derive(Debug)]
pub struct Error {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl From<Error> for String {
    fn from(value: Error) -> Self {
        value.message
    }
}
impl Accounts {
    pub async fn from_env() -> Result<Self, String> {
        let base_url = std::env::var("ACCOUNTS_URL")
            .unwrap_or_else(|_| "https://accounts.teamofsilicons.com".into());
        validate_url(&base_url)?;
        let app_id = std::env::var("STARTER_ACCOUNTS_APP_ID")
            .or_else(|_| std::env::var("ACCOUNTS_APP_ID"))
            .unwrap_or_else(|_| "starter".into());
        crate::auth::validate_app_id(&app_id)?;
        let secret = std::env::var("STARTER_ACCOUNTS_APP_SECRET")
            .or_else(|_| std::env::var("ACCOUNTS_APP_SECRET"))
            .map_err(|_| "STARTER_ACCOUNTS_APP_SECRET is not configured")?;
        if secret.is_empty() {
            return Err("STARTER_ACCOUNTS_APP_SECRET is empty".into());
        }
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(35))
                .user_agent(concat!("silicon-starter/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|_| "Cannot initialize Silicon Accounts")?,
            app_id,
            base_url: base_url.trim_end_matches('/').into(),
            secret,
        })
    }
    async fn post(&self, path: &str, body: &Value, key: Option<&str>) -> Result<Value, Error> {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base_url))
            .basic_auth(&self.app_id, Some(&self.secret))
            .json(body);
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        let response = request.send().await.map_err(|e| Error {
            code: if e.is_connect() {
                "accounts_connection_failed"
            } else {
                "accounts_unavailable"
            }
            .into(),
            message: "Silicon Accounts could not be reached".into(),
        })?;
        let status = response.status();
        let value: Value = response.json().await.map_err(|_| Error {
            code: "accounts_unavailable".into(),
            message: "Silicon Accounts returned an unreadable response".into(),
        })?;
        if !status.is_success() {
            return Err(Error {
                code: value["error"]
                    .as_str()
                    .or_else(|| value["error"]["code"].as_str())
                    .unwrap_or("accounts_unavailable")
                    .into(),
                message: value["error_description"]
                    .as_str()
                    .or_else(|| value["error"]["message"].as_str())
                    .unwrap_or("Silicon Accounts could not complete this request")
                    .into(),
            });
        }
        Ok(value)
    }
    pub async fn exchange_slt(&self, slt: &str) -> Result<Value, Error> {
        self.post(
            "/v1/oauth/token",
            &json!({"grant_type":"urn:silicon:params:oauth:grant-type:slt","slt":slt}),
            None,
        )
        .await
    }
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<Value, Error> {
        self.post("/v1/oauth/token", &json!({"grant_type":"authorization_code","code":code,"redirect_uri":redirect_uri,"code_verifier":verifier}), None).await
    }
    pub async fn device_start(&self) -> Result<Value, Error> {
        self.post(
            "/v1/device/authorize",
            &json!({"client_id":self.app_id,"client_label":"Silicon Starter CLI"}),
            None,
        )
        .await
    }
    pub async fn device_complete(&self, code: &str) -> Result<Value, Error> {
        self.post("/v1/oauth/token", &json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code","device_code":code}), None).await
    }
    pub async fn refresh(&self, token: &str) -> Result<Value, Error> {
        self.post(
            "/v1/oauth/token",
            &json!({"grant_type":"refresh_token","refresh_token":token}),
            None,
        )
        .await
    }
    pub async fn introspect(&self, token: &str) -> Result<Value, Error> {
        self.post("/v1/oauth/introspect", &json!({"token":token}), None)
            .await
    }
    pub async fn revoke(&self, token: &str) -> Result<(), Error> {
        self.post("/v1/oauth/revoke", &json!({"token":token}), None)
            .await
            .map(|_| ())
    }
    pub async fn user_proof(&self, subject: &str, scope: &str) -> Result<Value, Error> {
        self.post("/v1/proofs/user-verification", &json!({"subject_token":subject,"receiving_app":"briefcase","scopes":[scope],"access_ttl_seconds":120}), Some(&uuid::Uuid::new_v4().to_string())).await
    }
}
pub fn validate_url(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid service URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Service URLs require HTTPS, or HTTP on loopback".into());
    }
    Ok(())
}
