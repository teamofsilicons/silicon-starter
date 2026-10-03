//! One configured IAM world per Starter backend. Testing clean/rotation metadata
//! is part of the immutable storage binding, not merely the environment UUID.
use serde::{Deserialize, Serialize};
use silicon_iam_client::{Client, Credential, EnvironmentKey, models};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct World {
    pub id: String,
    pub environment_id: Option<Uuid>,
    pub key_generation: i64,
    pub cleaned_at: Option<String>,
    pub version: i64,
}
impl Default for World {
    fn default() -> Self {
        Self {
            id: "production".into(),
            environment_id: None,
            key_generation: 0,
            cleaned_at: None,
            version: 0,
        }
    }
}
impl World {
    pub fn fingerprint(&self) -> String {
        // Public identity only: no app secrets or environment keys.
        serde_json::to_string(self).expect("World serialization is infallible")
    }
    fn testing(context: &models::ApplicationTestingContext, app: &str) -> Result<Self, String> {
        let metadata = context
            .environment
            .as_ref()
            .ok_or("IAM omitted testing generation metadata")?;
        if context.environment_id.is_nil()
            || context.application.app_id != app
            || metadata.environment_id != context.environment_id
            || metadata.key_generation < 1
            || metadata.version < 1
        {
            return Err("IAM returned a mismatched testing context".into());
        }
        Ok(Self {
            id: format!("testing:{}", context.environment_id),
            environment_id: Some(context.environment_id),
            key_generation: metadata.key_generation,
            cleaned_at: metadata.cleaned_at.map(|at| at.to_string()),
            version: metadata.version,
        })
    }
}

