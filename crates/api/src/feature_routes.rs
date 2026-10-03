//! Authenticated manual-code feature controls. Browser/CLI context fencing is
//! shared with all Starter routes; none of these endpoints approves IAM consent.
use crate::{
    AppState,
    auth::IamTokens,
    authenticated_session,
    authority::{Error, Feature, Result},
    durable::FeatureStore,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

pub(crate) async fn feature(s: &AppState, session: &IamTokens) -> Result<Feature> {
    let iam = s.auth.iam().await?;
    let store = s
        .feature_store
        .get_or_try_init(|| async { FeatureStore::from_env() })
        .await?;
    Feature::open(iam, store, session)
}
pub(crate) fn key(headers: &HeaderMap) -> Result<&str> {
    let mut values = headers.get_all("idempotency-key").iter();
    let key = values
        .next()
        .and_then(|v| v.to_str().ok())
        .filter(|k| k.len() >= 16 && k.len() <= 255 && k.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| {
            Error::new(
                StatusCode::BAD_REQUEST,
                "idempotency_key_required",
                "Retain an Idempotency-Key of 16–255 visible ASCII characters for this action",
            )
        })?;
    if values.next().is_some() {
        return Err(Error::new(
            StatusCode::BAD_REQUEST,
            "invalid_idempotency_key",
            "Send one Idempotency-Key",
        ));
    }
    Ok(key)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Complete {
    code: String,
}
pub(crate) async fn start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(_): Json<Empty>,
) -> Result<Json<Value>> {
    let session = authenticated_session(&s, &headers).await?;
    let feature = feature(&s, &session).await?;
    Ok(Json(feature.start(&session, key(&headers)?).await?))
}
pub(crate) async fn status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    let session = authenticated_session(&s, &headers).await?;
    Ok(Json(feature(&s, &session).await?.status(id).await?))
}
pub(crate) async fn complete(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<Complete>,
) -> Result<Json<Value>> {
    key(&headers)?;
    let session = authenticated_session(&s, &headers).await?;
    Ok(Json(
        feature(&s, &session)
            .await?
            .complete(id, &body.code)
            .await?,
    ))
}
