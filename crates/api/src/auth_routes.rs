//! Browser cookies select a saved ordinary session; the public context marker fences every
//! request made by an already-mounted page. CLI requests carry their own selected secret.
use crate::*;
use axum::{
    extract::Request,
    middleware::Next,
    response::{Redirect, Response},
};
use serde_json::Value;
type Error = (StatusCode, Json<serde_json::Value>);
fn unavailable(error: String) -> Error {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error":{"code":"iam_unavailable","message":error}})),
    )
}
fn changed() -> Error {
    (
        StatusCode::CONFLICT,
        Json(
            json!({"error":{"code":"context_changed","message":"The selected account changed. Reload this page before continuing."}}),
        ),
    )
}
fn cookie(name: &str, value: &str, max_age: Option<i64>) -> HeaderValue {
    let age = max_age
        .map(|v| format!("; Max-Age={v}"))
        .unwrap_or_default();
    format!(
        "{name}={value}; HttpOnly; SameSite=Lax{}; Path=/{age}",
        secure_cookie()
    )
    .parse()
    .expect("server cookie")
}
fn session_cookies(id: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.append(
        "set-cookie",
        cookie(
            "starter_session",
            id,
            if id.is_empty() { Some(0) } else { None },
        ),
    );
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h
}
fn origin_ok(headers: &HeaderMap) -> bool {
    headers
        .get("origin")
        .is_none_or(|v| v.to_str().ok() == Some(frontend_url().as_str()))
}
pub(super) async fn context_guard(
    State(s): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/api/") && s.world.environment_id.is_some() {
        match s.auth.iam().await {
            Ok(iam) if iam.world == *s.world => {}
            _ => {
                return unavailable(
                    "The testing world changed; restart with its current configuration".into(),
                )
                .into_response();
            }
        }
    }
    let headers = request.headers();
    let path = request.uri().path();
    let protected = path.starts_with("/api/")
        || matches!(path, "/auth/context" | "/auth/contexts" | "/auth/logout");
    let marker = headers
        .get("x-starter-context")
        .and_then(|v| v.to_str().ok());
    if protected || (matches!(path, "/auth/session" | "/auth/cli/status") && marker.is_some()) {
        if headers.get_all("x-starter-session").iter().count() > 1
            || headers.get_all("x-starter-context").iter().count() > 1
        {
            return changed().into_response();
        }
        let secret = session_id(headers);
        if let Some(id) = secret {
            match s.auth.get(&id).await {
                Ok(Some(tokens)) if marker == Some(tokens.context_id.as_str()) => {}
                Ok(_) => return changed().into_response(),
                Err(error) => return unavailable(error).into_response(),
            }
        } else if marker.is_some_and(|marker| marker != "anonymous") {
            return changed().into_response();
        }
        if !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        ) && !origin_ok(headers)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
