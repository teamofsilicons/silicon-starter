//! One durable catalog for Carbon and Silicon accounts.
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}
impl Store {
    pub async fn connect_from_env() -> Result<Option<Self>, sqlx::Error> {
        let Some(url) =
            std::env::var_os("STARTER_DATABASE_URL").or_else(|| std::env::var_os("DATABASE_URL"))
        else {
            return Ok(None);
        };
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url.to_string_lossy())
            .await?;
        sqlx::query("CREATE TABLE IF NOT EXISTS starter_accounts_state (id SMALLINT PRIMARY KEY CHECK(id=1),payload JSONB NOT NULL,updated_at TIMESTAMPTZ NOT NULL DEFAULT now())").execute(&pool).await?;
        Ok(Some(Self { pool }))
    }
    pub async fn load(&self) -> Result<Option<Value>, sqlx::Error> {
        if let Some(payload) =
            sqlx::query_scalar("SELECT payload FROM starter_accounts_state WHERE id=1")
                .fetch_optional(&self.pool)
                .await?
        {
            return Ok(Some(payload));
        }
        // Read the former production catalog without committing an import until
        // restore succeeds. Retain the original tables for rollback.
        let old: Option<String> =
            sqlx::query_scalar("SELECT to_regclass('starter_world_state')::text")
                .fetch_one(&self.pool)
                .await?;
        if old.is_some() {
            let payload = sqlx::query_scalar("SELECT payload FROM starter_world_state WHERE world_fingerprint::jsonb->>'id'='production' AND world_fingerprint::jsonb->>'environment_id' IS NULL ORDER BY updated_at DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
            if payload.is_some() {
                return Ok(payload);
            }
        }
        let old: Option<String> = sqlx::query_scalar("SELECT to_regclass('starter_state')::text")
            .fetch_one(&self.pool)
            .await?;
        if old.is_some() {
            return sqlx::query_scalar("SELECT payload FROM starter_state WHERE id=1")
                .fetch_optional(&self.pool)
                .await;
        }
        Ok(None)
    }
    pub async fn save(&self, payload: Value) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO starter_accounts_state(id,payload,updated_at) VALUES(1,$1,now()) ON CONFLICT(id) DO UPDATE SET payload=EXCLUDED.payload,updated_at=now()").bind(payload).execute(&self.pool).await.map(|_| ())
    }
}
