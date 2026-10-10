//! Durable account sessions. Access tokens rotate while the sign-in's absolute
//! expiry and immutable account UUID stay fixed across browser and CLI restarts.
use crate::accounts::Accounts;
use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, AeadCore, OsRng},
};
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;
type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: i64,
    pub expires_at: i64,
    pub actor: Value,
    pub context_id: String,
}
impl SessionTokens {
    pub fn account_uuid(&self) -> &str {
        self.actor["uuid"].as_str().unwrap_or_default()
    }
    pub fn actor_id(&self) -> &str {
        self.actor["id"].as_str().unwrap_or_default()
    }
    fn valid(&self) -> bool {
        Uuid::parse_str(self.account_uuid()).is_ok_and(|id| !id.is_nil())
            && valid_actor_id(
                self.actor["kind"].as_str().unwrap_or_default(),
                self.actor_id(),
            )
            && !self.context_id.is_empty()
            && !self.access_token.is_empty()
            && self.refresh_token.starts_with("sar_")
            && self.expires_at > Utc::now().timestamp()
    }
    fn status(&self) -> Value {
        json!({"authenticated":true,"context_id":self.context_id,"actor":self.actor,
            "expires_at":self.expires_at,"access_expires_at":self.access_expires_at})
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Session {
    secret: String,
    tokens: SessionTokens,
    group: Option<String>,
    #[serde(default)]
    refresh_started: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct LoginReceipt {
    binding: String,
    expires_at: i64,
    session: String,
    completed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct LoginState {
    group: String,
    expires_at: i64,
    verifier: String,
    attempt: BrowserAttempt,
    receipt: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct BrowserAttempt {
    pub attempt_id: String,
    pub identity_kind: String,
    pub return_to: String,
    pub popup: bool,
    pub challenge: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    sessions: HashMap<String, Session>,
    groups: HashSet<String>,
    logins: HashMap<String, LoginReceipt>,
    states: HashMap<String, LoginState>,
    attempts: HashMap<String, String>,
}
#[derive(Clone)]
pub struct AuthState {
    inner: Arc<Mutex<Store>>,
    file: Option<PathBuf>,
    key: [u8; 32],
    configuration_error: Option<String>,
    accounts: Arc<OnceCell<Accounts>>,
}
impl Default for AuthState {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            file: None,
            key: Sha256::digest(random_secret().as_bytes()).into(),
            configuration_error: None,
            accounts: Default::default(),
        }
    }
}
impl AuthState {
    pub fn from_env() -> Self {
        let mut state = Self {
            file: std::env::var_os("STARTER_AUTH_FILE").map(PathBuf::from),
            ..Self::default()
        };
        if state.file.is_some() {
            match std::env::var("STARTER_AUTH_ENCRYPTION_KEY")
                .ok()
                .and_then(|s| hex::decode(s).ok())
                .and_then(|v| v.try_into().ok())
            {
                Some(key) => state.key = key,
                None => {
                    state.configuration_error = Some(
                        "STARTER_AUTH_ENCRYPTION_KEY must contain 64 hexadecimal characters".into(),
                    )
                }
            }
        }
        state
    }
    pub async fn accounts(&self) -> Result<Accounts, String> {
        Ok(self
            .accounts
            .get_or_try_init(Accounts::from_env)
            .await?
            .clone())
    }
    pub async fn load(&self) -> Result<(), String> {
        let configured = std::env::var_os("STARTER_ACCOUNTS_APP_SECRET").is_some()
            || std::env::var_os("ACCOUNTS_APP_SECRET").is_some();
        if configured && self.file.is_none() {
            return Err("Configure STARTER_AUTH_FILE and STARTER_AUTH_ENCRYPTION_KEY for persistent account sessions".into());
        }
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        if configured {
            self.accounts().await?;
        }
        Ok(())
    }
    async fn file_lock(&self) -> Result<Option<std::fs::File>, String> {
        if let Some(error) = &self.configuration_error {
            return Err(error.clone());
        }
        let Some(path) = self.file.clone() else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(|_| "Cannot create session store directory")?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.create(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(path.with_extension("lock"))
                .map_err(|_| "Cannot open session store lock")?;
            fs2::FileExt::lock_exclusive(&file).map_err(|_| "Cannot lock session store")?;
            Ok(Some(file))
        })
        .await
        .map_err(|_| "Session store lock failed")?
    }
    fn reload(&self, data: &mut Store) -> Result<(), String> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let bytes = match std::fs::read(path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                *data = Store::default();
                return Ok(());
            }
            Err(_) => return Err("Cannot read session store".into()),
        };
        if !bytes.starts_with(b"STARTER-ACCT\0") || bytes.len() < 25 {
            return Err("Legacy or invalid STARTER_AUTH_FILE; retain its backup and use a new encrypted session store, then sign in again".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "Invalid encryption key")?;
        let clear = cipher
            .decrypt(
                (&bytes[13..25]).into(),
                aes_gcm::aead::Payload {
                    msg: &bytes[25..],
                    aad: b"starter-accounts-session-store",
                },
            )
            .map_err(|_| "Cannot decrypt session store")?;
        *data = serde_json::from_slice(&clear).map_err(|_| "Invalid encrypted session store")?;
        Ok(())
    }
    fn persist(&self, data: &Store) -> Result<(), String> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "Invalid encryption key")?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let clear = serde_json::to_vec(data).map_err(|_| "Cannot encode session store")?;
        let encrypted = cipher
            .encrypt(
                &nonce,
                aes_gcm::aead::Payload {
                    msg: &clear,
                    aad: b"starter-accounts-session-store",
                },
            )
            .map_err(|_| "Cannot encrypt session store")?;
        let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&temp)
                .map_err(|_| "Cannot create session store")?;
            file.write_all(b"STARTER-ACCT\0")
                .and_then(|_| file.write_all(&nonce))
                .and_then(|_| file.write_all(&encrypted))
                .and_then(|_| file.sync_all())
                .map_err(|_| "Cannot write session store")?;
            std::fs::rename(&temp, path).map_err(|_| "Cannot commit session store")?;
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::File::open(parent)
                    .and_then(|p| p.sync_all())
                    .map_err(|_| "Cannot synchronize session store")?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result
    }
    fn receipt_key(&self, operation: &str, secret: &str) -> String {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.key).expect("HMAC key");
        mac.update(operation.as_bytes());
        mac.update(&[0]);
        mac.update(secret.as_bytes());
        let digest = mac.finalize().into_bytes();
        Uuid::from_bytes(digest[..16].try_into().expect("16 bytes")).to_string()
    }
    pub async fn browser_group(
        &self,
        existing: Option<&str>,
        login_key: Option<&str>,
    ) -> Result<String, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let group = existing
            .filter(|v| data.groups.contains(&hash(v)))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                login_key
                    .map(|key| self.receipt_key("browser-group", key))
                    .unwrap_or_else(random_secret)
            });
        data.groups.insert(hash(&group));
        self.persist(&data)?;
        Ok(group)
    }
    pub async fn begin_browser_login(
        &self,
        existing: Option<&str>,
        identity_kind: &str,
        return_to: String,
        popup: bool,
    ) -> Result<(String, String), String> {
        if identity_kind != "carbon" {
            return Err("Silicons sign in with an SLT from Silicon Accounts".into());
        }
        self.accounts().await?;
        let group = self.browser_group(existing, None).await?;
        let nonce = random_secret();
        let verifier = random_secret();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        data.states
            .retain(|_, s| s.expires_at > Utc::now().timestamp());
        data.logins
            .retain(|_, r| r.expires_at > Utc::now().timestamp());
        data.states.insert(
            hash(&nonce),
            LoginState {
                group: hash(&group),
                expires_at: Utc::now().timestamp() + 3600,
                verifier,
                receipt: None,
                attempt: BrowserAttempt {
                    attempt_id: Uuid::new_v4().to_string(),
                    identity_kind: identity_kind.into(),
                    return_to,
                    popup,
                    challenge,
                },
            },
        );
        self.persist(&data)?;
        Ok((nonce, group))
    }
    pub async fn browser_attempt(
        &self,
        nonce: &str,
        group: &str,
    ) -> Result<BrowserAttempt, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        data.states
            .get(&hash(nonce))
            .filter(|s| s.group == hash(group) && s.expires_at > Utc::now().timestamp())
            .map(|s| s.attempt.clone())
            .ok_or("Login state expired or belongs to another browser".into())
    }
    pub async fn browser_login(
        &self,
        code: Option<&str>,
        nonce: &str,
        group: &str,
    ) -> Result<String, String> {
        let accounts = self.accounts().await?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let state = data
            .states
            .get(&hash(nonce))
            .filter(|s| s.group == hash(group) && s.expires_at > Utc::now().timestamp())
            .cloned()
            .ok_or("Login state expired or belongs to another browser")?;
        let binding = format!("browser:{}:{}", hash(group), hash(nonce));
        if let Some(key) = &state.receipt {
            if let Some(code) = code
                && self.receipt_key("code", code) != *key
            {
                return Err("This login state belongs to another code".into());
            }
            return receipt_session(&data, key, &binding);
        }
        let code = code
            .filter(|s| s.starts_with("sac_"))
            .ok_or("The authorization code is missing; begin a new sign-in")?;
        let key = self.receipt_key("code", code);
        if data.logins.contains_key(&key) {
            return receipt_session(&data, &key, &binding);
        }
        self.begin_receipt(&mut data, &key, &binding);
        data.states
            .get_mut(&hash(nonce))
            .expect("bound state")
            .receipt = Some(key.clone());
        // Accounts codes are one-use: record the attempt before sending, and never
        // resend an uncertain exchange (reusing a code revokes its winning session).
        self.persist(&data)?;
        let pair = match accounts
            .exchange_code(
                code,
                &format!("{}/auth/callback", crate::frontend_url()),
                &state.verifier,
            )
            .await
        {
            Ok(pair) => pair,
            Err(error) => {
                if error.code == "accounts_connection_failed" {
                    data.logins.remove(&key);
                    data.states
                        .get_mut(&hash(nonce))
                        .expect("bound state")
                        .receipt = None;
                    self.persist(&data)?;
                }
                return Err(error.into());
            }
        };
        let tokens = tokens_of(pair)?;
        if tokens.actor["kind"] != "carbon" {
            return Err("Hosted sign-in requires a Carbon account".into());
        }
        self.finish_receipt(&mut data, &key, tokens, Some(hash(group)))
    }
    fn begin_receipt(&self, data: &mut Store, key: &str, binding: &str) {
        data.logins.insert(
            key.into(),
            LoginReceipt {
                binding: binding.into(),
                expires_at: Utc::now().timestamp() + 3600,
                session: random_secret(),
                completed: false,
            },
        );
    }
    fn finish_receipt(
        &self,
        data: &mut Store,
        key: &str,
        tokens: SessionTokens,
        group: Option<String>,
    ) -> Result<String, String> {
        let receipt = data.logins.get_mut(key).expect("login receipt");
        receipt.completed = true;
        let id = receipt.session.clone();
        data.sessions.insert(
            hash(&id),
            Session {
                secret: id.clone(),
                tokens,
                group,
                refresh_started: false,
            },
        );
        self.persist(data)?;
        Ok(id)
    }
    pub async fn login(
        &self,
        slt: &str,
        cli_key: Option<&str>,
        group: Option<&str>,
    ) -> Result<String, String> {
        if !slt.starts_with("slt_") || slt.len() > 1024 {
            return Err("Use a short-lived token from silicon-accounts login --app starter".into());
        }
        let cli_key = cli_key
            .filter(|k| Uuid::parse_str(k).is_ok())
            .ok_or("Login requires a UUID Idempotency-Key")?;
        let accounts = self.accounts().await?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        if let Some(group) = group
            && !data.groups.contains(&hash(group))
        {
            return Err("Browser group expired".into());
        }
        let binding = format!("cli:{cli_key}:{}", group.map(hash).unwrap_or_default());
        let key = self.receipt_key("slt", slt);
        if let Some(previous) = data.attempts.get(&binding)
            && previous != &key
        {
            return Err("Begin a new login attempt for a different token".into());
        }
        if data.logins.contains_key(&key) {
            return receipt_session(&data, &key, &binding);
        }
        data.attempts.insert(binding.clone(), key.clone());
        self.begin_receipt(&mut data, &key, &binding);
        self.persist(&data)?;
        let pair = match accounts.exchange_slt(slt).await {
            Ok(pair) => pair,
            Err(error) => {
                if error.code == "accounts_connection_failed" {
                    data.logins.remove(&key);
                    self.persist(&data)?;
                }
                return Err(error.into());
            }
        };
        let tokens = tokens_of(pair)?;
        self.finish_receipt(&mut data, &key, tokens, group.map(hash))
    }
    pub async fn device_complete(
        &self,
        code: &str,
        key: &str,
    ) -> Result<String, crate::accounts::Error> {
        let failure = |message: String| crate::accounts::Error {
            code: "login_unavailable".into(),
            message,
        };
        if !code.starts_with("sad_") || code.len() > 1024 || Uuid::parse_str(key).is_err() {
            return Err(failure(
                "A device code and UUID Idempotency-Key are required".into(),
            ));
        }
        let accounts = self.accounts().await.map_err(failure)?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await.map_err(failure)?;
        self.reload(&mut data).map_err(failure)?;
        let binding = format!("device:{key}");
        let receipt = self.receipt_key("device", code);
        if let Some(previous) = data.attempts.get(&binding)
            && previous != &receipt
        {
            return Err(failure("Begin a new device login attempt".into()));
        }
        if data.logins.contains_key(&receipt) {
            return receipt_session(&data, &receipt, &binding).map_err(failure);
        }
        data.attempts.insert(binding.clone(), receipt.clone());
        self.begin_receipt(&mut data, &receipt, &binding);
        self.persist(&data).map_err(failure)?;
        match accounts.device_complete(code).await {
            Ok(pair) => {
                let tokens = tokens_of(pair).map_err(failure)?;
                self.finish_receipt(&mut data, &receipt, tokens, None)
                    .map_err(failure)
            }
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "authorization_pending" | "slow_down" | "accounts_connection_failed"
                ) {
                    data.logins.remove(&receipt);
                    self.persist(&data).map_err(failure)?;
                }
                Err(error)
            }
        }
    }
    pub async fn get(&self, id: &str) -> Result<Option<SessionTokens>, String> {
        let accounts = self.accounts().await?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let key = hash(id);
        let Some(mut row) = data.sessions.get(&key).cloned() else {
            return Ok(None);
        };
        if !row.tokens.valid() {
            data.sessions.remove(&key);
            self.persist(&data)?;
            return Ok(None);
        }
        if row.refresh_started {
            return Err("The session refresh response was lost. Sign in again to avoid reusing a consumed token".into());
        }
        if row.tokens.access_expires_at <= Utc::now().timestamp() + 30 {
            row.refresh_started = true;
            data.sessions.insert(key.clone(), row.clone());
            self.persist(&data)?;
            let pair = match accounts.refresh(&row.tokens.refresh_token).await {
                Ok(pair) => pair,
                Err(error) if error.code == "invalid_grant" => {
                    data.sessions.remove(&key);
                    self.persist(&data)?;
                    return Ok(None);
                }
                Err(error) => {
                    if error.code == "accounts_connection_failed" {
                        row.refresh_started = false;
                        data.sessions.insert(key.clone(), row.clone());
                        self.persist(&data)?;
                    }
                    return Err(error.into());
                }
            };
            let mut next = tokens_of(pair)?;
            if next.account_uuid() != row.tokens.account_uuid()
                || next.expires_at > row.tokens.expires_at
            {
                return Err(
                    "Silicon Accounts changed the session's immutable account or expiry".into(),
                );
            }
            next.context_id = row.tokens.context_id.clone();
            row.tokens = next;
            row.refresh_started = false;
            data.sessions.insert(key.clone(), row.clone());
            self.persist(&data)?;
        }
        let seen = accounts.introspect(&row.tokens.access_token).await?;
        if seen["active"] != true {
            data.sessions.remove(&key);
            self.persist(&data)?;
            return Ok(None);
        }
        let valid = seen["sub"].as_str() == Some(row.tokens.account_uuid())
            && seen["aud"] == accounts.app_id
            && seen["client_id"] == accounts.app_id
            && seen["kind"] == row.tokens.actor["kind"]
            && seen["token_type"] == "access_token"
            && seen["exp"]
                .as_i64()
                .is_some_and(|t| t > Utc::now().timestamp())
            && valid_actor_id(
                seen["kind"].as_str().unwrap_or_default(),
                seen["id"].as_str().unwrap_or_default(),
            );
        if !valid {
            return Err("Silicon Accounts returned a mismatched session identity".into());
        }
        row.tokens.actor["id"] = seen["id"].clone();
        row.tokens.access_expires_at = seen["exp"].as_i64().expect("validated expiry");
        data.sessions.insert(key, row.clone());
        self.persist(&data)?;
        Ok(Some(row.tokens))
    }
    pub async fn status(&self, id: Option<&str>) -> Result<Value, String> {
        Ok(match id {
            Some(id) => self
                .get(id)
                .await?
                .map(|t| t.status())
                .unwrap_or(json!({"authenticated":false})),
            None => json!({"authenticated":false}),
        })
    }
    pub async fn contexts(
        &self,
        group: Option<&str>,
        selected: Option<&str>,
    ) -> Result<Value, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let group = group.map(hash);
        let mut rows: Vec<Value> = data
            .sessions
            .values()
            .filter(|s| group.is_some() && s.group == group && s.tokens.valid())
            .map(|s| {
                let mut row = s.tokens.status();
                row["selected"] = json!(selected.is_some_and(|id| id == s.secret));
                row
            })
            .collect();
        rows.sort_by(|a, b| a["context_id"].as_str().cmp(&b["context_id"].as_str()));
        Ok(json!({"contexts":rows}))
    }
    pub async fn select(&self, group: Option<&str>, context: &str) -> Result<String, String> {
        let secret = {
            let mut data = self.inner.lock().await;
            let _lock = self.file_lock().await?;
            self.reload(&mut data)?;
            let group = group.map(hash);
            data.sessions
                .values()
                .find(|s| group.is_some() && s.group == group && s.tokens.context_id == context)
                .map(|s| s.secret.clone())
                .ok_or("Account is not saved in this browser")?
        };
        self.get(&secret)
            .await?
            .ok_or("Saved account needs a new sign-in")?;
        Ok(secret)
    }
    pub async fn remove(&self, id: &str) -> Result<bool, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let Some(row) = data.sessions.get(&hash(id)) else {
            return Ok(false);
        };
        self.accounts()
            .await?
            .revoke(&row.tokens.refresh_token)
            .await?;
        data.sessions.remove(&hash(id));
        self.persist(&data)?;
        Ok(true)
    }
}
fn receipt_session(data: &Store, key: &str, binding: &str) -> Result<String, String> {
    let receipt = data.logins.get(key).ok_or("Login attempt is unavailable")?;
    if receipt.binding != binding {
        return Err("The login belongs to another attempt".into());
    }
    if !receipt.completed {
        return Err("This one-use exchange is incomplete. Begin a new sign-in; retrying its token can revoke the session".into());
    }
    data.sessions
        .get(&hash(&receipt.session))
        .filter(|s| s.tokens.valid())
        .map(|_| receipt.session.clone())
        .ok_or("This sign-in ended; sign in again".into())
}
fn tokens_of(pair: Value) -> Result<SessionTokens, String> {
    let text = |field: &str| {
        pair[field]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("Silicon Accounts omitted {field}"))
    };
    let expires_at = chrono::DateTime::parse_from_rfc3339(&text("refresh_token_expires_at")?)
        .map_err(|_| "Silicon Accounts returned an invalid session expiry")?
        .timestamp();
    let expires_in = pair["expires_in"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or("Silicon Accounts omitted the access-token expiry")?;
    let tokens = SessionTokens {
        access_token: text("access_token")?,
        refresh_token: text("refresh_token")?,
        access_expires_at: (Utc::now().timestamp() + expires_in).min(expires_at),
        expires_at,
        actor: pair["account"].clone(),
        context_id: Uuid::new_v4().to_string(),
    };
    if pair["token_type"] != "Bearer" || !tokens.valid() {
        return Err("Silicon Accounts did not return a valid Carbon or Silicon session".into());
    }
    Ok(tokens)
}
fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
pub fn valid_login_state(expected: Option<&str>, supplied: Option<&str>) -> bool {
    matches!((expected,supplied),(Some(a),Some(b)) if a.len()==64&&a==b)
}
pub(crate) fn validate_app_id(id: &str) -> Result<(), String> {
    if (2..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err("Application ID must be a Silicon Apps handle".into())
    }
}
pub(crate) fn valid_actor_id(kind: &str, id: &str) -> bool {
    let prefix = match kind {
        "carbon" => "c:",
        "silicon" => "si:",
        _ => return false,
    };
    id.strip_prefix(prefix).is_some_and(|h| {
        !h.is_empty()
            && h.len() <= 50
            && h.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    })
}
