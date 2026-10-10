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
        Json(json!({"error":{"code":"accounts_unavailable","message":error}})),
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
fn session_cookies(id: &str, expires_at: i64) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.append(
        "set-cookie",
        cookie(
            "starter_session",
            id,
            Some(if id.is_empty() {
                0
            } else {
                (expires_at - chrono::Utc::now().timestamp()).max(0)
            }),
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
                Ok(Some(_)) => return changed().into_response(),
                Ok(None) => return (StatusCode::UNAUTHORIZED, Json(json!({"error":{"code":"session_expired","message":"This sign-in has expired or was revoked. Sign in again."}}))).into_response(),
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
#[derive(Deserialize)]
pub(super) struct LoginOptions {
    identity_kind: String,
    return_to: Option<String>,
    #[serde(default)]
    popup: bool,
}
pub(super) fn validated_return(value: Option<&str>) -> Result<String, String> {
    let base = url::Url::parse(&frontend_url()).map_err(|_| "Invalid application origin")?;
    let value = value.unwrap_or("/");
    if value.starts_with("//") || value.contains('\\') || value.chars().any(char::is_control) {
        return Err("Return destination must stay within this application".into());
    }
    let target = base.join(value).map_err(|_| "Invalid return destination")?;
    if target.origin() != base.origin()
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return Err("Return destination must stay within this application".into());
    }
    Ok(target.into())
}
async fn start_login(
    s: &AppState,
    headers: &HeaderMap,
    options: LoginOptions,
) -> Result<(HeaderMap, Value), Error> {
    if options.identity_kind != "carbon" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Silicons sign in with an SLT; hosted sign-in is for Carbons"})),
        ));
    }
    let return_to = validated_return(options.return_to.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error":e}))))?;
    let (nonce, group) = s
        .auth
        .begin_browser_login(
            cookie_value(headers, "starter_browser"),
            &options.identity_kind,
            return_to,
            options.popup,
        )
        .await
        .map_err(unavailable)?;
    let attempt = s
        .auth
        .browser_attempt(&nonce, &group)
        .await
        .map_err(unavailable)?;
    let callback = format!("{}/auth/callback", frontend_url());
    let accounts = s.auth.accounts().await.map_err(unavailable)?;
    let mut url =
        url::Url::parse(&format!("{}/authorize", accounts.base_url)).expect("Accounts URL");
    url.query_pairs_mut()
        .append_pair("app_id", &accounts.app_id)
        .append_pair("redirect_uri", &callback)
        .append_pair("response_type", "code")
        .append_pair("state", &nonce)
        .append_pair("code_challenge", &attempt.challenge)
        .append_pair("code_challenge_method", "S256");
    let mut cookies = HeaderMap::new();
    cookies.append(
        "set-cookie",
        cookie("starter_login_state", &nonce, Some(3600)),
    );
    cookies.append(
        "set-cookie",
        cookie("starter_browser", &group, Some(900 * 86400)),
    );
    cookies.insert("cache-control", HeaderValue::from_static("no-store"));
    Ok((
        cookies,
        json!({"attempt_id":attempt.attempt_id,"login_url":url.as_str(),"accounts_origin":url.origin().ascii_serialization()}),
    ))
}
pub(super) async fn auth_attempt(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(options): Json<LoginOptions>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match start_login(&s, &headers, options).await {
        Ok((cookies, attempt)) => (cookies, Json(attempt)).into_response(),
        Err(error) => error.into_response(),
    }
}
pub(super) async fn auth_login(
    headers: HeaderMap,
    State(s): State<AppState>,
    Query(mut options): Query<LoginOptions>,
) -> Response {
    // The fallback always navigates the browser after establishing its session.
    options.popup = false;
    match start_login(&s, &headers, options).await {
        Ok((cookies, attempt)) => (
            cookies,
            Redirect::temporary(attempt["login_url"].as_str().expect("login URL")),
        )
            .into_response(),
        Err(error) => error.into_response(),
    }
}
async fn bound_attempt(
    s: &AppState,
    headers: &HeaderMap,
    state: Option<&str>,
) -> Result<auth::BrowserAttempt, Error> {
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
    s.auth
        .browser_attempt(expected.expect("checked"), group)
        .await
        .map_err(|error| (StatusCode::UNAUTHORIZED, Json(json!({"error":error}))))
}
fn popup_completion(
    attempt: &auth::BrowserAttempt,
    state: &str,
    complete: bool,
    can_retry: bool,
) -> Response {
    let origin = url::Url::parse(&frontend_url())
        .expect("configured frontend URL")
        .origin()
        .ascii_serialization();
    let message = json!({"type":"starter:login","attempt_id":attempt.attempt_id,"status":if complete {"complete"} else {"error"}});
    // JSON escapes '<' so neither configuration nor message data can end the script element.
    let message = message.to_string().replace('<', "\\u003c");
    let origin = serde_json::to_string(&origin)
        .expect("origin JSON")
        .replace('<', "\\u003c");
    let destination = serde_json::to_string(&attempt.return_to)
        .expect("return JSON")
        .replace('<', "\\u003c");
    let label = if complete {
        "Signed in. You can close this window."
    } else {
        "Sign-in could not complete. Retry this attempt, or return to Starter."
    };
    let retry = serde_json::to_string(&format!(
        "/auth/callback?state={}",
        urlencoding::encode(state)
    ))
    .expect("retry JSON");
    let hide_retry = complete || !can_retry;
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><title>Starter sign-in</title></head><body><p>{label}</p><p><a id="return">Return to Starter</a></p><p><a id="retry">Retry this login</a></p><script>history.replaceState(null,"","/auth/callback");document.getElementById("return").href={destination};const retry=document.getElementById("retry");retry.href={retry};retry.hidden={hide_retry};if(window.opener){{window.opener.postMessage({message},{origin});if({complete})window.close();}}else if({complete}){{location.replace({destination});}}</script></body></html>"#
    );
    let mut response = axum::response::Html(html).into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
