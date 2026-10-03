//! Encrypted, synchronous-durability feature receipts. SQLite leases coordinate
//! independent backend processes; every write checks the original lease owner.
use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct FeatureStore {
    db: Arc<Mutex<Connection>>,
    cipher: Aes256Gcm,
}
impl FeatureStore {
    pub(crate) fn from_env() -> Result<Self, String> {
        let file = std::env::var_os("STARTER_AUTH_FILE")
            .ok_or("STARTER_AUTH_FILE is required for durable feature permission")?;
        let path = format!("{}.features.sqlite", Path::new(&file).display());
        let key = std::env::var("STARTER_AUTH_ENCRYPTION_KEY")
            .map_err(|_| "STARTER_AUTH_ENCRYPTION_KEY is required")?;
        let key: [u8; 32] = hex::decode(key)
            .map_err(|_| "Encryption key must be 64 hex characters")?
            .try_into()
            .map_err(|_| "Encryption key must be 64 hex characters")?;
        Self::open(Path::new(&path), &key)
    }
    pub(crate) fn open(path: &Path, key: &[u8; 32]) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| "Cannot create feature-store directory")?;
        }
        let db = Connection::open(path).map_err(|_| "Cannot open feature store")?;
        db.busy_timeout(Duration::from_secs(5))
            .map_err(|_| "Cannot configure feature store")?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS feature_items(namespace TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,value BLOB NOT NULL,PRIMARY KEY(namespace,kind,id)); CREATE TABLE IF NOT EXISTS feature_locks(namespace TEXT PRIMARY KEY,owner TEXT NOT NULL,expires_at INTEGER NOT NULL);").map_err(|_| "Cannot initialize feature store")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "Cannot restrict feature-store permissions")?;
        }
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)),
        })
    }
    pub(crate) fn lease(&self, namespace: &str) -> Result<Lease, String> {
        let owner = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let changed = self.db.lock().map_err(|_| "Feature store lock failed")?.execute("INSERT INTO feature_locks(namespace,owner,expires_at) VALUES(?1,?2,?3) ON CONFLICT(namespace) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at WHERE feature_locks.expires_at<?4", params![namespace, owner, now+120, now]).map_err(|_| "Feature lease could not be acquired")?;
        if changed != 1 {
            return Err("Another feature operation is running; retry the same request".into());
        }
        Ok(Lease {
            store: self.clone(),
            namespace: namespace.into(),
            owner,
        })
    }
    fn seal(&self, namespace: &str, kind: &str, id: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        let aad = serde_json::to_vec(&("starter-feature-v1", namespace, kind, id))
            .map_err(|_| "Invalid feature binding")?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut out = nonce.to_vec();
        out.extend(
            self.cipher
                .encrypt(
                    &nonce,
                    Payload {
                        msg: data,
                        aad: &aad,
                    },
                )
                .map_err(|_| "Feature encryption failed")?,
        );
        Ok(out)
    }
    fn open_value(
        &self,
        namespace: &str,
        kind: &str,
        id: &str,
        data: &[u8],
    ) -> Result<Vec<u8>, String> {
        if data.len() < 28 {
            return Err("Invalid encrypted feature state".into());
        }
        let aad = serde_json::to_vec(&("starter-feature-v1", namespace, kind, id))
            .map_err(|_| "Invalid feature binding")?;
        self.cipher
            .decrypt(
                Nonce::from_slice(&data[..12]),
                Payload {
                    msg: &data[12..],
                    aad: &aad,
                },
            )
            .map_err(|_| "Feature state does not match its encryption key or context".into())
    }
}
pub(crate) struct Lease {
    store: FeatureStore,
    namespace: String,
    owner: String,
}
impl Lease {
    fn held(&self, db: &Connection) -> Result<(), String> {
        let held: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM feature_locks WHERE namespace=?1 AND owner=?2 AND expires_at>=?3)", params![self.namespace,self.owner,chrono::Utc::now().timestamp()], |r|r.get(0)).map_err(|_| "Could not verify feature lease")?;
        if held {
            Ok(())
        } else {
            Err("Feature lease expired; retry the same operation".into())
        }
    }
    pub(crate) fn get_bytes(&self, kind: &str, id: &str) -> Result<Option<Vec<u8>>, String> {
        let db = self
            .store
            .db
            .lock()
            .map_err(|_| "Feature store lock failed")?;
        self.held(&db)?;
        let bytes: Option<Vec<u8>> = db
            .query_row(
                "SELECT value FROM feature_items WHERE namespace=?1 AND kind=?2 AND id=?3",
                params![self.namespace, kind, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Could not read feature state")?;
        bytes
            .map(|bytes| self.store.open_value(&self.namespace, kind, id, &bytes))
            .transpose()
    }
    pub(crate) fn get<T: DeserializeOwned>(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<Option<T>, String> {
        self.get_bytes(kind, id)?
            .map(|v| {
                serde_json::from_slice(&v).map_err(|_| "Saved feature state is invalid".into())
            })
            .transpose()
    }
    pub(crate) fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<(), String> {
        self.put_many(vec![(
            kind.into(),
            id.into(),
            serde_json::to_vec(value).map_err(|_| "Could not encode feature state")?,
        )])
    }
    pub(crate) fn put_many(&self, values: Vec<(String, String, Vec<u8>)>) -> Result<(), String> {
        let mut db = self
            .store
            .db
            .lock()
            .map_err(|_| "Feature store lock failed")?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "Could not begin feature update")?;
        self.held(&tx)?;
        for (kind, id, bytes) in values {
            let sealed = self.store.seal(&self.namespace, &kind, &id, &bytes)?;
            tx.execute("INSERT INTO feature_items(namespace,kind,id,value) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,kind,id) DO UPDATE SET value=excluded.value",params![self.namespace,kind,id,sealed]).map_err(|_|"Could not save feature state")?;
        }
        tx.commit()
            .map_err(|_| "Could not commit feature state".into())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(db) = self.store.db.lock() {
            let _ = db.execute(
                "DELETE FROM feature_locks WHERE namespace=?1 AND owner=?2",
                params![self.namespace, self.owner],
            );
        }
    }
}
