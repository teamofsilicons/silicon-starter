//! Small Briefcase OBO storage adapter for published starter bundles.
//!
//! Every request obtains a fresh IAM proof bound to its exact method, path and
//! body digest. Proofs and application credentials never appear in errors.

use hmac::{Hmac, Mac};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;
const IAM_DEFAULT: &str = "https://backend.iam.teamofsilicons.com";
const BRIEFCASE_DEFAULT: &str = "https://backend.briefcase.teamofsilicons.com";

#[derive(Clone)]
pub struct BriefcaseStorage {
    http: Client,
    iam_base: String,
    briefcase_base: String,
    pub app_id: String,
    app_secret: String,
    org_id: Option<String>,
    subject_token: String,
}

impl BriefcaseStorage {
    /// Build from deployment environment and the caller's IAM access token.
    pub fn from_env(subject_token: impl Into<String>) -> Result<Self, String> {
        let required =
            |name: &str| std::env::var(name).map_err(|_| format!("{name} is not configured"));
        Ok(Self {
            http: Client::new(),
            iam_base: std::env::var("IAM_URL").unwrap_or_else(|_| IAM_DEFAULT.into()),
            briefcase_base: std::env::var("BRIEFCASE_URL")
                .unwrap_or_else(|_| BRIEFCASE_DEFAULT.into()),
            app_id: std::env::var("BRIEFCASE_APP_ID").unwrap_or_else(|_| "tos>starter".into()),
            app_secret: required("BRIEFCASE_APP_SECRET")?,
            org_id: std::env::var("BRIEFCASE_ORG_ID").ok(),
            subject_token: subject_token.into(),
        })
    }

    /// Create `public/starters/<org>/<starter>/<version>` one folder at a time.
    pub async fn ensure_release_path(
        &self,
        org: &str,
        starter: &str,
        version: &str,
    ) -> Result<String, String> {
        let mut parent = "public".to_string();
        for name in ["starters", org, starter, version] {
            let body = json!({
                "operation_id": Uuid::now_v7(),
                "parent_path": parent,
                "name": name,
            });
            let entry = self
                .obo_json(
                    "briefcase.folders.create",
                    "/api/v1/obo/folders/create",
                    body,
                )
                .await?;
            parent = entry
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{parent}/{name}"));
        }
        Ok(parent)
    }

    /// Upload an archive or git directory export and make it publicly readable.
    pub async fn upload_public_bundle(
        &self,
        parent_path: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<Value, String> {
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let metadata =
            json!({"path": parent_path, "name": name, "content_type": "application/octet-stream"});
        let proof = self
            .exchange(
                "briefcase.files.create",
                "POST",
                "/api/v1/obo/files",
                &digest,
                metadata,
            )
            .await?;
        let mut request = self
            .http
            .post(self.url("/api/v1/obo/files"))
            .header("X-App-ID", &self.app_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .body(bytes);
        if let Some(org) = &self.org_id {
            request = request.header("X-Org-ID", org);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Briefcase upload request failed".to_string())?;
        self.json_response(response, StatusCode::CREATED).await
    }

    /// Read the current bytes for a Briefcase file entry.
    pub async fn read_file(&self, entry_id: Uuid) -> Result<Vec<u8>, String> {
        let body = json!({"entry_id": entry_id, "download": true});
        let proof = self
            .exchange(
                "briefcase.files.read",
                "POST",
                "/api/v1/obo/files/read",
                &self.digest(&body),
                json!({}),
            )
            .await?;
        let mut request = self
            .http
            .post(self.url("/api/v1/obo/files/read"))
            .header("X-App-ID", &self.app_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .json(&body);
        if let Some(org) = &self.org_id {
            request = request.header("X-Org-ID", org);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Briefcase read request failed".to_string())?;
        if !response.status().is_success() {
            return Err(format!("Briefcase read returned {}", response.status()));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|_| "Briefcase read body failed".into())
    }

    /// Enable anyone-with-link read access for a published release folder/file.
    pub async fn set_public_link(&self, entry_id: Uuid) -> Result<Value, String> {
        let body = json!({"operation_id": Uuid::now_v7(), "entry_id": entry_id, "enabled": true});
        self.obo_json(
            "briefcase.link_access.update",
            "/api/v1/obo/link-access",
            body,
        )
        .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.briefcase_base.trim_end_matches('/'), path)
    }
    fn digest(&self, body: &Value) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(body).unwrap_or_default())
        )
    }

    async fn obo_json(&self, endpoint: &str, path: &str, body: Value) -> Result<Value, String> {
        let bytes = serde_json::to_vec(&body)
            .map_err(|_| "Briefcase request serialization failed".to_string())?;
        let proof = self
            .exchange(
                endpoint,
                "POST",
                path,
                &format!("{:x}", Sha256::digest(&bytes)),
                json!({}),
            )
            .await?;
        let mut request = self
            .http
            .post(self.url(path))
            .header("X-App-ID", &self.app_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .header("Content-Type", "application/json")
            .body(bytes);
        if let Some(org) = &self.org_id {
            request = request.header("X-Org-ID", org);
        }
        self.json_response(
            request
                .send()
                .await
                .map_err(|_| "Briefcase request failed".to_string())?,
            StatusCode::OK,
        )
        .await
    }

    async fn exchange(
        &self,
        endpoint: &str,
        method: &str,
        path: &str,
        body_sha256: &str,
        metadata: Value,
    ) -> Result<String, String> {
        let key = Uuid::now_v7().to_string();
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let signing = format!("{timestamp}.{method}.{path}.{body_sha256}.{key}");
        let mut mac = HmacSha256::new_from_slice(self.app_secret.as_bytes())
            .map_err(|_| "invalid Briefcase app secret".to_string())?;
        mac.update(signing.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let body = json!({"subject_token": self.subject_token, "audience": "tos>briefcase", "endpoint_id": endpoint, "metadata": metadata, "request": {"method": method, "body_sha256": body_sha256}});
        let response = self
            .http
            .post(format!(
                "{}/api/v1/obo-access/exchanges",
                self.iam_base.trim_end_matches('/')
            ))
            .basic_auth(&self.app_id, Some(&self.app_secret))
            .header("Idempotency-Key", key)
            .header("X-OBO-Timestamp", timestamp)
            .header("X-OBO-Signature", signature)
            .json(&body)
            .send()
            .await
            .map_err(|_| "IAM OBO exchange failed".to_string())?;
        if !response.status().is_success() {
            return Err(format!("IAM OBO exchange returned {}", response.status()));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| "IAM OBO response was invalid".to_string())?;
        value
            .get("access_proof")
            .and_then(Value::as_str)
            .filter(|proof| !proof.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "IAM OBO response omitted access proof".into())
    }

    async fn json_response(
        &self,
        response: reqwest::Response,
        expected: StatusCode,
    ) -> Result<Value, String> {
        let status = response.status();
        if status != expected && !status.is_success() {
            return Err(format!("Briefcase returned {status}"));
        }
        response
            .json()
            .await
            .map_err(|_| "Briefcase response was invalid".into())
    }
}
