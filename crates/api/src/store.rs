//! Durable catalog snapshots partitioned by immutable IAM world generation.
use crate::iam::World;
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    world: World,
}
impl Store {
    pub async fn connect_from_env(world: &World) -> Result<Option<Self>, sqlx::Error> {
        let Some(url) =
            std::env::var_os("STARTER_DATABASE_URL").or_else(|| std::env::var_os("DATABASE_URL"))
        else {
            return Ok(None);
        };
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url.to_string_lossy())
            .await?;
        Self::from_pool(pool, world.clone()).await.map(Some)
    }
    pub(crate) async fn from_pool(pool: PgPool, world: World) -> Result<Self, sqlx::Error> {
        sqlx::query("CREATE TABLE IF NOT EXISTS starter_world_state (world_fingerprint TEXT PRIMARY KEY,payload JSONB NOT NULL,updated_at TIMESTAMPTZ NOT NULL DEFAULT now())").execute(&pool).await?;
        // The historical singleton belongs only to production. Testing never
        // reads or copies it, even on a shared database connection.
        if world == World::default() {
            sqlx::query("CREATE TABLE IF NOT EXISTS starter_state (id SMALLINT PRIMARY KEY,payload JSONB NOT NULL,updated_at TIMESTAMPTZ NOT NULL DEFAULT now())").execute(&pool).await?;
            sqlx::query("INSERT INTO starter_world_state(world_fingerprint,payload) SELECT $1,payload FROM starter_state WHERE id=1 ON CONFLICT(world_fingerprint) DO NOTHING").bind(world.fingerprint()).execute(&pool).await?;
        }
        Ok(Self { pool, world })
    }
    pub async fn load(&self) -> Result<Option<Value>, sqlx::Error> {
        sqlx::query_scalar("SELECT payload FROM starter_world_state WHERE world_fingerprint=$1")
            .bind(self.world.fingerprint())
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn save(&self, mut payload: Value) -> Result<(), sqlx::Error> {
        payload["world"] =
            serde_json::to_value(&self.world).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        sqlx::query("INSERT INTO starter_world_state(world_fingerprint,payload,updated_at) VALUES($1,$2,now()) ON CONFLICT(world_fingerprint) DO UPDATE SET payload=EXCLUDED.payload,updated_at=now()").bind(self.world.fingerprint()).bind(payload).execute(&self.pool).await.map(|_|())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn postgres_partitions_production_testing_and_clean_generations() {
        let Ok(url) = std::env::var("STARTER_TEST_DATABASE_URL") else {
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("starter_iam5_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(&format!("SET search_path TO {schema}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE starter_state(id SMALLINT PRIMARY KEY,payload JSONB NOT NULL,updated_at TIMESTAMPTZ NOT NULL DEFAULT now())").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO starter_state(id,payload) VALUES(1,$1)")
            .bind(json!({"legacy_production":true}))
            .execute(&pool)
            .await
            .unwrap();
        let env = uuid::Uuid::new_v4();
        let world = World {
            id: format!("testing:{env}"),
            environment_id: Some(env),
            key_generation: 1,
            cleaned_at: None,
            version: 1,
        };
        let testing = Store::from_pool(pool.clone(), world.clone()).await.unwrap();
        assert!(
            testing.load().await.unwrap().is_none(),
            "never falls back to production singleton"
        );
        testing.save(json!({"test_only":true})).await.unwrap();
        let production = Store::from_pool(pool.clone(), World::default())
            .await
            .unwrap();
        assert_eq!(
            production.load().await.unwrap().unwrap()["legacy_production"],
            true
        );
        production
            .save(json!({"production_only":true}))
            .await
            .unwrap();
        let cleaned = World {
            key_generation: 2,
            cleaned_at: Some("2026-10-03T00:00:00Z".into()),
            version: 2,
            ..world.clone()
        };
        let fresh = Store::from_pool(pool.clone(), cleaned).await.unwrap();
        assert!(
            fresh.load().await.unwrap().is_none(),
            "clean generation cannot reopen old rows"
        );
        fresh.save(json!({"new_generation":true})).await.unwrap();
        assert_eq!(testing.load().await.unwrap().unwrap()["test_only"], true);
        assert_eq!(
            production.load().await.unwrap().unwrap()["production_only"],
            true
        );
        let state = crate::AppState {
            world: std::sync::Arc::new(world),
            ..Default::default()
        };
        assert!(
            crate::restore_state(&state, production.load().await.unwrap().unwrap())
                .await
                .is_err()
        );
        assert!(
            crate::restore_state(&state, json!({"starters":{}}))
                .await
                .is_err()
        );
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
