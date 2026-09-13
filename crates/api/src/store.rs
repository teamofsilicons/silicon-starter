//! Durable application snapshots for the single-instance Starter API.
//! Mutations are serialized by the caller; this store only persists one JSON row.

use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    /// Connect using `STARTER_DATABASE_URL` or `DATABASE_URL`.
    /// Missing configuration keeps local development in the in-memory mode.
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
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS starter_state (
                id SMALLINT PRIMARY KEY,
                payload JSONB NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        )
        .execute(&pool)
        .await?;
        Ok(Some(Self { pool }))
    }

    pub async fn load(&self) -> Result<Option<Value>, sqlx::Error> {
        sqlx::query_scalar::<_, Value>("SELECT payload FROM starter_state WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn save(&self, payload: Value) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO starter_state (id, payload, updated_at) VALUES (1, $1, now())
             ON CONFLICT (id) DO UPDATE SET payload = EXCLUDED.payload, updated_at = now()",
        )
        .bind(payload)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}
