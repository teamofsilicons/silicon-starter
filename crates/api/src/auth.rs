//! IAM 5 ordinary sessions. Each encrypted row owns one immutable actor/org/world.
use crate::iam::{Iam, World};
use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, AeadCore, OsRng},
};
use axum::http::HeaderMap;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_iam_client::{Mutation, models};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;
type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IamTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub expires_at: Option<i64>,
    pub actor: Option<Value>,
    pub org_id: Option<String>,
    pub org_ids: Vec<String>,
    pub context_id: String,
    pub world: World,
}
impl IamTokens {
    pub fn organizations(&self) -> Vec<String> {
        self.org_id
            .as_ref()
            .filter(|org| valid_org(org) && self.org_ids == [org.as_str()])
            .cloned()
            .into_iter()
            .collect()
    }
    fn valid(&self) -> bool {
        let Some(actor) = &self.actor else {
            return false;
        };
        !self.context_id.is_empty()
            && self.organizations().len() == 1
            && valid_actor_id(
                actor["type"].as_str().unwrap_or_default(),
                actor["public_id"].as_str().unwrap_or_default(),
            )
            && !self.access_token.is_empty()
            && !self.refresh_token.is_empty()
    }
    fn status(&self) -> Value {
        json!({"authenticated":true,"context_id":self.context_id,"actor":self.actor,
            "org_id":self.org_id,"org_ids":self.org_ids,"world":self.world.id,
            "world_fingerprint":self.world.fingerprint()})
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Session {
    secret: String,
    tokens: IamTokens,
    group: Option<String>,
    pending_pair: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct LoginReceipt {
    binding: String,
    expires_at: i64,
    session: String,
    candidate: Option<IamTokens>,
    completed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct LoginState {
    group: String,
    expires_at: i64,
    slt_hash: Option<String>,
    #[serde(default)]
    slt: Option<String>,
    #[serde(default)]
    attempt: Option<BrowserAttempt>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct BrowserAttempt {
    pub attempt_id: String,
    pub identity_kind: String,
    pub return_to: String,
    pub popup: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    sessions: HashMap<String, Session>,
    groups: HashSet<String>,
    logins: HashMap<String, LoginReceipt>,
    states: HashMap<String, LoginState>,
    #[serde(default)]
    attempts: HashMap<String, String>,
}
#[derive(Clone)]
pub struct AuthState {
    inner: Arc<Mutex<Store>>,
    file: Option<PathBuf>,
    key: [u8; 32],
    configuration_error: Option<String>,
    iam: Arc<OnceCell<Iam>>,
    #[cfg(test)]
    fixture: bool,
}
impl Default for AuthState {
    fn default() -> Self {
        let key = Sha256::digest(random_secret().as_bytes()).into();
        Self {
            inner: Default::default(),
            file: None,
            key,
            configuration_error: None,
            iam: Default::default(),
            #[cfg(test)]
            fixture: true,
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
                .and_then(|k| hex::decode(k).ok())
                .and_then(|k| k.try_into().ok())
            {
                Some(key) => state.key = key,
                None => {
                    state.configuration_error = Some(
                        "STARTER_AUTH_ENCRYPTION_KEY must contain 64 hexadecimal characters".into(),
                    )
                }
            }
        }
        #[cfg(test)]
        {
            state.fixture = false;
        }
        state
    }
    pub async fn iam(&self) -> Result<Iam, String> {
        let iam = self.iam.get_or_try_init(Iam::from_env).await?.clone();
        iam.assert_current().await?;
        Ok(iam)
    }
    pub async fn load(&self) -> Result<(), String> {
        if self.file.is_none()
            && (std::env::var_os("STARTER_IAM_APP_SECRET").is_some()
                || std::env::var_os("STARTER_IAM_TEST_APP_SECRET").is_some())
        {
            return Err("Configure STARTER_AUTH_FILE and STARTER_AUTH_ENCRYPTION_KEY for durable IAM sessions".into());
        }
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        if std::env::var_os("STARTER_IAM_APP_SECRET").is_some()
            || std::env::var_os("STARTER_IAM_TEST_APP_SECRET").is_some()
        {
            self.iam().await?;
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
        if !bytes.starts_with(b"STARTER-IAM5\0") || bytes.len() < 25 {
            return Err("Legacy or invalid STARTER_AUTH_FILE; retain its backup and use a new encrypted session store, then sign in again".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "Invalid encryption key")?;
        let clear = cipher
            .decrypt(
                (&bytes[13..25]).into(),
                aes_gcm::aead::Payload {
                    msg: &bytes[25..],
                    aad: b"starter-iam5-session-store",
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
                    aad: b"starter-iam5-session-store",
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
            file.write_all(b"STARTER-IAM5\0")
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
    pub async fn begin_browser_login(
        &self,
        existing: Option<&str>,
        identity_kind: &str,
        return_to: String,
        popup: bool,
    ) -> Result<(String, String), String> {
        if !matches!(identity_kind, "carbon" | "silicon") {
            return Err("Choose Carbon or Silicon before starting login".into());
        }
        self.iam().await?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let group = existing
            .filter(|v| data.groups.contains(&hash(v)))
            .map(str::to_owned)
            .unwrap_or_else(random_secret);
        data.groups.insert(hash(&group));
        let nonce = random_secret();
        data.states
            .retain(|_, s| s.expires_at > Utc::now().timestamp());
        data.states.insert(
            hash(&nonce),
            LoginState {
                group: hash(&group),
                expires_at: Utc::now().timestamp() + 600,
                slt_hash: None,
                slt: None,
                attempt: Some(BrowserAttempt {
                    attempt_id: Uuid::new_v4().to_string(),
                    identity_kind: identity_kind.into(),
                    return_to,
                    popup,
                }),
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
            .filter(|ticket| {
                ticket.group == hash(group) && ticket.expires_at > Utc::now().timestamp()
            })
            .and_then(|ticket| ticket.attempt.clone())
            .ok_or("Login state is missing, expired, or belongs to another browser".into())
    }
    pub async fn resume_browser_login(&self, nonce: &str, group: &str) -> Result<String, String> {
        let (slt, session) = {
            let mut data = self.inner.lock().await;
            let _lock = self.file_lock().await?;
            self.reload(&mut data)?;
            let ticket = data
                .states
                .get(&hash(nonce))
                .filter(|ticket| {
                    ticket.group == hash(group) && ticket.expires_at > Utc::now().timestamp()
                })
                .ok_or("Login state is missing, expired, or belongs to another browser")?;
            let session = ticket
                .slt_hash
                .as_ref()
                .and_then(|key| data.logins.get(key))
                .filter(|receipt| receipt.completed)
                .map(|receipt| receipt.session.clone());
            (ticket.slt.clone(), session)
        };
        if let Some(session) = session {
            self.get(&session)
                .await?
                .ok_or("This login is no longer active; begin again")?;
            return Ok(session);
        }
        let slt = slt.ok_or("No recoverable login exists; begin a new IAM login")?;
        self.login(&slt, Some((nonce, group)), None, None).await
    }
    pub async fn login(
        &self,
        slt: &str,
        browser: Option<(&str, &str)>,
        cli_key: Option<&str>,
        org: Option<&str>,
    ) -> Result<String, String> {
        let iam = self.iam().await?;
        let is_actor = valid_actor_id("carbon", slt) || valid_actor_id("silicon", slt);
        if !slt.starts_with("oac_")
            && !(is_actor && iam.world.environment_id.is_some() && org.is_some_and(valid_org))
        {
            return Err("Login requires an IAM short-lived token; testing actors require a verified testing world and explicit organization".into());
        }
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let login_hash = if is_actor {
            // A testing actor is reusable, but one CLI attempt still has an immutable request receipt.
            self.receipt_key(
                "test-login",
                &format!(
                    "{}|{}|{}|{}",
                    iam.world.fingerprint(),
                    cli_key.unwrap_or_default(),
                    slt,
                    org.unwrap_or_default()
                ),
            )
        } else {
            self.receipt_key("login", slt)
        };
        let (binding, group, requested_kind) = if let Some((nonce, group)) = browser {
            let ticket = data
                .states
                .get_mut(&hash(nonce))
                .ok_or("Login state is missing or expired")?;
            if ticket.group != hash(group)
                || ticket.expires_at <= Utc::now().timestamp()
                || ticket.slt_hash.as_ref().is_some_and(|v| v != &login_hash)
            {
                return Err("Login state does not match the initiating browser".into());
            }
            ticket.slt_hash = Some(login_hash.clone());
            let kind = ticket
                .attempt
                .as_ref()
                .filter(|attempt| matches!(attempt.identity_kind.as_str(), "carbon" | "silicon"))
                .ok_or("Begin a new login and choose Carbon or Silicon")?
                .identity_kind
                .clone();
            (
                format!("browser:{}:{}", hash(group), hash(nonce)),
                Some(hash(group)),
                Some(kind),
            )
        } else {
            let key = cli_key
                .filter(|k| Uuid::parse_str(k).is_ok())
                .ok_or("CLI login requires a UUID Idempotency-Key")?;
            (format!("cli:{key}"), None, None)
        };
        let input_hash = self.receipt_key(
            "login-input",
            &format!(
                "{}|{}|{}",
                iam.world.fingerprint(),
                slt,
                org.unwrap_or_default()
            ),
        );
        if data
            .attempts
            .get(&binding)
            .is_some_and(|existing| existing != &input_hash)
        {
            return Err(
                "This login attempt already belongs to different input; begin a new login".into(),
            );
        }
        data.attempts.insert(binding.clone(), input_hash);
        if let Some(receipt) = data.logins.get(&login_hash) {
            if receipt.expires_at <= Utc::now().timestamp() {
                return Err("Login recovery expired; begin a new IAM login".into());
            }
            if receipt.binding != binding {
                return Err("Login credential belongs to another login attempt".into());
            }
            if receipt.completed {
                return data
                    .sessions
                    .get(&hash(&receipt.session))
                    .filter(|row| {
                        row.tokens.world == iam.world
                            && org.is_none_or(|org| row.tokens.org_id.as_deref() == Some(org))
                    })
                    .map(|_| receipt.session.clone())
                    .ok_or("This login was signed out; start a new login".into());
            }
        } else {
            data.logins.insert(
                login_hash.clone(),
                LoginReceipt {
                    binding,
                    expires_at: Utc::now().timestamp() + 600,
                    session: random_secret(),
                    candidate: None,
                    completed: false,
                },
            );
        }
        if let Some((nonce, _)) = browser {
            // The encrypted attempt recovers a lost exchange without exposing its SLT again.
            data.states
                .get_mut(&hash(nonce))
                .expect("bound login state")
                .slt = Some(slt.to_owned());
        }
        self.persist(&data)?;
        if data.logins[&login_hash].candidate.is_none() {
            let mutation = mutation(&login_hash)?;
            let pair = if is_actor {
                iam.client
                    .oauth()
                    .login_testing_actor(&iam.app_id, slt, org.expect("checked"), &mutation)
                    .await
            } else {
                iam.client.oauth().login(&iam.app_id, slt, &mutation).await
            }
            .map_err(|_| "IAM login could not complete; retry the same attempt")?;
            let tokens = tokens_of(pair, &iam.world)?;
            data.logins.get_mut(&login_hash).expect("receipt").candidate = Some(tokens);
            self.persist(&data)?;
        }
        let mut tokens = data.logins[&login_hash]
            .candidate
            .clone()
            .expect("candidate");
        if org.is_some_and(|org| tokens.org_id.as_deref() != Some(org)) {
            return Err("The IAM login belongs to another organization".into());
        }
        if tokens.world != iam.world {
            return Err("Login belongs to a previous IAM data world".into());
        }
        if !prove(&iam, &mut tokens).await? {
            return Err("IAM login is no longer active; start a new login".into());
        }
        let actor = tokens
            .actor
            .as_ref()
            .ok_or("IAM did not prove the login identity")?;
        if requested_kind
            .as_deref()
            .is_some_and(|kind| actor["type"].as_str() != Some(kind))
        {
            return Err("IAM identity kind does not match the chosen login button".into());
        }
        if is_actor && actor["public_id"].as_str() != Some(slt) {
            return Err("IAM testing login returned a different account".into());
        }
        let receipt = data.logins.get_mut(&login_hash).expect("receipt");
        receipt.completed = true;
        receipt.candidate = None;
        let id = receipt.session.clone();
        data.sessions.insert(
            hash(&id),
            Session {
                secret: id.clone(),
                tokens,
                group,
                pending_pair: false,
            },
        );
        if let Some((nonce, _)) = browser {
            data.states
                .get_mut(&hash(nonce))
                .expect("bound login state")
                .slt = None;
        }
        self.persist(&data)?;
        Ok(id)
    }
    #[cfg(test)]
    pub async fn insert(&self, tokens: IamTokens) -> Result<String, String> {
        let id = random_secret();
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        data.sessions.insert(
            hash(&id),
            Session {
                secret: id.clone(),
                tokens,
                group: None,
                pending_pair: false,
            },
        );
        self.persist(&data)?;
        Ok(id)
    }
    pub async fn get(&self, id: &str) -> Result<Option<IamTokens>, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let Some(mut row) = data.sessions.get(&hash(id)).cloned() else {
            return Ok(None);
        };
        if !row.tokens.valid() {
            return Ok(None);
        }
        #[cfg(test)]
        if self.fixture {
            return Ok(row
                .tokens
                .expires_at
                .filter(|t| *t > Utc::now().timestamp())
                .map(|_| row.tokens));
        }
        let iam = self.iam().await?;
        if row.tokens.world != iam.world {
            return Ok(None);
        }
        if !row.pending_pair
            && row
                .tokens
                .expires_at
                .is_none_or(|t| t <= Utc::now().timestamp())
        {
            let mutation = mutation(&self.receipt_key("refresh", &row.tokens.refresh_token))?;
            let pair = iam
                .client
                .oauth()
                .refresh(&iam.app_id, &row.tokens.refresh_token, &mutation)
                .await
                .map_err(|_| "IAM refresh could not complete; retry in this account")?;
            let mut next = tokens_of(pair, &iam.world)?;
            if next
                .actor
                .as_ref()
                .is_some_and(|actor| Some(actor) != row.tokens.actor.as_ref())
                || next.org_id != row.tokens.org_id
            {
                return Err("IAM refresh changed the immutable account or organization".into());
            }
            // Missing exchange identity is verified by introspection against the saved account.
            if next.actor.is_none() {
                next.actor = row.tokens.actor.clone();
            }
            next.context_id = row.tokens.context_id.clone();
            row.tokens = next;
            row.pending_pair = true;
            // Persist the rotated credentials before making another network request. A failed
            // introspection retries this exact pair, never the consumed refresh credential.
            data.sessions.insert(hash(id), row.clone());
            self.persist(&data)?;
        }
        if !prove(&iam, &mut row.tokens).await? {
            return Ok(None);
        }
        row.pending_pair = false;
        data.sessions.insert(hash(id), row.clone());
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
        let iam = self.iam().await?;
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let group = group.map(hash);
        let mut rows: Vec<Value> = data
            .sessions
            .values()
            .filter(|s| {
                group.is_some()
                    && s.group == group
                    && s.tokens.world == iam.world
                    && s.tokens.valid()
            })
            .map(|s| {
                let mut row = s.tokens.status();
                row["selected"] = json!(selected.is_some_and(|id| hash(id) == hash(&s.secret)));
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
            .ok_or("Saved account needs a new IAM login")?;
        Ok(secret)
    }
    pub async fn remove(&self, id: &str) -> Result<bool, String> {
        let mut data = self.inner.lock().await;
        let _lock = self.file_lock().await?;
        self.reload(&mut data)?;
        let Some(row) = data.sessions.get(&hash(id)) else {
            return Ok(false);
        };
        #[cfg(test)]
        let remote = !self.fixture;
        #[cfg(not(test))]
        let remote = true;
        if remote {
            let iam = self.iam().await?;
            if row.tokens.world == iam.world {
                let mutation = mutation(&self.receipt_key("logout", &row.tokens.refresh_token))?;
                iam.client
                    .oauth()
                    .revoke(
                        &models::OAuthRevocationRequest {
                            token: row.tokens.refresh_token.clone(),
                            token_type_hint: Some(
                                models::OAuthRevocationRequestTokenTypeHint::RefreshToken,
                            ),
                        },
                        &mutation,
                    )
                    .await
                    .map_err(|_| "IAM logout could not complete; retry in this account")?;
            }
        }
        data.sessions.remove(&hash(id));
        self.persist(&data)?;
        Ok(true)
    }
}
fn mutation(key: &str) -> Result<Mutation, String> {
    Ok(Mutation::with_key(
        silicon_iam_client::IdempotencyKey::parse(key).map_err(|_| "Invalid operation receipt")?,
    ))
}
fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
pub fn valid_org(org: &str) -> bool {
    (3..=50).contains(&org.len())
        && org
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
pub fn valid_login_state(expected: Option<&str>, supplied: Option<&str>) -> bool {
    matches!((expected,supplied),(Some(a),Some(b)) if a.len()==64 && a==b)
}
fn tokens_of(pair: models::OAuthTokenResponse, world: &World) -> Result<IamTokens, String> {
    let actor = pair
        .actor
        .map(|a| serde_json::to_value(a).expect("actor serialization"));
    let tokens = IamTokens {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        expires_in: pair.expires_in,
        expires_at: None,
        actor,
        org_ids: pair.org_id.iter().cloned().collect(),
        org_id: pair.org_id,
        context_id: Uuid::new_v4().to_string(),
        world: world.clone(),
    };
    if tokens.organizations().len() != 1
        || tokens.access_token.is_empty()
        || tokens.refresh_token.is_empty()
        || tokens.actor.as_ref().is_some_and(|actor| {
            !valid_actor_id(
                actor["type"].as_str().unwrap_or_default(),
                actor["public_id"].as_str().unwrap_or_default(),
            )
        })
        || pair.token_type != "Bearer"
        || pair.expires_in <= 0
        || pair.scope.split_whitespace().any(|s| s.starts_with("obo:"))
    {
        return Err("IAM 5 login requires one canonical account and organization with ordinary session credentials".into());
    }
    Ok(tokens)
}
async fn prove(iam: &Iam, tokens: &mut IamTokens) -> Result<bool, String> {
    let seen = iam
        .client
        .oauth()
        .introspect(
            &models::TokenIntrospectionRequest {
                token: tokens.access_token.clone(),
                token_type_hint: Some(models::TokenIntrospectionRequestTokenTypeHint::AccessToken),
            },
            tokens.org_id.as_deref(),
        )
        .await
        .map_err(|_| "IAM session verification is unavailable")?;
    validate_proof(&seen, &iam.app_id, tokens)
}
fn validate_proof(
    seen: &models::TokenIntrospection,
    app: &str,
    tokens: &mut IamTokens,
) -> Result<bool, String> {
    if !seen.active {
        return Ok(false);
    }
    let a = seen
        .authorization
        .as_ref()
        .ok_or("IAM 5 requires one authorization snapshot; sign in again")?;
    let proven_kind =
        serde_json::to_value(&seen.actor_type).map_err(|_| "Missing session identity")?;
    let proven_kind = proven_kind.as_str().ok_or("Missing session identity")?;
    let proven_id = seen
        .public_id
        .as_deref()
        .ok_or("Missing session identity")?;
    if !valid_actor_id(proven_kind, proven_id) {
        return Err("IAM did not prove a canonical Carbon or Silicon identity".into());
    }
    let proven_actor = json!({"type":proven_kind,"public_id":proven_id});
    let actor = tokens.actor.as_ref().unwrap_or(&proven_actor);
    let kind = actor["type"].as_str().ok_or("Missing session identity")?;
    let id = actor["public_id"]
        .as_str()
        .ok_or("Missing session identity")?;
    let org = tokens
        .org_id
        .as_deref()
        .ok_or("Missing session organization")?;
    let membership = format!("{id}[{org}]");
    if seen.authorizations.is_some()
        || seen.public_id.as_deref() != Some(id)
        || serde_json::to_value(&seen.actor_type)
            .ok()
            .as_ref()
            .and_then(Value::as_str)
            != Some(kind)
        || serde_json::to_value(&a.actor_type)
            .ok()
            .as_ref()
            .and_then(Value::as_str)
            != Some(kind)
        || a.public_id.as_deref() != Some(id)
        || seen.org_id.as_deref() != Some(org)
        || a.org_id != org
        || seen.client_id.as_deref() != Some(app)
        || seen.audience.as_deref() != Some(app)
        || a.audience != app
        || a.membership_id != membership
        || seen.membership_id.as_deref() != Some(membership.as_str())
        || a.organization_id.is_nil()
        || a.membership_version < 1
        || a.authorization_epoch < 1
        || seen.authorization_epoch != Some(a.authorization_epoch)
        || a.testing_environment_id != tokens.world.environment_id
        || seen
            .scope
            .as_deref()
            .is_some_and(|s| s.split_whitespace().any(|s| s.starts_with("obo:")))
        || a.scopes.iter().any(|s| s.starts_with("obo:"))
        || seen
            .expires_at
            .is_none_or(|expiry| expiry <= Utc::now().timestamp())
    {
        return Err(
            "IAM returned a different or invalid ordinary account, organization, or data world"
                .into(),
        );
    }
    tokens.expires_at = seen.expires_at;
    tokens.actor = Some(proven_actor);
    Ok(true)
}

pub(crate) fn validate_app_id(app_id: &str) -> Result<(), String> {
    if (1..=80).contains(&app_id.len())
        && app_id.as_bytes()[0].is_ascii_lowercase()
        && app_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err("application ID must be a bare IAM handle (1–80 lowercase letters, digits, underscores or hyphens, starting with a letter); migrate legacy org>app IDs using the authoritative IAM mapping".into())
    }
}

pub(crate) fn valid_actor_id(kind: &str, id: &str) -> bool {
    let (prefix, max_len) = match kind {
        "carbon" => ("c:", 30),
        "silicon" => ("si:", 50),
        _ => return false,
    };
    id.strip_prefix(prefix).is_some_and(|handle| {
        (3..=max_len).contains(&handle.len())
            && handle
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    })
}

/// Verify IAM's signature over the exact, unparsed request body.
/// `now` is injectable so replay-window tests do not depend on wall clock.
pub fn verify_webhook(headers: &HeaderMap, body: &[u8], secret: &[u8], now: i64) -> bool {
    if !single_header(headers, "x-silicon-iam-key-version")
        .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let ts = match single_header(headers, "x-silicon-iam-timestamp")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(v) => v,
        None => return false,
    };
    if now.abs_diff(ts) > 300 {
        return false;
    }
    let signature = match single_header(headers, "x-silicon-iam-signature") {
        Some(v) => v,
        None => return false,
    };
    let digest = match signature.strip_prefix("v1=") {
        Some(v)
            if v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            v
        }
        _ => return false,
    };
    let expected = match hex::decode(digest) {
        Ok(v) if v.len() == 32 => v,
        _ => return false,
    };
    let mut mac = match <HmacSha256 as Mac>::new_from_slice(secret) {
        Ok(v) => v,
        Err(_) => return false,
    };
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

pub fn verify_webhook_now(headers: &HeaderMap, body: &[u8], secret: &[u8]) -> bool {
    verify_webhook(headers, body, secret, Utc::now().timestamp())
}

/// Extract the authenticated event ID after signature verification. Test envelopes nest metadata.
pub fn webhook_event_id(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .pointer("/metadata/event_id")
        .or_else(|| value.pointer("/test/metadata/event_id"))?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
