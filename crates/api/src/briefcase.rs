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
    org_id: String,
    subject_token: String,
}

impl BriefcaseStorage {
    /// Build from deployment environment and the caller's IAM access token.
    pub fn from_env(subject_token: impl Into<String>, org_id: &str) -> Result<Self, String> {
        let required =
            |name: &str| std::env::var(name).map_err(|_| format!("{name} is not configured"));
        // BRIEFCASE_APP_ID is the calling application's credential ID, not its audience.
        let app_id = std::env::var("BRIEFCASE_APP_ID")
            .or_else(|_| std::env::var("STARTER_IAM_APP_ID"))
            .unwrap_or_else(|_| "starter".into());
        crate::auth::validate_app_id(&app_id)?;
        if org_id.is_empty() {
            return Err("Briefcase requires an explicitly authorized organization".into());
        }
        Ok(Self {
            http: Client::new(),
            iam_base: std::env::var("IAM_URL").unwrap_or_else(|_| IAM_DEFAULT.into()),
            briefcase_base: std::env::var("BRIEFCASE_URL")
                .unwrap_or_else(|_| BRIEFCASE_DEFAULT.into()),
            app_id,
            app_secret: std::env::var("BRIEFCASE_APP_SECRET")
                .or_else(|_| required("STARTER_IAM_APP_SECRET"))?,
            org_id: org_id.into(),
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
        let request = self
            .http
            .post(self.url("/api/v1/obo/files"))
            .header("X-App-ID", &self.app_id)
            .header("X-Org-ID", &self.org_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .body(bytes);
        let response = request
            .send()
            .await
            .map_err(|_| "Briefcase upload request failed".to_string())?;
        self.json_response(response, StatusCode::CREATED).await
    }

    /// Read the current bytes for a Briefcase file entry.
    #[allow(dead_code)]
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
        let request = self
            .http
            .post(self.url("/api/v1/obo/files/read"))
            .header("X-App-ID", &self.app_id)
            .header("X-Org-ID", &self.org_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .json(&body);
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
        let request = self
            .http
            .post(self.url(path))
            .header("X-App-ID", &self.app_id)
            .header("X-Org-ID", &self.org_id)
            .header("X-IAM-OBO-Access-Proof", proof)
            .header("Content-Type", "application/json")
            .body(bytes);
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
        let body = json!({"subject_token": self.subject_token, "audience": "briefcase", "org_id": self.org_id, "endpoint_id": endpoint, "metadata": metadata, "request": {"method": method, "body_sha256": body_sha256}});
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, body::Bytes, http::HeaderMap, routing::post};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn canonical_obo_binds_selected_organization_and_exact_downstream_bytes() {
        let exchanges = Arc::new(Mutex::new(Vec::<Value>::new()));
        let received = exchanges.clone();
        let app = Router::new()
            .route(
                "/api/v1/obo-access/exchanges",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let received = received.clone();
                    async move {
                        assert_eq!(
                            headers["authorization"],
                            format!("Basic {}", STANDARD.encode("starter:secret"))
                        );
                        assert!(!headers.contains_key("x-org-id"));
                        assert_eq!(body["audience"], "briefcase");
                        assert_eq!(body["endpoint_id"], "briefcase.link_access.update");
                        assert_eq!(body["request"]["method"], "POST");
                        let signature = format!(
                            "{}.POST./api/v1/obo/link-access.{}.{}",
                            headers["x-obo-timestamp"].to_str().unwrap(),
                            body["request"]["body_sha256"].as_str().unwrap(),
                            headers["idempotency-key"].to_str().unwrap()
                        );
                        let mut mac = HmacSha256::new_from_slice(b"secret").unwrap();
                        mac.update(signature.as_bytes());
                        mac.verify_slice(
                            &hex::decode(headers["x-obo-signature"].to_str().unwrap()).unwrap(),
                        )
                        .unwrap();
                        received.lock().await.push(body);
                        Json(json!({"access_proof":"single-use-proof"}))
                    }
                }),
            )
            .route(
                "/api/v1/obo/link-access",
                post({
                    let exchanges = exchanges.clone();
                    move |headers: HeaderMap, bytes: Bytes| {
                        let exchanges = exchanges.clone();
                        async move {
                            let records = exchanges.lock().await;
                            let exchange = records.last().unwrap();
                            assert_eq!(headers["x-app-id"], "starter");
                            assert_eq!(headers["x-iam-obo-access-proof"], "single-use-proof");
                            assert_eq!(
                                headers["x-org-id"].to_str().unwrap(),
                                exchange["org_id"].as_str().unwrap()
                            );
                            assert_eq!(
                                exchange["request"]["body_sha256"],
                                format!("{:x}", Sha256::digest(&bytes))
                            );
                            let body: Value = serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(body["enabled"], true);
                            Json(body)
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for org in ["tos", "lab"] {
            let storage = BriefcaseStorage {
                http: Client::new(),
                iam_base: base.clone(),
                briefcase_base: base.clone(),
                app_id: "starter".into(),
                app_secret: "secret".into(),
                org_id: org.into(),
                subject_token: "opaque-token".into(),
            };
            let entry = Uuid::now_v7();
            assert_eq!(
                storage.set_public_link(entry).await.unwrap()["entry_id"],
                entry.to_string()
            );
        }
        let exchanges = exchanges.lock().await;
        assert_eq!(exchanges.len(), 2);
        assert_eq!(exchanges[0]["org_id"], "tos");
        assert_eq!(exchanges[1]["org_id"], "lab");
        assert!(
            exchanges
                .iter()
                .all(|body| body["subject_token"] == "opaque-token")
        );
        server.abort();
    }
}
