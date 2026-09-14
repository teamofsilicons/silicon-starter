use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub mod local;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Starter {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner: String,
    pub visibility: Visibility,
    pub version: String,
    pub downloads: u64,
    pub stars: u64,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub yaml: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Private,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version {
    pub version: String,
    pub commit: String,
    pub notes: String,
    pub published_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Discussion {
    pub id: String,
    pub starter_id: String,
    pub parent_id: Option<String>,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CreateStarter {
    pub id: String,
    #[serde(default)]
    pub org_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub visibility: Visibility,
    pub yaml: String,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CreateDiscussion {
    pub body: String,
    pub parent_id: Option<String>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// Parse the public `Y.X` release notation and return numeric components.
pub fn release_version(value: &str) -> Result<(u64, u64), String> {
    let (major, minor) = value
        .split_once('.')
        .ok_or_else(|| "version must use Y.X notation".to_string())?;
    if major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(|b| b.is_ascii_digit())
        || !minor.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("version must use Y.X notation with numeric components".into());
    }
    Ok((
        major
            .parse()
            .map_err(|_| "version component is too large")?,
        minor
            .parse()
            .map_err(|_| "version component is too large")?,
    ))
}
pub fn validate_silicon_yaml(text: &str) -> Result<(), String> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| format!("silicon.yaml is invalid YAML: {e}"))?;
    let map = value
        .as_mapping()
        .ok_or("silicon.yaml must contain a mapping at its root")?;
    for key in ["silicon", "isi", "access", "flow"] {
        if !map.contains_key(serde_yaml::Value::String(key.into())) {
            return Err(format!(
                "silicon.yaml is missing required top-level key `{key}`"
            ));
        }
    }
    if !map[&serde_yaml::Value::String("silicon".into())].is_mapping() {
        return Err("silicon must be a mapping".into());
    }
    if !map[&serde_yaml::Value::String("isi".into())].is_mapping() {
        return Err("isi must be a mapping".into());
    }
    Ok(())
}
pub fn validate_directory(path: &Path) -> Result<(), String> {
    let yaml = path.join("silicon.yaml");
    validate_silicon_yaml(
        &fs::read_to_string(&yaml).map_err(|e| format!("cannot read {}: {e}", yaml.display()))?,
    )
}
pub const SEED_YAML: &str = "silicon:\n  id: starter:tos\n  token: local-development-token\n  timezone: UTC\n  SILICON_HOME: .\n  inference_providers: [all-available-providers]\nisi:\n  registry:\n    model: fast\n    primary_send_mode: global\n    session_type: persistent\n    dna:\n      assemble: []\naccess:\n  registry: []\nflow: []\n";
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_contract() {
        assert!(validate_silicon_yaml(SEED_YAML).is_ok());
        assert!(valid_id("tos.hello-world"));
        assert!(!valid_id("../secret"));
        assert!(local::is_hex_commit(&"a".repeat(40)));
        assert!(!local::is_hex_commit("main"));
        assert_eq!(release_version("2.7"), Ok((2, 7)));
        assert!(release_version("2").is_err());
    }
}