#[derive(Clone)]
pub struct Iam {
    pub client: Client,
    pub app_id: String,
    pub world: World,
    base: Client,
}
impl Iam {
    pub async fn from_env() -> Result<Self, String> {
        let base = std::env::var("IAM_URL")
            .unwrap_or_else(|_| "https://backend.iam.teamofsilicons.com".into());
        let app = std::env::var("STARTER_IAM_APP_ID").unwrap_or_else(|_| "starter".into());
        let test_secret = std::env::var("STARTER_IAM_TEST_APP_SECRET").ok();
        let test_key = std::env::var("STARTER_TESTING_ENVIRONMENT_KEY").ok();
        let testing = match (test_secret, test_key) {
            (None, None) => None,
            (Some(secret), Some(key)) => Some((secret, key)),
            _ => return Err("Testing requires STARTER_IAM_TEST_APP_SECRET and STARTER_TESTING_ENVIRONMENT_KEY together".into()),
        };
        let secret = std::env::var("STARTER_IAM_APP_SECRET").unwrap_or_default();
        Self::connect(&base, &app, &secret, testing).await
    }
    /// Explicit configuration makes isolated protocol tests independent of process env.
    pub async fn connect(
        base_url: &str,
        app_id: &str,
        secret: &str,
        testing: Option<(String, String)>,
    ) -> Result<Self, String> {
        crate::auth::validate_app_id(app_id)?;
        validate_url(base_url)?;
        let base = Client::builder(base_url)
            .map_err(|_| "Invalid IAM base URL")?
            .timeout(Duration::from_secs(10))
            .telemetry(false)
            .user_agent(concat!("silicon-starter/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "Could not initialize IAM client")?;
        let (client, world) = match testing {
            Some((secret, key)) => {
                let key =
                    EnvironmentKey::new(&key).map_err(|_| "Invalid testing environment key")?;
                let client = base
                    .with_testing_application(app_id, &secret)
                    .map_err(|_| "Invalid testing application credential")?
                    .with_environment(key)
                    .with_credential(Credential::application(app_id, secret));
                let context = client
                    .applications()
                    .testing_context()
                    .await
                    .map_err(|_| "Could not verify the testing environment")?;
                let world = World::testing(&context, app_id)?;
                (client, world)
            }
            None => {
                if secret.is_empty() {
                    return Err("STARTER_IAM_APP_SECRET is not configured".into());
                }
                (
                    base.with_credential(Credential::application(app_id, secret)),
                    World::default(),
                )
            }
        };
        Ok(Self {
            client,
            app_id: app_id.into(),
            world,
            base,
        })
    }
    pub async fn assert_current(&self) -> Result<(), String> {
        if self.world.environment_id.is_some() {
            let context = self
                .client
                .applications()
                .testing_context()
                .await
                .map_err(|_| "Could not verify current testing generation")?;
            if World::testing(&context, &self.app_id)? != self.world {
                return Err("Testing world changed or was cleaned; restart with its current configuration and sign in again".into());
            }
        }
        Ok(())
    }
    pub async fn validate_provider_context(
        &self,
        context: Option<&models::OboTestingContext>,
    ) -> Result<(), String> {
        match (&self.world.environment_id, context) {
            (None, None) => Ok(()),
            (Some(_), Some(context)) if context.app_id == "briefcase" => {
                let client = self
                    .base
                    .with_testing_application("briefcase", &context.app_secret)
                    .map_err(|_| "Invalid provider testing credential")?
                    .with_environment(
                        EnvironmentKey::new(&context.iam_test_key)
                            .map_err(|_| "Invalid provider environment key")?,
                    )
                    .with_credential(Credential::application("briefcase", &context.app_secret));
                let verified = client
                    .applications()
                    .testing_context()
                    .await
                    .map_err(|_| "Could not verify provider testing world")?;
                if World::testing(&verified, "briefcase")? == self.world {
                    Ok(())
                } else {
                    Err("Provider authority belongs to another testing generation".into())
                }
            }
            _ => Err("Provider authority belongs to another data world".into()),
        }
    }
}

pub fn validate_url(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Invalid service URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (!matches!(url.scheme(), "https") && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Service URLs require HTTPS, or HTTP on loopback for isolated tests".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };
    const KEY: &str = "0123456789abcdefghijklmnopqrstuv";
    fn context(app: &str, env: Uuid) -> Value {
        json!({"environment_id":env,"application":{"app_id":app,"base_url":"https://example.test","app_scope":{"iam":[],"external":[]},"webhook_scope":[],"testing_idle_days":30},"environment":{"environment_id":env,"org_id":"tos","name":"Starter protocol test","version":1,"key_generation":1,"created_at":"2026-10-03T00:00:00Z","creator_type":"carbon","creator_id":"c:author"}})
    }
    async fn mount(
        server: &MockServer,
        app: &str,
        secret: &str,
        key: &str,
        value: Arc<Mutex<Value>>,
    ) {
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{app}:{secret}"))
        );
        Mock::given(method("GET"))
            .and(path("/api/v1/application/testing-context"))
            .and(header("authorization", basic.as_str()))
            .and(header("x-testing-environment-key", key))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(value.lock().unwrap().clone())
            })
            .mount(server)
            .await;
    }
    #[test]
    fn world_requires_matching_metadata_and_each_clean_marker_changes_binding() {
        let value = context("starter", Uuid::new_v4());
        let original =
            World::testing(&serde_json::from_value(value.clone()).unwrap(), "starter").unwrap();
        for (field, new) in [
            ("version", json!(2)),
            ("key_generation", json!(2)),
            ("cleaned_at", json!("2026-10-03T01:00:00Z")),
        ] {
            let mut changed = value.clone();
            changed["environment"][field] = new;
            let other =
                World::testing(&serde_json::from_value(changed).unwrap(), "starter").unwrap();
            assert_ne!(other.fingerprint(), original.fingerprint());
        }
        for change in [
            json!(null),
            json!({"environment_id":Uuid::new_v4(),"org_id":"tos","name":"wrong","version":1,"key_generation":1,"created_at":"2026-10-03T00:00:00Z","creator_type":"carbon","creator_id":"c:author"}),
        ] {
            let mut changed = value.clone();
            changed["environment"] = change;
            assert!(World::testing(&serde_json::from_value(changed).unwrap(), "starter").is_err());
        }
        assert!(World::testing(&serde_json::from_value(value).unwrap(), "briefcase").is_err());
    }
    #[tokio::test]
    async fn cleaned_world_rejects_existing_runtime_even_when_environment_uuid_is_unchanged() {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(context("starter", Uuid::new_v4())));
        mount(
            &server,
            "starter",
            "ask_testSecrettestSecrettestSecrettestSecrettes",
            KEY,
            state.clone(),
        )
        .await;
        let iam = Iam::connect(
            &server.uri(),
            "starter",
            "",
            Some((
                "ask_testSecrettestSecrettestSecrettestSecrettes".into(),
                KEY.into(),
            )),
        )
        .await
        .unwrap();
        iam.assert_current().await.unwrap();
        state.lock().unwrap()["environment"]["cleaned_at"] = json!("2026-10-03T02:00:00Z");
        assert!(
            iam.assert_current()
                .await
                .unwrap_err()
                .contains("changed or was cleaned")
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
    #[tokio::test]
    async fn provider_selector_must_resolve_to_same_world_and_generation() {
        let server = MockServer::start().await;
        let env = Uuid::new_v4();
        let starter = Arc::new(Mutex::new(context("starter", env)));
        let provider = Arc::new(Mutex::new(context("briefcase", env)));
        mount(
            &server,
            "starter",
            "ask_testSecrettestSecrettestSecrettestSecrettes",
            KEY,
            starter,
        )
        .await;
        mount(
            &server,
            "briefcase",
            "ask_providerproviderproviderproviderproviderxxx",
            KEY,
            provider.clone(),
        )
        .await;
        let iam = Iam::connect(
            &server.uri(),
            "starter",
            "",
            Some((
                "ask_testSecrettestSecrettestSecrettestSecrettes".into(),
                KEY.into(),
            )),
        )
        .await
        .unwrap();
        let selector = models::OboTestingContext {
            app_id: "briefcase".into(),
            app_secret: "ask_providerproviderproviderproviderproviderxxx".into(),
            iam_test_key: KEY.into(),
        };
        iam.validate_provider_context(Some(&selector))
            .await
            .unwrap();
        assert!(iam.validate_provider_context(None).await.is_err());
        provider.lock().unwrap()["environment"]["key_generation"] = json!(2);
        assert!(
            iam.validate_provider_context(Some(&selector))
                .await
                .unwrap_err()
                .contains("another testing generation")
        );
        let production = Iam::connect(&server.uri(), "starter", "ask_production", None)
            .await
            .unwrap();
        assert!(
            production
                .validate_provider_context(Some(&selector))
                .await
                .is_err()
        );
    }
}
