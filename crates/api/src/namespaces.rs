use serde_json::{Value, json};
use silicon_starter_core::{blocks, canonical_starter_id};
use std::collections::HashMap;

/// Complete migration before restoring any live state; collisions must never overwrite a catalog.
pub(crate) fn migrate(mut snapshot: Value) -> Result<Value, String> {
    let object = snapshot.as_object_mut().ok_or("invalid starter snapshot")?;
    let mut aliases: HashMap<String, String> = object
        .get("starter_aliases")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("invalid starter aliases: {e}"))?
        .unwrap_or_default();
    let mut starters = HashMap::new();
    for field in ["starters", "blocks"] {
        let Some(entries) = object.get_mut(field) else {
            continue;
        };
        let entries = entries.as_object_mut().ok_or("invalid catalog entries")?;
        let mut migrated = serde_json::Map::new();
        for (old, mut entry) in std::mem::take(entries) {
            let item = if field == "blocks" {
                &mut entry["block"]
            } else {
                &mut entry
            };
            if item["id"].as_str() != Some(&old) {
                return Err(format!("catalog key {old} does not match its stored id"));
            }
            let id = if field == "blocks" {
                let (kind, _) = blocks::parse_id(&old)?;
                if item["kind"] != json!(kind) {
                    return Err(format!("block {old} has a conflicting kind"));
                }
                blocks::canonical_id(&old)?
            } else {
                canonical_starter_id(&old)?
            };
            if migrated.contains_key(&id) {
                return Err(format!(
                    "global namespace collision: multiple {field} map to {id}; rename one before migrating"
                ));
            }
            if item["name"].as_str() == Some(&old) {
                item["name"] = json!(id);
            }
            item["id"] = json!(id);
            if field == "starters" {
                starters.insert(old.clone(), id.clone());
                if old != id
                    && aliases
                        .insert(old.clone(), id.clone())
                        .is_some_and(|prior| prior != id)
                {
                    return Err(format!("conflicting starter alias {old}"));
                }
            }
            migrated.insert(id, entry);
        }
        *entries = migrated;
    }
    for target in aliases.values_mut() {
        if let Some(id) = starters.get(target) {
            *target = id.clone();
        }
    }
    if let Some(entries) = object.get("starters").and_then(Value::as_object) {
        for (alias, target) in &aliases {
            if !entries.contains_key(target) || entries.contains_key(alias) {
                return Err(format!(
                    "invalid or conflicting starter alias {alias} -> {target}"
                ));
            }
        }
    }
    for field in [
        "versions",
        "discussions",
        "bundles",
        "bundle_commits",
        "briefcase_entries",
    ] {
        let Some(entries) = object.get_mut(field) else {
            continue;
        };
        let entries = entries
            .as_object_mut()
            .ok_or("invalid starter related entries")?;
        let mut migrated = serde_json::Map::new();
        for (old, mut entry) in std::mem::take(entries) {
            let id = if field == "briefcase_entries" {
                old.split_once(':')
                    .map(|(starter, commit)| {
                        format!(
                            "{}:{commit}",
                            starters.get(starter).map_or(starter, String::as_str)
                        )
                    })
                    .unwrap_or_else(|| old.clone())
            } else {
                starters.get(&old).cloned().unwrap_or_else(|| old.clone())
            };
            if field == "discussions"
                && let Some(discussions) = entry.as_array_mut()
            {
                for discussion in discussions {
                    discussion["starter_id"] = json!(id);
                }
            }
            if migrated.insert(id.clone(), entry).is_some() {
                return Err(format!("global namespace collision in {field}: {id}"));
            }
        }
        *entries = migrated;
    }
    object.insert("starter_aliases".into(), json!(aliases));
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_related_state_and_legacy_aliases() {
        let snapshot = json!({
            "starters": {"alice.udd": {"id":"alice.udd", "name":"alice.udd", "downloads":9}},
            "blocks": {"gene:docker": {"block":{"id":"gene:docker", "name":"Docker", "kind":"gene"}, "versions":[{"version":"hash"}]}},
            "versions":{"alice.udd":[{"version":"1.0"}]},
            "discussions":{"alice.udd":[{"starter_id":"alice.udd", "body":"hello"}]},
            "bundles":{"alice.udd":"archive"},
            "bundle_commits":{"alice.udd":"commit"},
            "briefcase_entries":{"alice.udd:commit":"receipt"}
        });
        let migrated = migrate(snapshot).unwrap();
        assert_eq!(migrated["starters"]["s.udd"]["downloads"], 9);
        assert_eq!(migrated["starters"]["s.udd"]["name"], "s.udd");
        assert_eq!(migrated["starter_aliases"]["alice.udd"], "s.udd");
        assert_eq!(migrated["blocks"]["g.docker"]["block"]["id"], "g.docker");
        assert_eq!(
            migrated["blocks"]["g.docker"]["versions"][0]["version"],
            "hash"
        );
        assert_eq!(migrated["versions"]["s.udd"][0]["version"], "1.0");
        assert_eq!(migrated["discussions"]["s.udd"][0]["starter_id"], "s.udd");
        assert_eq!(migrated["bundles"]["s.udd"], "archive");
        assert_eq!(migrated["bundle_commits"]["s.udd"], "commit");
        assert_eq!(migrated["briefcase_entries"]["s.udd:commit"], "receipt");
        assert_eq!(migrate(migrated.clone()).unwrap(), migrated);
    }

    #[test]
    fn collisions_fail_instead_of_dropping_either_owner() {
        let snapshot = json!({"starters":{
            "alice.udd":{"id":"alice.udd"},
            "bob.udd":{"id":"bob.udd"}
        }});
        assert!(migrate(snapshot).unwrap_err().contains("collision"));
        let snapshot = json!({"blocks":{
            "gene:docker":{"block":{"id":"gene:docker", "kind":"gene"}},
            "g.docker":{"block":{"id":"g.docker", "kind":"gene"}}
        }});
        assert!(migrate(snapshot).unwrap_err().contains("collision"));
    }

    #[tokio::test]
    async fn legacy_starter_routes_keep_content_history_and_privacy() {
        use crate::*;
        let state = AppState::default();
        let now = Utc::now();
        let old = "alice.udd";
        let starter = Starter {
            id: old.into(),
            name: old.into(),
            description: "fixture".into(),
            owner: "c:alice".into(),
            owner_uuid: Uuid::new_v4().to_string(),
            visibility: Visibility::Public,
            version: "1.0".into(),
            downloads: 7,
            stars: 3,
            updated_at: now,
            tags: vec![],
            yaml: silicon_starter_core::SEED_YAML.into(),
        };
        let commit = "a".repeat(40);
        restore_state(&state, json!({
            "starters":{old:starter},
            "versions":{old:[{"version":"1.0","commit":commit,"notes":"release","published_at":now}]},
            "bundles":{old:BASE64.encode(b"original bundle")},
            "bundle_commits":{old:commit},
            "discussions":{old:[{"id":"discussion","starter_id":old,"parent_id":null,"author":"c:alice","body":"hello","created_at":now}]}
        })).await.unwrap();
        for id in [old, "s.udd"] {
            let Json(starter) = show(State(state.clone()), HeaderMap::new(), Path(id.into()))
                .await
                .unwrap();
            assert_eq!(starter.id, "s.udd");
            assert_eq!(starter.stars, 3);
            let Json(history) = versions(State(state.clone()), HeaderMap::new(), Path(id.into()))
                .await
                .unwrap();
            assert_eq!(history[0].notes, "release");
            let Json(threads) =
                discussions(State(state.clone()), HeaderMap::new(), Path(id.into())).await;
            assert_eq!(threads[0].starter_id, "s.udd");
            let Json(download) = archive(
                State(state.clone()),
                HeaderMap::new(),
                Path(id.into()),
                Query(HashMap::new()),
            )
            .await
            .unwrap();
            assert_eq!(download["id"], "s.udd");
            assert_eq!(download["bundle_base64"], BASE64.encode(b"original bundle"));
        }
        assert_eq!(state.starters.read().await["s.udd"].downloads, 9);
        state
            .starters
            .write()
            .await
            .get_mut("s.udd")
            .unwrap()
            .visibility = Visibility::Private;
        for id in [old, "s.udd"] {
            assert_eq!(
                show(State(state.clone()), HeaderMap::new(), Path(id.into()))
                    .await
                    .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                archive(
                    State(state.clone()),
                    HeaderMap::new(),
                    Path(id.into()),
                    Query(HashMap::new())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            let Json(threads) =
                discussions(State(state.clone()), HeaderMap::new(), Path(id.into())).await;
            assert!(threads.is_empty());
        }
    }

    #[tokio::test]
    async fn legacy_block_routes_share_payload_downloads_and_privacy() {
        use crate::*;
        let state = AppState::default();
        let now = Utc::now();
        let text = "# Original gene";
        let version = silicon_starter_core::blocks::content_hash(text.as_bytes());
        restore_state(&state, json!({"blocks":{"gene:docker":{
            "block":{"id":"gene:docker","kind":"gene","name":"Docker","description":"fixture","owner":"c:alice","owner_uuid":Uuid::new_v4(),"visibility":"public","version":version,"downloads":4,"updated_at":now},
            "versions":[{"version":version,"published_at":now,"content_base64":BASE64.encode(text),"search_text":text,"embedding":null,"briefcase_entry":null}]
        }}})).await.unwrap();
        for id in ["gene:docker", "g.docker"] {
            let Json(block) = blocks::show(State(state.clone()), HeaderMap::new(), Path(id.into()))
                .await
                .unwrap();
            assert_eq!(block.id, "g.docker");
            let Json(download) = blocks::download(
                State(state.clone()),
                HeaderMap::new(),
                Path(format!("{id}@{version}")),
                Query(HashMap::new()),
            )
            .await
            .unwrap();
            assert_eq!(download["id"], "g.docker");
            assert_eq!(download["text"], text);
        }
        assert_eq!(state.blocks.read().await["g.docker"].block.downloads, 6);
        state
            .blocks
            .write()
            .await
            .get_mut("g.docker")
            .unwrap()
            .block
            .visibility = Visibility::Private;
        for id in ["gene:docker", "g.docker"] {
            assert_eq!(
                blocks::show(State(state.clone()), HeaderMap::new(), Path(id.into()))
                    .await
                    .unwrap_err(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                blocks::download(
                    State(state.clone()),
                    HeaderMap::new(),
                    Path(id.into()),
                    Query(HashMap::new())
                )
                .await
                .unwrap_err(),
                StatusCode::NOT_FOUND
            );
        }
    }
}
