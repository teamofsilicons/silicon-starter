//! Authenticated browser and manual-code feature controls. Context fencing is
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
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
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
pub(crate) struct Start {
    #[serde(default)]
    popup: Option<bool>,
    #[serde(default)]
    return_to: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Complete {
    code: String,
}
pub(crate) async fn start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Start>,
) -> Result<Json<Value>> {
    let session = authenticated_session(&s, &headers).await?;
    let feature = feature(&s, &session).await?;
    let response = if let Some(popup) = body.popup {
        let return_to = crate::auth_routes::validated_return(body.return_to.as_deref())
            .map_err(|e| Error::new(StatusCode::BAD_REQUEST, "invalid_return_to", e))?;
        feature
            .start_browser(&session, key(&headers)?, Some((return_to, popup)))
            .await?
    } else {
        if body.return_to.is_some() {
            return Err(Error::new(
                StatusCode::BAD_REQUEST,
                "invalid_callback",
                "A browser return destination requires popup mode",
            ));
        }
        feature.start(&session, key(&headers)?).await?
    };
    Ok(Json(response))
}

#[derive(Deserialize)]
pub(crate) struct Callback {
    request_id: Uuid,
    authorization_id: Uuid,
    state: String,
    code: Option<String>,
    error: Option<String>,
}
pub(crate) async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Result<Response> {
    // Browser navigation has no context header. The current cookie plus the
    // encrypted request's account/context/state binding must all agree.
    let id = crate::cookie_value(&headers, "starter_session").ok_or(StatusCode::UNAUTHORIZED)?;
    let session = s.auth.get(id).await?.ok_or(StatusCode::UNAUTHORIZED)?;
    let feature = feature(&s, &session).await?;
    let bound = feature.browser_callback(q.request_id, q.authorization_id, &q.state)?;
    let result = if q.error.is_some() {
        Err(Error::permission())
    } else {
        feature
            .complete_browser(
                q.request_id,
                q.authorization_id,
                &q.state,
                q.code.as_deref(),
            )
            .await
    };
    let completed = result.is_ok();
    let retry = format!(
        "/auth/briefcase/callback?request_id={}&authorization_id={}&state={}",
        q.request_id,
        q.authorization_id,
        urlencoding::encode(&q.state)
    );
    // Only a status/attempt marker reaches the opener. An uncertain exchange
    // retains its encrypted code and original mutation key for this retry URL.
    let message = serde_json::json!({"type":"starter:briefcase","attempt_id":q.request_id,"status":if completed {"complete"} else {"error"}});
    let js = |v: &Value| {
        serde_json::to_string(v)
            .expect("JSON value")
            .replace('<', "\\u003c")
    };
    let origin = url::Url::parse(&crate::frontend_url())
        .map_err(|_| Error::unavailable("Invalid frontend URL"))?
        .origin()
        .ascii_serialization();
    let status = if completed {
        "Storage access is ready. Return to your pending publication."
    } else {
        "Storage access was not completed. Your pending publication is preserved."
    };
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=referrer content=no-referrer><title>Starter storage permission</title><p>{status}</p><p><a id=return>Return to Starter</a></p><p><a id=retry>Retry this authorization</a></p><script>history.replaceState(null,'','/auth/briefcase/callback');document.getElementById('return').href={return_to};const retry=document.getElementById('retry');retry.href={retry};retry.hidden={hide_retry};if({popup}&&window.opener){{window.opener.postMessage({message},{origin});if({completed})window.close();}}else if({completed}){{location.replace({return_to});}}</script>",
        return_to = js(&Value::String(bound.return_to)),
        retry = js(&Value::String(retry)),
        hide_retry = completed || q.error.is_some(),
        popup = bound.popup,
        message = js(&message),
        origin = js(&Value::String(origin))
    );
    Ok((
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
        ],
        Html(html),
    )
        .into_response())
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