pub(super) async fn auth_callback(
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    State(s): State<AppState>,
) -> Response {
    let state = q.get("state").map(String::as_str);
    let attempt = match bound_attempt(&s, &headers, state).await {
        Ok(attempt) => attempt,
        Err(error) => return error.into_response(),
    };
    let can_retry = !q.contains_key("error");
    let result = if can_retry {
        browser_login(&s, &headers, q.get("code").map(String::as_str), state).await
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Silicon Accounts sign-in was not completed"})),
        ))
    };
    match result {
        Ok(cookies) if attempt.popup => (
            cookies,
            popup_completion(&attempt, state.expect("bound state"), true, false),
        )
            .into_response(),
        Ok(cookies) => (cookies, Redirect::to(&attempt.return_to)).into_response(),
        Err(_) => popup_completion(&attempt, state.expect("bound state"), false, can_retry),
    }
}
#[derive(Deserialize)]
pub(super) struct AuthCallbackBody {
    pub code: String,
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
    match browser_login(&s, &headers, Some(&body.code), body.state.as_deref()).await {
        Ok(cookies) => (cookies, Json(json!({"authenticated":true}))).into_response(),
        Err(e) => e.into_response(),
    }
}
async fn browser_login(
    s: &AppState,
    headers: &HeaderMap,
    code: Option<&str>,
    state: Option<&str>,
) -> Result<HeaderMap, Error> {
    bound_attempt(s, headers, state).await?;
    let expected = cookie_value(headers, "starter_login_state").expect("bound state");
    let group = cookie_value(headers, "starter_browser").expect("bound browser");
    let id = s
        .auth
        .browser_login(code, expected, group)
        .await
        .map_err(unavailable)?;
    let tokens = s
        .auth
        .get(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("This sign-in expired".into()))?;
    Ok(session_cookies(&id, tokens.expires_at))
}
pub(super) async fn auth_cli(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(slt) = body.get("slt").and_then(Value::as_str) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let browser = body["browser"] == true;
    if browser
        && headers.get("origin").and_then(|v| v.to_str().ok()) != Some(frontend_url().as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok());
    if key.is_none_or(|key| uuid::Uuid::parse_str(key).is_err()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let group = if browser {
        match s
            .auth
            .browser_group(cookie_value(&headers, "starter_browser"), key)
            .await
        {
            Ok(group) => Some(group),
            Err(e) => return unavailable(e).into_response(),
        }
    } else {
        None
    };
    match s.auth.login(slt, key, group.as_deref()).await {
        Ok(id) => login_response(&s, &id, group.as_deref()).await,
        Err(e) => unavailable(e).into_response(),
    }
}
async fn login_response(s: &AppState, id: &str, group: Option<&str>) -> Response {
    match s.auth.status(Some(id)).await {
        Ok(mut status) => {
            status["session_id"] = json!(id);
            let mut cookies = HeaderMap::new();
            if let Some(group) = group {
                cookies = session_cookies(id, status["expires_at"].as_i64().unwrap_or_default());
                cookies.append(
                    "set-cookie",
                    cookie("starter_browser", group, Some(900 * 86400)),
                );
                // Browser code receives only the HttpOnly cookie, never its secret.
                status
                    .as_object_mut()
                    .expect("session status")
                    .remove("session_id");
            }
            (cookies, Json(status)).into_response()
        }
        Err(e) => unavailable(e).into_response(),
    }
}
pub(super) async fn auth_cli_start(headers: HeaderMap, State(s): State<AppState>) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let accounts = match s.auth.accounts().await {
        Ok(a) => a,
        Err(e) => return unavailable(e).into_response(),
    };
    match accounts.device_start().await {
        Ok(value) => Json(value).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":e.code,"message":e.message}})),
        )
            .into_response(),
    }
}
pub(super) async fn auth_cli_complete(
    headers: HeaderMap,
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    if !origin_ok(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let code = body["device_code"].as_str().unwrap_or_default();
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    match s.auth.device_complete(code, key).await {
        Ok(id) => login_response(&s, &id, None).await,
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":e.code,"message":e.message}})),
        )
            .into_response(),
    }
}
pub(super) async fn auth_cli_status(headers: HeaderMap, State(s): State<AppState>) -> Response {
    auth_session(headers, State(s)).await
}
pub(super) async fn auth_session(headers: HeaderMap, State(s): State<AppState>) -> Response {
    match s.auth.status(session_id(&headers).as_deref()).await {
        Ok(status)
            if status["authenticated"] == false
                && !headers.contains_key("x-starter-session")
                && cookie_value(&headers, "starter_session").is_some() =>
        {
            // Anonymous boot must also remove the obsolete browser credential.
            (session_cookies("", 0), Json(status)).into_response()
        }
        Ok(status) => {
            if !headers.contains_key("x-starter-session") && status["authenticated"] == true {
                let id = session_id(&headers).unwrap_or_default();
                (
                    session_cookies(&id, status["expires_at"].as_i64().unwrap_or_default()),
                    Json(status),
                )
                    .into_response()
            } else {
                Json(status).into_response()
            }
        }
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
        Ok(id) => match s.auth.get(&id).await {
            Ok(Some(tokens)) => (
                session_cookies(&id, tokens.expires_at),
                Json(json!({"context_id":context})),
            )
                .into_response(),
            _ => unavailable("Saved account needs a new sign-in".into()).into_response(),
        },
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
        session_cookies(
            &next,
            if next.is_empty() {
                0
            } else {
                s.auth
                    .get(&next)
                    .await
                    .ok()
                    .flatten()
                    .map(|t| t.expires_at)
                    .unwrap_or_default()
            },
        ),
        Json(json!({"authenticated":!next.is_empty()})),
    )
        .into_response()
}
