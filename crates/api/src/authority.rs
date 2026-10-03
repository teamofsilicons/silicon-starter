//! Explicit Briefcase feature consent, independently stored from ordinary login.
use crate::{
    auth::IamTokens,
    durable::{FeatureStore, Lease},
    iam::Iam,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_iam_client::{IdempotencyKey, Mutation, models};
use uuid::Uuid;

pub(crate) const ENDPOINTS: [&str; 5] = [
    "briefcase.folders.create",
    "briefcase.uploads.reserve",
    "briefcase.uploads.commit",
    "briefcase.uploads.status",
    "briefcase.link_access.update",
];

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
            "reconsent_required",
            "Briefcase permission is required. Review a new authorization in IAM, then retry this unchanged action.",
        );
        e.details = json!({"feature":"briefcase","permission_required":true});
        e
    }
    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "idempotency_conflict", message)
    }
    pub(crate) fn invalid() -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "invalid_provider_authority",
            "IAM returned incomplete or mismatched Briefcase authority",
        )
    }
    pub(crate) fn iam(error: silicon_iam_client::Error) -> Self {
        match error.api().map(|e| e.status) {
            Some(412) => {
                let mut e = Self::permission();
                e.status = StatusCode::PRECONDITION_FAILED;
                e.details["graph_changed"] = json!(true);
                e
            }
            Some(400 | 401 | 403 | 404 | 409 | 410) => Self::permission(),
            _ => Self::unavailable(
                "IAM could not complete this permission operation; retry its original request",
            ),
        }
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
fn mutation(key: &str) -> Result<Mutation> {
    IdempotencyKey::parse(key)
        .map(Mutation::with_key)
        .map_err(|_| {
            Error::new(
                StatusCode::BAD_REQUEST,
                "invalid_idempotency_key",
                "A stable Idempotency-Key is required",
            )
        })
}

#[derive(Serialize, Deserialize)]
struct Pending {
    id: Uuid,
    context_id: String,
    body: Option<models::OboAuthorizationRequest>,
    authorization: Option<models::OboConsentDetail>,
    code_hash: Option<String>,
    completed: bool,
    roots: Vec<Value>,
    #[serde(default)]
    callback: Option<BrowserCallback>,
    #[serde(default)]
    callback_code: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BrowserCallback {
    pub redirect_uri: String,
    pub state: String,
    pub return_to: String,
    pub popup: bool,
}
#[derive(Serialize, Deserialize)]
struct Root {
    pair: models::OboTokenPair,
    refresh_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Destination {
    pub org_id: String,
    pub actor: Value,
}
impl Destination {
    pub(crate) fn of(pair: &models::OboTokenPair) -> Result<Self> {
        Ok(Self {
            org_id: pair.org_id.clone(),
            actor: serde_json::to_value(pair.actor.as_ref().ok_or_else(Error::invalid)?)
                .map_err(|_| Error::invalid())?,
        })
    }
}

pub(crate) struct Feature {
    pub iam: Iam,
    pub lease: Lease,
    pub context_id: String,
    pub actor: String,
    pub org: String,
}
impl Feature {
    pub(crate) fn open(iam: Iam, store: &FeatureStore, session: &IamTokens) -> Result<Self> {
        if session.world != iam.world {
            return Err(Error::conflict(
                "This session belongs to another world generation",
            ));
        }
        let org = session
            .org_id
            .as_ref()
            .filter(|s| !s.is_empty())
            .ok_or_else(Error::invalid)?
            .clone();
        let actor = session
            .actor
            .as_ref()
            .and_then(|v| v["public_id"].as_str())
            .ok_or_else(Error::invalid)?
            .to_owned();
        let namespace = json!([iam.world.fingerprint(), org, actor]).to_string();
        let lease = store.lease(&namespace)?;
        Ok(Self {
            iam,
            lease,
            context_id: session.context_id.clone(),
            actor,
            org,
        })
    }
    fn check_detail(&self, detail: &models::OboConsentDetail, row: &Pending) -> Result<()> {
        if detail.id.is_nil()
            || detail.app_id != self.iam.app_id
            || detail.org_id != self.org
            || detail.actor.public_id != self.actor
            || detail.redirect_uri.as_deref()
                != row.callback.as_ref().map(|c| c.redirect_uri.as_str())
            || detail.state.as_deref() != row.callback.as_ref().map(|c| c.state.as_str())
        {
            return Err(Error::invalid());
        }
        let actor = serde_json::to_value(&detail.actor).map_err(|_| Error::invalid())?;
        if !crate::auth::valid_actor_id(
            actor["type"].as_str().unwrap_or_default(),
            &detail.actor.public_id,
        ) {
            return Err(Error::invalid());
        }
        if let Some(url) = &detail.authorization_url {
            let u = url::Url::parse(url).map_err(|_| Error::invalid())?;
            let request_ids: Vec<_> = u
                .query_pairs()
                .filter(|(key, _)| key == "request")
                .map(|(_, value)| value.into_owned())
                .collect();
            if u.origin().ascii_serialization() != "https://auth.iam.teamofsilicons.com"
                || u.path() != "/obo/consent"
                || request_ids != [detail.id.to_string()]
                || u.query_pairs()
                    .any(|(key, _)| matches!(key.as_ref(), "app_id" | "app_ids" | "bundle_id"))
                || u.fragment().is_some()
                || !u.username().is_empty()
                || u.password().is_some()
            {
                return Err(Error::invalid());
            }
        }
        Ok(())
    }
    fn request(&self, id: Uuid) -> Result<Pending> {
        let row: Pending = self.lease.get("request", &id.to_string())?.ok_or_else(|| {
            Error::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Permission request is unavailable in this context",
            )
        })?;
        if row.context_id != self.context_id {
            return Err(Error::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "Permission request is unavailable in this context",
            ));
        }
        Ok(row)
    }
    fn view(&self, row: &Pending) -> Value {
        json!({"request_id":row.id,"attempt_id":row.id,"context_id":row.context_id,"authorization":row.authorization,"completed":row.completed,"roots":row.roots})
    }
    pub(crate) async fn start(&self, session: &IamTokens, key: &str) -> Result<Value> {
        self.start_browser(session, key, None).await
    }
    pub(crate) async fn start_browser(
        &self,
        session: &IamTokens,
        key: &str,
        browser: Option<(String, bool)>,
    ) -> Result<Value> {
        mutation(key)?;
        let key = digest(&encode(&(self.context_id.as_str(), key))?);
        let mut row = if let Some(id) = self.lease.get::<Uuid>("request-key", &key)? {
            let row = self.request(id)?;
            if row.callback.as_ref().map(|c| (&c.return_to, c.popup))
                != browser.as_ref().map(|(url, popup)| (url, *popup))
            {
                return Err(Error::conflict(
                    "Retry the original permission return destination",
                ));
            }
            row
        } else {
            let id = Uuid::new_v4();
            let callback = browser.map(|(return_to, popup)| BrowserCallback {
                redirect_uri: format!(
                    "{}/auth/briefcase/callback?request_id={id}",
                    crate::frontend_url()
                ),
                state: Uuid::new_v4().to_string(),
                return_to,
                popup,
            });
            let row = Pending {
                id,
                context_id: self.context_id.clone(),
                body: Some(models::OboAuthorizationRequest {
                    subject_token: session.access_token.clone(),
                    org_id: self.org.clone(),
                    redirect_uri: callback.as_ref().map(|c| c.redirect_uri.clone()),
                    state: callback.as_ref().map(|c| c.state.clone()),
                    endpoints: ENDPOINTS
                        .iter()
                        .map(|e| models::OboAuthorizationEndpoint {
                            audience: "briefcase".into(),
                            endpoint_id: (*e).into(),
                        })
                        .collect(),
                }),
                authorization: None,
                code_hash: None,
                completed: false,
                roots: vec![],
                callback,
                callback_code: None,
            };
            self.lease.put_many(vec![
                ("request-key".into(), key, encode(&id)?),
                ("request".into(), id.to_string(), encode(&row)?),
            ])?;
            row
        };
        if row.authorization.is_none() {
            let body = row.body.as_ref().ok_or_else(Error::invalid)?;
            let detail = self
                .iam
                .client
                .obo()
                .authorize(
                    body,
                    &mutation(&format!("starter-briefcase-start-{}", row.id))?,
                )
                .await
                .map_err(Error::iam)?;
            self.check_detail(&detail, &row)?;
            row.authorization = Some(detail);
            row.body = None;
            self.lease.put("request", &row.id.to_string(), &row)?;
        }
        Ok(self.view(&row))
    }
    pub(crate) async fn status(&self, id: Uuid) -> Result<Value> {
        let mut row = self.request(id)?;
        let auth = row
            .authorization
            .as_ref()
            .ok_or_else(|| Error::unavailable("Retry the original permission-start request"))?
            .id;
        let detail = self
            .iam
            .client
            .obo()
            .authorization(auth)
            .await
            .map_err(Error::iam)?;
        self.check_detail(&detail, &row)?;
        if detail.id != auth {
            return Err(Error::invalid());
        }
        row.authorization = Some(detail);
        Ok(self.view(&row))
    }
    pub(crate) async fn complete(&self, id: Uuid, code: &str) -> Result<Value> {
        if code.is_empty() || code.len() > 1024 || !code.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::new(
                StatusCode::BAD_REQUEST,
                "invalid_code",
                "Paste the code displayed by IAM",
            ));
        }
        let mut row = self.request(id)?;
        let hash = digest(code.as_bytes());
        if row.code_hash.as_ref().is_some_and(|old| old != &hash) {
            return Err(Error::conflict("Retry this request with its original code"));
        }
        if row.completed {
            return Ok(self.view(&row));
        }
        let auth = row.authorization.as_ref().ok_or_else(Error::invalid)?.id;
        let detail = self
            .iam
            .client
            .obo()
            .authorization(auth)
            .await
            .map_err(Error::iam)?;
        self.check_detail(&detail, &row)?;
        if detail.id != auth
            || !matches!(
                detail.status,
                models::OboConsentDetailStatus::Approved
                    | models::OboConsentDetailStatus::Exchanged
            )
        {
            return Err(Error::permission());
        }
        row.code_hash = Some(hash);
        self.lease.put("request", &row.id.to_string(), &row)?;
        let response = self
            .iam
            .client
            .obo()
            .exchange_code(
                auth,
                code,
                &mutation(&format!("starter-briefcase-code-{}", row.id))?,
            )
            .await
            .map_err(Error::iam)?;
        if response.items.len() != ENDPOINTS.len() {
            return Err(Error::invalid());
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut destination = None;
        let mut updates = vec![];
        for pair in response.items {
            self.validate_pair(&pair).await?;
            if !seen.insert(pair.endpoint_id.clone()) {
                return Err(Error::invalid());
            }
            let next = Destination::of(&pair)?;
            if destination.as_ref().is_some_and(|d| d != &next) {
                return Err(Error::invalid());
            }
            destination = Some(next);
            row.roots.push(json!({"audience":pair.audience,"endpoint_id":pair.endpoint_id,"grant_id":pair.grant_id,"actor":pair.actor,"org_id":pair.org_id,"expires_at":pair.expires_at.unix_timestamp()}));
            updates.push((
                "root".into(),
                pair.endpoint_id.clone(),
                encode(&Root {
                    pair,
                    refresh_key: None,
                })?,
            ));
        }
        row.authorization = Some(detail);
        row.completed = true;
        row.callback_code = None;
        updates.push(("request".into(), row.id.to_string(), encode(&row)?));
        self.lease.put_many(updates)?;
        Ok(self.view(&row))
    }
    pub(crate) fn browser_callback(
        &self,
        id: Uuid,
        authorization_id: Uuid,
        state: &str,
    ) -> Result<BrowserCallback> {
        let row = self.request(id)?;
        let callback = row.callback.ok_or_else(Error::invalid)?;
        if callback.state != state
            || row.authorization.as_ref().map(|a| a.id) != Some(authorization_id)
        {
            return Err(Error::new(
                StatusCode::FORBIDDEN,
                "invalid_callback",
                "The permission callback does not match its original review",
            ));
        }
        Ok(callback)
    }
    pub(crate) async fn complete_browser(
        &self,
        id: Uuid,
        authorization_id: Uuid,
        state: &str,
        code: Option<&str>,
    ) -> Result<Value> {
        self.browser_callback(id, authorization_id, state)?;
        let mut row = self.request(id)?;
        if row.completed {
            return Ok(self.view(&row));
        }
        if let Some(code) = code {
            if code.is_empty() || code.len() > 1024 || !code.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(Error::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_code",
                    "IAM returned an invalid permission code",
                ));
            }
            if row
                .callback_code
                .as_deref()
                .is_some_and(|saved| saved != code)
            {
                return Err(Error::conflict(
                    "Retry this callback with its original code",
                ));
            }
            row.callback_code = Some(code.to_owned());
            self.lease.put("request", &id.to_string(), &row)?;
        }
        let code = row.callback_code.ok_or_else(|| {
            Error::new(
                StatusCode::BAD_REQUEST,
                "missing_code",
                "The IAM permission code is missing",
            )
        })?;
        self.complete(id, &code).await
    }
    async fn validate_pair(&self, pair: &models::OboTokenPair) -> Result<()> {
        let actor = pair.actor.as_ref().ok_or_else(Error::invalid)?;
        let value = serde_json::to_value(actor).map_err(|_| Error::invalid())?;
        if pair.audience != "briefcase"
            || !ENDPOINTS.contains(&pair.endpoint_id.as_str())
            || pair.grant_id.is_nil()
            || !token(&pair.access_token, "oba_")
            || !token(&pair.refresh_token, "obr_")
            || pair.token_type != models::OboTokenPairTokenType::Bearer
            || pair.expires_in <= 0
            || pair.expires_at.unix_timestamp() <= chrono::Utc::now().timestamp()
            || pair.org_id.is_empty()
            || !crate::auth::valid_actor_id(
                value["type"].as_str().unwrap_or_default(),
                &actor.public_id,
            )
            || !pair
                .scope
                .split_whitespace()
                .any(|s| s == format!("obo:briefcase:{}", pair.endpoint_id))
        {
            return Err(Error::invalid());
        }
        self.iam
            .validate_provider_context(pair.testing_context.as_ref())
            .await
            .map_err(|_| Error::invalid())
    }
    pub(crate) async fn root(&self, endpoint: &str, force: bool) -> Result<models::OboTokenPair> {
        let mut root: Root = self
            .lease
            .get("root", endpoint)?
            .ok_or_else(Error::permission)?;
        if force
            || root.pair.expires_at.unix_timestamp() <= chrono::Utc::now().timestamp() + 60
            || root.refresh_key.is_some()
        {
            let key = root
                .refresh_key
                .get_or_insert_with(|| format!("starter-briefcase-refresh-{}", Uuid::new_v4()))
                .clone();
            self.lease.put("root", endpoint, &root)?;
            let mut response = self
                .iam
                .client
                .obo()
                .refresh(&root.pair.refresh_token, &mutation(&key)?)
                .await
                .map_err(Error::iam)?;
            if response.items.len() != 1 {
                return Err(Error::invalid());
            }
            let next = response.items.remove(0);
            self.validate_pair(&next).await?;
            if next.grant_id != root.pair.grant_id
                || next.endpoint_id != endpoint
                || Destination::of(&next)? != Destination::of(&root.pair)?
            {
                return Err(Error::invalid());
            }
            root = Root {
                pair: next,
                refresh_key: None,
            };
            self.lease.put("root", endpoint, &root)?;
        }
        self.validate_pair(&root.pair).await?;
        Ok(root.pair)
    }
}
fn token(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() < 8192
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
