//! Account-bound, durable publication authority via fresh User verification proofs.
use crate::{
    accounts::Accounts,
    auth::SessionTokens,
    durable::{FeatureStore, Lease},
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Debug)]
pub(crate) struct Error {
    pub status: StatusCode,
    code: &'static str,
    message: String,
    retryable: bool,
    details: Value,
}
impl Error {
    pub(crate) fn value(&self) -> Value {
        json!({"error":{"code":self.code,"message":self.message,"retryable":self.retryable,"details":self.details}})
    }
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retryable: false,
            details: json!({}),
        }
    }
    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        let mut e = Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
            message,
        );
        e.retryable = true;
        e
    }
    pub(crate) fn permission() -> Self {
        let mut e = Self::new(
            StatusCode::FORBIDDEN,
            "briefcase_forbidden",
            "Briefcase refused access for this account. Check Starter is an allowed proof issuer and the account can write its drive.",
        );
        e.details = json!({"feature":"briefcase"});
        e
    }
    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "idempotency_conflict", message)
    }
    pub(crate) fn invalid() -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "invalid_provider_authority",
            "Silicon Accounts returned incomplete or mismatched Briefcase authority",
        )
    }
}
impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::unavailable(s)
    }
}
impl From<StatusCode> for Error {
    fn from(s: StatusCode) -> Self {
        Self::new(
            s,
            "session_unavailable",
            "The original Starter session is required",
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.status,[("cache-control","no-store")],Json(json!({"error":{"code":self.code,"message":self.message,"retryable":self.retryable,"details":self.details}}))).into_response()
    }
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
pub(crate) fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| Error::unavailable("Could not encode feature state"))
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Destination {
    pub account_uuid: String,
}
pub(crate) struct Feature {
    pub accounts: Accounts,
    pub lease: Lease,
    pub context_id: String,
    pub account_uuid: String,
    subject_token: String,
}
impl Feature {
    pub(crate) fn open(
        accounts: Accounts,
        store: &FeatureStore,
        session: &SessionTokens,
    ) -> Result<Self> {
        let account_uuid = session.account_uuid().to_owned();
        if account_uuid.is_empty() {
            return Err(Error::invalid());
        }
        let namespace = json!(["accounts-v1", accounts.app_id, account_uuid]).to_string();
        let lease = store.lease(&namespace)?;
        Ok(Self {
            accounts,
            lease,
            context_id: session.context_id.clone(),
            account_uuid,
            subject_token: session.access_token.clone(),
        })
    }
    pub(crate) fn destination(&self) -> Destination {
        Destination {
            account_uuid: self.account_uuid.clone(),
        }
    }
    pub(crate) async fn proof(&self, scope: &str) -> Result<String> {
        let proof = self
            .accounts
            .user_proof(&self.subject_token, scope)
            .await
            .map_err(|e| Error::unavailable(e.message))?;
        if proof["kind"] != "user_verification"
            || proof["issuing_app"] != self.accounts.app_id
            || proof["receiving_app"] != "briefcase"
            || proof["user"]["uuid"] != self.account_uuid
            || !proof["scopes"]
                .as_array()
                .is_some_and(|s| s.contains(&json!(scope)))
        {
            return Err(Error::invalid());
        }
        proof["proof_token"]
            .as_str()
            .filter(|s| s.starts_with("sap_"))
            .map(str::to_owned)
            .ok_or_else(Error::invalid)
    }
}
