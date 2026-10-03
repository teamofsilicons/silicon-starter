//! Private, API/profile/world-scoped sessions and immutable request snapshots.
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
    #[serde(rename = "type")]
    pub kind: String,
    pub public_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Saved {
    pub api: String,
    pub profile: String,
    pub world: String,
    pub world_fingerprint: String,
    pub session_id: String,
    pub context_id: String,
    pub actor: Actor,
    pub org_id: String,
}
#[derive(Clone)]
pub struct Selection {
    pub api: String,
    pub profile: String,
    pub world: String,
    pub org: Option<String>,
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
    pub fn new(api: &str, profile: &str, world: &str, org: Option<String>) -> Result<Self> {
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
        if world != "production"
            && !world
                .strip_prefix("testing:")
                .is_some_and(|id| Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()))
        {
            return Err("--world must be production or testing:<environment-UUID>".into());
        }
        if org.as_ref().is_some_and(|org| org.trim().is_empty()) {
            return Err("--org must not be empty".into());
        }
        let api = normalize_api(api)?;
        let digest = format!("{:x}", Sha256::digest(format!("{api}\n{world}")));
        let dir = local::data_dir()
            .join("profiles")
            .join(profile)
            .join(digest);
        Ok(Self {
            api,
            profile: profile.into(),
            world: world.into(),
            org,
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
        validate_identity(&self.context_id, &self.actor, &self.org_id, &self.world)?;
        if self.api != selection.api
            || self.profile != selection.profile
            || self.world != selection.world
            || self.session_id.trim().is_empty()
            || self.world_fingerprint.trim().is_empty()
        {
            return Err(
                "saved session does not match the selected API, profile and world; log in again"
                    .into(),
            );
        }
        if selection
            .org
            .as_ref()
            .is_some_and(|org| org != &self.org_id)
        {
            return Err(format!(
                "this profile belongs to {}; choose another profile for the requested organization",
                self.org_id
            )
            .into());
        }
        Ok(())
    }
    pub fn check_status(&self, response: &Value) -> Result<()> {
        if response["authenticated"] != true
            || response["context_id"] != self.context_id
            || response["org_id"] != self.org_id
            || response["world"] != self.world
            || response["world_fingerprint"] != self.world_fingerprint
            || response["actor"] != serde_json::to_value(&self.actor)?
            || response
                .get("org_ids")
                .is_some_and(|orgs| *orgs != json!([self.org_id]))
        {
            return Err(
                "session identity changed or expired; log in again in the original profile".into(),
            );
        }
        Ok(())
    }
}
fn validate_identity(id: &str, actor: &Actor, org: &str, world: &str) -> Result<()> {
    let prefix = match actor.kind.as_str() {
        "carbon" => "c:",
        "silicon" => "si:",
        _ => return Err("session actor is not canonical Carbon or Silicon".into()),
    };
    if !Uuid::parse_str(id).is_ok_and(|id| !id.is_nil())
        || !actor.public_id.strip_prefix(prefix).is_some_and(|handle| {
            !handle.is_empty()
                && !handle.contains(['[', ']'])
                && !handle.chars().any(char::is_whitespace)
        })
        || org.is_empty()
        || org.chars().any(char::is_whitespace)
        || world.is_empty()
    {
        return Err(
            "session response omitted canonical context, actor, organization or world".into(),
        );
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
            world: self.selection.world.clone(),
            context_id: self.saved.as_ref().map(|s| s.context_id.clone()),
            actor_id: self.saved.as_ref().map(|s| s.actor.public_id.clone()),
            org_id: self.saved.as_ref().map(|s| s.org_id.clone()),
            world_fingerprint: self.saved.as_ref().map(|s| s.world_fingerprint.clone()),
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
            Some(_) => Err("checkout belongs to another saved login; select its original --profile/--world, or explicitly use starter context bind".into()),
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
                "not authenticated; log in with starter --profile <name> login <IAM SLT>".into(),
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
                "; authentication expired or invalid; run starter login <SLT> in the original profile"
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
    org_id: Option<String>,
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
    let path = selection.dir.join("login-pending.json");
    let old = read_private(&path)?
        .map(|bytes| serde_json::from_slice::<LoginReceipt>(&bytes))
        .transpose()?;
    let receipt=match (token,old) {
        (Some(token),Some(old)) if old.slt == token && old.org_id == selection.org => old,
        (None,Some(old)) => old,
        (Some(_),Some(_))=>return Err("a different login is pending; use login --recover or clear it with login --cancel before starting another".into()),
        (None,None)=>return Err("no interrupted login to recover".into()),
        (Some(token),None)=> {
            if !token.starts_with("oac_") && !(selection.world.starts_with("testing:") && (token.starts_with("si:") || token.starts_with("c:")) && selection.org.is_some()) { return Err("login requires an IAM oac_ SLT; testing public actors require --world testing:<UUID> and --org".into()); }
            LoginReceipt { key:Uuid::new_v4().to_string(),slt:token.into(),org_id:selection.org.clone(),started_at:now() }
        }
    };
    if now().saturating_sub(receipt.started_at) > 600 {
        remove(&path)?;
        return Err("interrupted login expired; mint a fresh IAM SLT".into());
    }
    private_write(&path, &receipt)?;
    let response = send(
        &selection.api,
        "/auth/cli",
        Method::POST,
        Some(json!({"slt":receipt.slt,"org_id":receipt.org_id})),
        None,
        None,
        Some(&receipt.key),
    )
    .await?;
    let saved = Saved {
        api: selection.api.clone(),
        profile: selection.profile.clone(),
        world: response["world"].as_str().unwrap_or_default().into(),
        world_fingerprint: response["world_fingerprint"]
            .as_str()
            .unwrap_or_default()
            .into(),
        session_id: response["session_id"].as_str().unwrap_or_default().into(),
        context_id: response["context_id"].as_str().unwrap_or_default().into(),
        actor: serde_json::from_value(response["actor"].clone())?,
        org_id: response["org_id"].as_str().unwrap_or_default().into(),
    };
    saved.validate(selection)?;
    saved.check_status(&response)?;
    if receipt
        .org_id
        .as_ref()
        .is_some_and(|org| org != &saved.org_id)
    {
        return Err("recovered login returned another organization".into());
    }
    if !receipt.slt.starts_with("oac_") && receipt.slt != saved.actor.public_id {
        return Err("testing login returned another actor".into());
    }
    private_write(&selection.dir.join("session.json"), &saved)?;
    remove(&path)?;
    println!(
        "{}",
        json!({"authenticated":true,"context_id":saved.context_id,"actor":saved.actor,"org_id":saved.org_id,"world":saved.world,"profile":selection.profile})
    );
    Ok(())
}
pub async fn cancel_login() -> Result<()> {
    let selection = &current()?.selection;
    let _lock = selection.lock().await?;
    remove(&selection.dir.join("login-pending.json"))?;
    println!("Local login retry discarded.");
    Ok(())
}
pub async fn status(as_json: bool) -> Result<()> {
    let snapshot = current()?;
    let value = if let Some(saved) = &snapshot.saved {
        let value = snapshot
            .request("/auth/cli/status", Method::GET, None, None, true)
            .await?;
        saved.check_status(&value)?;
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
pub fn daemon_lock() -> Result<File> {
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
    file.try_lock().map_err(|error| {
        format!("another Starter daemon may already supervise this home: {error}")
    })?;
    Ok(file)
}
pub fn remove(path: &Path) -> Result<()> {
    check_regular(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Snapshot, PathBuf) {
        let dir = std::env::temp_dir().join(format!("starter-context-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let mut selection =
            Selection::new("http://127.0.0.1:9", "work", "production", None).unwrap();
        selection.dir = dir.clone();
        let saved = Saved {
            api: selection.api.clone(),
            profile: selection.profile.clone(),
            world: "production".into(),
            world_fingerprint: "production:1".into(),
            session_id: "opaque-private-secret".into(),
            context_id: Uuid::new_v4().to_string(),
            actor: Actor {
                kind: "silicon".into(),
                public_id: "si:tester".into(),
            },
            org_id: "tos".into(),
        };
        private_write(&dir.join("session.json"), &saved).unwrap();
        (
            Snapshot {
                selection,
                saved: Some(saved),
            },
            dir,
        )
    }
    #[test]
    fn selectors_are_exact_and_api_cannot_leak_credentials() {
        for api in [
            "http://api.example",
            "https://user:password@api.example",
            "https://api.example/path",
            "https://api.example?token=secret",
            "https://api.example#fragment",
        ] {
            assert!(normalize_api(api).is_err(), "{api}");
        }
        assert_eq!(
            normalize_api("https://api.example/").unwrap(),
            "https://api.example"
        );
        assert!(Selection::new("https://api.example", "../other", "production", None).is_err());
        assert!(Selection::new("https://api.example", "work", "testing:invalid", None).is_err());
        let a = Selection::new("https://api.example", "work", "production", None).unwrap();
        let b = Selection::new("https://api.example", "personal", "production", None).unwrap();
        let c = Selection::new(
            "https://api.example",
            "work",
            &format!("testing:{}", Uuid::new_v4()),
            None,
        )
        .unwrap();
        assert_ne!(a.dir, b.dir);
        assert_ne!(a.dir, c.dir);
    }
    #[test]
    fn status_cannot_change_identity_or_world_fingerprint() {
        let (snapshot, dir) = fixture();
        let saved = snapshot.saved.as_ref().unwrap();
        let mut status = serde_json::to_value(saved).unwrap();
        status["authenticated"] = json!(true);
        saved.check_status(&status).unwrap();
        for (key, value) in [
            ("context_id", json!(Uuid::new_v4())),
            ("org_id", json!("other")),
            ("world", json!("testing:other")),
            ("world_fingerprint", json!("changed")),
            ("actor", json!({"type":"carbon","public_id":"c:tester"})),
        ] {
            let mut changed = status.clone();
            changed[key] = value;
            assert!(saved.check_status(&changed).is_err(), "{key}");
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn delayed_request_does_not_read_a_replacement_login() {
        let (snapshot, dir) = fixture();
        let mut changed = snapshot.saved.clone().unwrap();
        changed.context_id = Uuid::new_v4().to_string();
        changed.session_id = "another-session".into();
        private_write(&dir.join("session.json"), &changed).unwrap();
        let error = snapshot
            .request(
                "/api/v1/starters",
                Method::POST,
                Some(json!({})),
                None,
                true,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("login changed"));
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unbound_checkout_never_adopts_selected_account() {
        let (snapshot, dir) = fixture();
        let binding = local::Binding {
            id: "tos.example".into(),
            api: snapshot.selection.api.clone(),
            auth_context: None,
            mode: local::Mode::Download,
            auto_update: true,
            pinned: None,
        };
        assert!(snapshot.for_checkout(&binding).unwrap().saved.is_none());
        let mut bound = binding;
        bound.auth_context = Some(snapshot.binding());
        snapshot.for_checkout(&bound).unwrap();
        bound.auth_context.as_mut().unwrap().context_id = Some(Uuid::new_v4().to_string());
        assert!(snapshot.for_checkout(&bound).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
