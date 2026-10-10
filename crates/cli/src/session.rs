//! Private, API/profile-scoped sessions and immutable request snapshots.
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_starter_core::local::{self, CheckoutContext};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};
use uuid::Uuid;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub uuid: String,
    pub kind: String,
    pub id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Saved {
    pub api: String,
    pub profile: String,
    pub session_id: String,
    pub context_id: String,
    pub actor: Actor,
    pub expires_at: i64,
}
#[derive(Clone)]
pub struct Selection {
    pub api: String,
    pub profile: String,
    pub dir: PathBuf,
}
#[derive(Clone)]
pub struct Snapshot {
    pub selection: Selection,
    pub saved: Option<Saved>,
}
static CURRENT: OnceLock<Snapshot> = OnceLock::new();

pub fn normalize_api(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value)?;
    let local = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "--api must be an HTTPS origin (HTTP loopback is supported for development)".into(),
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}
impl Selection {
    pub fn new(api: &str, profile: &str) -> Result<Self> {
        if profile.is_empty()
            || profile.len() > 64
            || !profile
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        {
            return Err(
                "--profile must use 1–64 lowercase letters, digits, underscores or hyphens".into(),
            );
        }
        let api = normalize_api(api)?;
        let digest = format!("{:x}", Sha256::digest(api.as_bytes()));
        let dir = local::data_dir()
            .join("profiles")
            .join(profile)
            .join(digest);
        Ok(Self {
            api,
            profile: profile.into(),
            dir,
        })
    }
    pub fn read(&self) -> Result<Option<Saved>> {
        let Some(bytes) = read_private(&self.dir.join("session.json"))? else {
            return Ok(None);
        };
        let saved: Saved = serde_json::from_slice(&bytes)
            .map_err(|_| "saved session is invalid; log in again in this profile")?;
        saved.validate(self)?;
        if saved.expired() {
            return Ok(None);
        }
        Ok(Some(saved))
    }
    pub async fn lock(&self) -> Result<File> {
        private_dirs(&self.dir)?;
        let path = self.dir.join("session.lock");
        check_regular(&path)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}
impl Saved {
    pub fn validate(&self, selection: &Selection) -> Result<()> {
        validate_identity(&self.context_id, &self.actor)?;
        if self.expires_at <= 0 {
            return Err("session response omitted its expiry".into());
        }
        if self.api != selection.api
            || self.profile != selection.profile
            || self.session_id.trim().is_empty()
        {
            return Err(
                "saved session does not match the selected API and profile; log in again".into(),
            );
        }
        Ok(())
    }
    pub fn expired(&self) -> bool {
        self.expires_at <= chrono::Utc::now().timestamp()
    }
    pub fn check_status(&self, response: &Value) -> Result<()> {
        let actor: Actor = serde_json::from_value(response["actor"].clone())?;
        validate_identity(&self.context_id, &actor)?;
        if response["authenticated"] != true
            || response["context_id"] != self.context_id
            || actor.uuid != self.actor.uuid
            || actor.kind != self.actor.kind
            || response["expires_at"] != self.expires_at
        {
            return Err(
                "session identity changed or expired; log in again in the original profile".into(),
            );
        }
        Ok(())
    }
}
fn validate_identity(id: &str, actor: &Actor) -> Result<()> {
    let prefix = match actor.kind.as_str() {
        "carbon" => "c:",
        "silicon" => "si:",
        _ => return Err("session actor is not a Carbon or Silicon".into()),
    };
    if !Uuid::parse_str(id).is_ok_and(|id| !id.is_nil())
        || !Uuid::parse_str(&actor.uuid).is_ok_and(|id| !id.is_nil())
        || !actor.id.strip_prefix(prefix).is_some_and(|handle| {
            !handle.is_empty()
                && !handle.contains(['[', ']'])
                && !handle.chars().any(char::is_whitespace)
        })
    {
        return Err("session response omitted a canonical context or account identity".into());
    }
    Ok(())
}
pub fn configure(selection: Selection, ignore_saved: bool) -> Result<()> {
    let saved = if ignore_saved {
        None
    } else {
        selection.read()?
    };
    CURRENT
        .set(Snapshot { selection, saved })
        .map_err(|_| "request context was already selected")?;
    Ok(())
}
pub fn current() -> Result<&'static Snapshot> {
    CURRENT
        .get()
        .ok_or_else(|| "no selected request context".into())
}
impl Snapshot {
    pub fn check_current(&self) -> Result<()> {
        let now = self.selection.read()?;
        if now.as_ref().map(|saved| &saved.context_id)
            != self.saved.as_ref().map(|saved| &saved.context_id)
        {
            return Err("the login changed while this operation was pending; return to its original context".into());
        }
        Ok(())
    }
    pub fn binding(&self) -> CheckoutContext {
        CheckoutContext {
            profile: self.selection.profile.clone(),
            context_id: self.saved.as_ref().map(|s| s.context_id.clone()),
            actor_id: self.saved.as_ref().map(|s| s.actor.uuid.clone()),
        }
    }
    pub fn for_checkout(&self, binding: &local::Binding) -> Result<Self> {
        if normalize_api(&binding.api)? != self.selection.api {
            return Err(format!(
                "checkout belongs to {}; use its original --api",
                binding.api
            )
            .into());
        }
        match &binding.auth_context {
            None => Ok(Self { selection:self.selection.clone(), saved:None }),
            Some(origin) if origin.context_id.is_none() => Ok(Self { selection:self.selection.clone(), saved:None }),
            Some(origin) if origin == &self.binding() => { self.check_current()?; Ok(self.clone()) },
            Some(_) => Err("checkout belongs to another saved login; select its original --profile, or explicitly use starter context bind".into()),
        }
    }
    pub async fn request(
        &self,
        path: &str,
        method: Method,
        body: Option<Value>,
        key: Option<&str>,
        required: bool,
    ) -> Result<Value> {
        if required && self.saved.is_none() {
            return Err(
                "not authenticated; run starter login for browser approval, or starter login --slt <SLT>".into(),
            );
        }
        if self.saved.is_some() {
            self.check_current()?;
        }
        let value = send(
            &self.selection.api,
            path,
            method,
            body,
            self.saved.as_ref().map(|saved| saved.session_id.as_str()),
            self.saved.as_ref().map(|saved| saved.context_id.as_str()),
            key,
        )
        .await?;
        if self.saved.is_some() {
            self.check_current()?;
        }
        Ok(value)
    }
}
pub struct HttpError {
    pub status: u16,
    pub value: Value,
}
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.value)?;
        if self.status == 401 {
            write!(
                f,
                "; authentication expired or invalid; run starter login in the original profile"
            )?;
        }
        Ok(())
    }
}
impl std::fmt::Debug for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for HttpError {}
pub async fn send(
    api: &str,
    path: &str,
    method: Method,
    body: Option<Value>,
    session: Option<&str>,
    context_id: Option<&str>,
    key: Option<&str>,
) -> Result<Value> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let mut request = client.request(method, format!("{api}{path}"));
    if let Some(session) = session {
        request = request.header("X-Starter-Session", session);
    }
    if let Some(context_id) = context_id {
        request = request.header("X-Starter-Context", context_id);
    }
    if let Some(key) = key {
        request = request.header("Idempotency-Key", key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    let value: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"error":"backend returned an invalid response"}));
    if !status.is_success() {
        return Err(Box::new(HttpError {
            status: status.as_u16(),
            value,
        }));
    }
    Ok(value)
}
#[derive(Serialize, Deserialize)]
struct LoginReceipt {
    key: String,
    slt: String,
    started_at: u64,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub async fn login(token: Option<&str>) -> Result<()> {
    let selection = &current()?.selection;
    let _lock = selection.lock().await?;
    if read_private(&selection.dir.join("device-pending.json"))?.is_some() {
        return Err("a browser login is pending; use login --recover or login --cancel".into());
    }
    let path = selection.dir.join("login-pending.json");
    let old = read_private(&path)?
        .map(|bytes| serde_json::from_slice::<LoginReceipt>(&bytes))
        .transpose()?;
    let receipt=match (token,old) {
        (Some(token),Some(old)) if old.slt == token => old,
        (None,Some(old)) => old,
        (Some(_),Some(_))=>return Err("a different login is pending; use login --recover or clear it with login --cancel before starting another".into()),
        (None,None)=>return Err("no interrupted login to recover".into()),
        (Some(token),None)=> {
            if !token.starts_with("slt_") || token.chars().any(char::is_whitespace) {
                return Err("login requires a Silicon Accounts slt_ token for starter".into());
            }
            LoginReceipt { key:Uuid::new_v4().to_string(),slt:token.into(),started_at:now() }
        }
    };
    if now().saturating_sub(receipt.started_at) > 600 {
        remove(&path)?;
        return Err("interrupted login expired; mint a fresh Silicon Accounts SLT".into());
    }
    private_write(&path, &receipt)?;
    let response = send(
        &selection.api,
        "/auth/cli",
        Method::POST,
        Some(json!({"slt":receipt.slt})),
        None,
        None,
        Some(&receipt.key),
    )
    .await?;
    let saved = save_login(selection, &response)?;
    remove(&path)?;
    println!(
        "{}",
        json!({"authenticated":true,"context_id":saved.context_id,"actor":saved.actor,"expires_at":saved.expires_at,"profile":selection.profile})
    );
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct DeviceReceipt {
    key: String,
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_at: u64,
}
pub async fn recover_login() -> Result<()> {
    if read_private(&current()?.selection.dir.join("device-pending.json"))?.is_some() {
        device_login(true).await
    } else {
        login(None).await
    }
}
pub async fn device_login(recover: bool) -> Result<()> {
    let selection = &current()?.selection;
    let _lock = selection.lock().await?;
    if read_private(&selection.dir.join("login-pending.json"))?.is_some() {
        return Err("a token login is pending; use login --recover or login --cancel".into());
    }
    let path = selection.dir.join("device-pending.json");
    let saved = read_private(&path)?
        .map(|bytes| serde_json::from_slice::<DeviceReceipt>(&bytes))
        .transpose()?;
    let mut receipt = if let Some(receipt) = saved {
        receipt
    } else {
        if recover {
            return Err("no browser login to recover".into());
        }
        let response = send(
            &selection.api,
            "/auth/cli/start",
            Method::POST,
            Some(json!({})),
            None,
            None,
            None,
        )
        .await?;
        let uri = response["verification_uri_complete"]
            .as_str()
            .or_else(|| response["verification_uri"].as_str())
            .ok_or("sign-in URL missing")?;
        let url = reqwest::Url::parse(uri)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("sign-in URL must be HTTPS".into());
        }
        DeviceReceipt {
            key: Uuid::new_v4().to_string(),
            device_code: response["device_code"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("device code missing")?
                .into(),
            user_code: response["user_code"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("user code missing")?
                .into(),
            verification_uri: uri.into(),
            interval: response["interval"].as_u64().unwrap_or(5).max(1),
            expires_at: now()
                .checked_add(
                    response["expires_in"]
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("device expiry missing")?,
                )
                .ok_or("invalid device expiry")?,
        }
    };
    private_write(&path, &receipt)?;
    eprintln!(
        "Open {} and confirm code {}. Waiting for sign-in…",
        receipt.verification_uri, receipt.user_code
    );
    loop {
        if now() >= receipt.expires_at {
            remove(&path)?;
            return Err("browser sign-in expired; run starter login again".into());
        }
        let response = send(
            &selection.api,
            "/auth/cli/complete",
            Method::POST,
            Some(json!({"device_code":receipt.device_code})),
            None,
            None,
            Some(&receipt.key),
        )
        .await;
        match response {
            Ok(response) => {
                let saved = save_login(selection, &response)?;
                remove(&path)?;
                println!(
                    "{}",
                    json!({"authenticated":true,"context_id":saved.context_id,"actor":saved.actor,"expires_at":saved.expires_at,"profile":selection.profile})
                );
                return Ok(());
            }
            Err(error) => match error
                .downcast_ref::<HttpError>()
                .and_then(|error| error.value["error"]["code"].as_str())
            {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    receipt.interval = receipt.interval.saturating_add(5);
                    private_write(&path, &receipt)?;
                }
                Some("access_denied" | "expired_token" | "invalid_grant") => {
                    remove(&path)?;
                    return Err(error);
                }
                _ => return Err(error),
            },
        }
        tokio::time::sleep(std::time::Duration::from_secs(
            receipt
                .interval
                .min(receipt.expires_at.saturating_sub(now())),
        ))
        .await;
    }
}
fn save_login(selection: &Selection, response: &Value) -> Result<Saved> {
    let saved = Saved {
        api: selection.api.clone(),
        profile: selection.profile.clone(),
        session_id: response["session_id"].as_str().unwrap_or_default().into(),
        context_id: response["context_id"].as_str().unwrap_or_default().into(),
        actor: serde_json::from_value(response["actor"].clone())?,
        expires_at: response["expires_at"].as_i64().unwrap_or_default(),
    };
    saved.validate(selection)?;
    saved.check_status(response)?;
    if saved.expired() {
        return Err("sign-in already expired; request a new Silicon Accounts token".into());
    }
    private_write(&selection.dir.join("session.json"), &saved)?;
    Ok(saved)
}
pub async fn logout() -> Result<()> {
    let snapshot = current()?;
    let _lock = snapshot.selection.lock().await?;
    if let Some(saved) = &snapshot.saved {
        snapshot.check_current()?;
        match send(
            &snapshot.selection.api,
            "/auth/logout",
            Method::POST,
            None,
            Some(&saved.session_id),
            Some(&saved.context_id),
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(error)
                if error
                    .downcast_ref::<HttpError>()
                    .is_some_and(|error| error.status == 401) => {}
            Err(error) => return Err(error),
        }
    }
    for file in ["session.json", "login-pending.json", "device-pending.json"] {
        remove(&snapshot.selection.dir.join(file))?;
    }
    println!("Signed out.");
    Ok(())
}
pub async fn cancel_login() -> Result<()> {
    let selection = &current()?.selection;
    let _lock = selection.lock().await?;
    remove(&selection.dir.join("login-pending.json"))?;
    remove(&selection.dir.join("device-pending.json"))?;
    println!("Local login retry discarded.");
    Ok(())
}
pub async fn status(as_json: bool) -> Result<()> {
    let snapshot = current()?;
    let value = if let Some(saved) = &snapshot.saved {
        let value = snapshot
            .request("/auth/cli/status", Method::GET, None, None, true)
            .await;
        let value = match value {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<HttpError>()
                    .is_some_and(|error| error.status == 401) =>
            {
                json!({"authenticated":false})
            }
            Err(error) => return Err(error),
        };
        if value["authenticated"] == true {
            saved.check_status(&value)?;
        }
        snapshot.check_current()?;
        value
    } else {
        json!({"authenticated":false})
    };
    if as_json {
        println!("{value}");
    } else {
        println!("authenticated: {}", value["authenticated"]);
    }
    Ok(())
}
pub fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    check_regular(path)?;
    match fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
                    return Err(
                        "saved credentials must have mode 0600; tighten permissions before retrying"
                            .into(),
                    );
                }
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn check_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            Err(format!("{} must be a regular private file", path.display()).into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn private_dirs(path: &Path) -> Result<()> {
    let base = local::data_dir();
    let mut next = base.clone();
    for component in std::iter::once(None).chain(path.strip_prefix(&base)?.components().map(Some)) {
        if let Some(component) = component {
            next.push(component);
        }
        match fs::symlink_metadata(&next) {
            Ok(meta) if !meta.file_type().is_dir() => {
                return Err("profile directories must not be symlinks".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&next)?;
            }
            Err(error) => return Err(error.into()),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&next, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}
pub fn private_write(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("private file has no parent")?;
    if parent.starts_with(local::data_dir()) {
        private_dirs(parent)?;
    }
    check_regular(path)?;
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
/// One registry supervisor per home, independent of selected profile.
pub fn daemon_lock() -> Result<Option<File>> {
    let dir = local::data_dir();
    private_dirs(&dir)?;
    let path = dir.join("daemon.lock");
    check_regular(&path)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}
pub fn remove(path: &Path) -> Result<()> {
    check_regular(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
