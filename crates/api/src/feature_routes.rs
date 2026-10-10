//! Durable publication helpers shared by browser and CLI requests.
use crate::{
    AppState,
    auth::SessionTokens,
    authority::{Error, Feature, Result},
    durable::FeatureStore,
};
use axum::http::{HeaderMap, StatusCode};
pub(crate) async fn feature(s: &AppState, session: &SessionTokens) -> Result<Feature> {
    let accounts = s.auth.accounts().await?;
    let store = s
        .feature_store
        .get_or_try_init(|| async { FeatureStore::from_env() })
        .await?;
    Feature::open(accounts, store, session)
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