pub(super) async fn auth_login(headers: HeaderMap, State(s): State<AppState>) -> Response {
    let (nonce, group) = match s
        .auth
        .begin_browser_login(cookie_value(&headers, "starter_browser"))
        .await
    {
        Ok(v) => v,
        Err(e) => return unavailable(e).into_response(),
    };
    let callback = format!("{}/auth/callback?state={nonce}", frontend_url());
    let url = format!(
        "https://auth.iam.teamofsilicons.com/login?app_id={}&redirect_uri={}",
        urlencoding::encode(&app_id()),
        urlencoding::encode(&callback)
    );
    let mut cookies = HeaderMap::new();
    cookies.append(
        "set-cookie",
        cookie("starter_login_state", &nonce, Some(600)),
    );
    cookies.append(
        "set-cookie",
        cookie("starter_browser", &group, Some(365 * 86400)),
    );
    cookies.insert("cache-control", HeaderValue::from_static("no-store"));
    (cookies, Redirect::temporary(&url)).into_response()
}
pub(super) async fn auth_callback(
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    State(s): State<AppState>,
) -> Response {
    let Some(slt) = q.get("slt") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match browser_login(&s, &headers, slt, q.get("state").map(String::as_str)).await {
        Ok(cookies) => (cookies, Redirect::to(&frontend_url())).into_response(),
        Err(e) => e.into_response(),
    }
}
#[derive(Deserialize)]
pub(super) struct AuthCallbackBody {
    pub slt: String,
    pub state: Option<String>,
}
pub(super) async fn auth_callback_json(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<AuthCallbackBody>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match browser_login(&s, &headers, &body.slt, body.state.as_deref()).await {
        Ok(cookies) => (cookies, Json(json!({"authenticated":true}))).into_response(),
        Err(e) => e.into_response(),
    }
}
async fn browser_login(
    s: &AppState,
    headers: &HeaderMap,
    slt: &str,
    state: Option<&str>,
) -> Result<HeaderMap, Error> {
    let expected = cookie_value(headers, "starter_login_state");
    if !auth::valid_login_state(expected, state) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Login state does not match the initiating browser"})),
        ));
    }
    let group = cookie_value(headers, "starter_browser").ok_or((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"Login browser is missing"})),
    ))?;
    let id = s
        .auth
        .login(slt, Some((expected.expect("checked"), group)), None, None)
        .await
        .map_err(unavailable)?;
    // Keep the expiring nonce cookie so an uncertain callback can recover its original receipt.
    Ok(session_cookies(&id))
}
pub(super) async fn auth_cli(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let Some(slt) = body.get("slt").and_then(Value::as_str) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let org = body.get("org_id").and_then(Value::as_str);
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok());
    match s.auth.login(slt, None, key, org).await {
        Ok(id) => match s.auth.status(Some(&id)).await {
            Ok(mut status) => {
                status["session_id"] = json!(id);
                Json(status).into_response()
            }
            Err(e) => unavailable(e).into_response(),
        },
        Err(e) => unavailable(e).into_response(),
    }
}
pub(super) async fn auth_cli_status(headers: HeaderMap, State(s): State<AppState>) -> Response {
    auth_session(headers, State(s)).await
}
pub(super) async fn auth_session(headers: HeaderMap, State(s): State<AppState>) -> Response {
    match s.auth.status(session_id(&headers).as_deref()).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => unavailable(e).into_response(),
    }
}
pub(super) async fn auth_contexts(headers: HeaderMap, State(s): State<AppState>) -> Response {
    match s
        .auth
        .contexts(
            cookie_value(&headers, "starter_browser"),
            session_id(&headers).as_deref(),
        )
        .await
    {
        Ok(contexts) => Json(contexts).into_response(),
        Err(e) => unavailable(e).into_response(),
    }
}
pub(super) async fn auth_context(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    if headers.contains_key("x-starter-session") || !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(context) = body.get("context_id").and_then(Value::as_str) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match s
        .auth
        .select(cookie_value(&headers, "starter_browser"), context)
        .await
    {
        Ok(id) => (session_cookies(&id), Json(json!({"context_id":context}))).into_response(),
        Err(e) => unavailable(e).into_response(),
    }
}
pub(super) async fn auth_logout(headers: HeaderMap, State(s): State<AppState>) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(id) = session_id(&headers)
        && let Err(e) = s.auth.remove(&id).await
    {
        return unavailable(e).into_response();
    }
    let mut next = String::new();
    if !headers.contains_key("x-starter-session")
        && let Ok(contexts) = s
            .auth
            .contexts(cookie_value(&headers, "starter_browser"), None)
            .await
    {
        for row in contexts["contexts"].as_array().into_iter().flatten() {
            if let Some(context) = row["context_id"].as_str()
                && let Ok(id) = s
                    .auth
                    .select(cookie_value(&headers, "starter_browser"), context)
                    .await
            {
                next = id;
                break;
            }
        }
    }
    (
        session_cookies(&next),
        Json(json!({"authenticated":!next.is_empty()})),
    )
        .into_response()
}
