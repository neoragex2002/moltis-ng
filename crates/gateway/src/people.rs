use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct PeopleMd {
    path: PathBuf,
    prefix: String,
    body: String,
    has_frontmatter: bool,
    frontmatter: serde_yaml::Value,
}

fn split_yaml_frontmatter(content: &str) -> Option<(&str, &str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---\n") {
        return None;
    }
    let prefix_len = content.len() - trimmed.len();
    let inner_start = prefix_len + "---\n".len();
    let rest = &content[inner_start..];
    let end_marker_newline = rest.find("\n---")?;
    let end_marker_line_start = inner_start + end_marker_newline + 1;
    if !content[end_marker_line_start..].starts_with("---") {
        return None;
    }
    let after_end_marker = match content[end_marker_line_start..].find('\n') {
        Some(nl) => end_marker_line_start + nl + 1,
        None => content.len(),
    };

    Some((
        &content[..prefix_len],
        &content[inner_start..(end_marker_line_start - 1)],
        &content[after_end_marker..],
    ))
}

fn atomic_write_if_changed(path: &Path, content: &str) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing == content {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("tmp.{nanos}"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn normalize_yaml_doc(mut s: String) -> String {
    if let Some(rest) = s.strip_prefix("---\n") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_suffix("\n...\n") {
        s = rest.to_string();
    }
    s.trim_matches('\n').to_string()
}

fn load_people_md() -> anyhow::Result<PeopleMd> {
    moltis_config::ensure_people_md_seeded()?;
    let path = moltis_config::people_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();

    let (prefix, inner, body, has_frontmatter) = match split_yaml_frontmatter(&raw) {
        Some((p, i, b)) => (p.to_string(), i.to_string(), b.to_string(), true),
        None => ("".to_string(), "".to_string(), raw, false),
    };

    let mut frontmatter: serde_yaml::Value = if inner.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(&inner)?
    };

    let serde_yaml::Value::Mapping(map) = &mut frontmatter else {
        // Normalize to a mapping so later code is predictable.
        frontmatter = serde_yaml::Value::Mapping(Default::default());
        let serde_yaml::Value::Mapping(map) = &mut frontmatter else {
            unreachable!();
        };
        map.insert(
            serde_yaml::Value::String("schema_version".to_string()),
            serde_yaml::Value::Number(1.into()),
        );
        map.insert(
            serde_yaml::Value::String("people".to_string()),
            serde_yaml::Value::Sequence(Vec::new()),
        );
        return Ok(PeopleMd {
            path,
            prefix,
            body,
            has_frontmatter,
            frontmatter,
        });
    };

    map.entry(serde_yaml::Value::String("schema_version".to_string()))
        .or_insert_with(|| serde_yaml::Value::Number(1.into()));
    map.entry(serde_yaml::Value::String("people".to_string()))
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));

    Ok(PeopleMd {
        path,
        prefix,
        body,
        has_frontmatter,
        frontmatter,
    })
}

fn save_people_md(doc: &PeopleMd) -> anyhow::Result<()> {
    let inner = normalize_yaml_doc(serde_yaml::to_string(&doc.frontmatter)?);
    let content = if doc.has_frontmatter {
        format!("{}---\n{inner}\n---\n{}", doc.prefix, doc.body)
    } else {
        format!("---\n{inner}\n---\n{}", doc.body)
    };
    atomic_write_if_changed(&doc.path, &content)?;
    Ok(())
}

fn yaml_scalar_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(crate) fn people_get() -> anyhow::Result<serde_json::Value> {
    let doc = load_people_md()?;
    let Some(map) = doc.frontmatter.as_mapping() else {
        return Ok(serde_json::json!({"schemaVersion": 1, "people": [], "body": doc.body}));
    };

    let schema_version = map
        .get(&serde_yaml::Value::String("schema_version".to_string()))
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let people = map
        .get(&serde_yaml::Value::String("people".to_string()))
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();

    let mut out_people: Vec<serde_json::Value> = Vec::new();
    for item in people {
        let Some(entry) = item.as_mapping() else {
            continue;
        };
        let name = entry
            .get(&serde_yaml::Value::String("name".to_string()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }

        let display_name = entry
            .get(&serde_yaml::Value::String("display_name".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let emoji = entry
            .get(&serde_yaml::Value::String("emoji".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let creature = entry
            .get(&serde_yaml::Value::String("creature".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let telegram_user_id = entry
            .get(&serde_yaml::Value::String("telegram_user_id".to_string()))
            .and_then(yaml_scalar_to_string);
        let telegram_user_name = entry
            .get(&serde_yaml::Value::String("telegram_user_name".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let telegram_display_name = entry
            .get(&serde_yaml::Value::String(
                "telegram_display_name".to_string(),
            ))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        out_people.push(serde_json::json!({
            "name": name,
            "displayName": display_name,
            "emoji": emoji,
            "creature": creature,
            "telegramUserId": telegram_user_id,
            "telegramUserName": telegram_user_name,
            "telegramDisplayName": telegram_display_name,
        }));
    }

    Ok(serde_json::json!({
        "schemaVersion": schema_version,
        "people": out_people,
        "body": doc.body,
    }))
}

pub(crate) fn people_update_entry(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        anyhow::bail!("missing 'name'");
    }
    if !moltis_config::is_valid_person_name(&name) {
        anyhow::bail!("invalid name");
    }

    let patch = params
        .get("patch")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    if !patch.is_object() {
        anyhow::bail!("patch must be an object");
    }

    let body_param = params.get("body");
    if let Some(v) = body_param {
        if !(v.is_string() || v.is_null()) {
            anyhow::bail!("body must be a string or null");
        }
    }

    let mut doc = load_people_md()?;
    let Some(map) = doc.frontmatter.as_mapping_mut() else {
        anyhow::bail!("PEOPLE.md frontmatter is not a mapping");
    };
    let Some(seq) = map
        .get_mut(&serde_yaml::Value::String("people".to_string()))
        .and_then(|v| v.as_sequence_mut())
    else {
        anyhow::bail!("PEOPLE.md frontmatter 'people' is not a list");
    };

    let mut found = false;
    for item in seq.iter_mut() {
        let Some(entry) = item.as_mapping_mut() else {
            continue;
        };
        let Some(entry_name) = entry
            .get(&serde_yaml::Value::String("name".to_string()))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if entry_name.trim() != name {
            continue;
        }
        found = true;

        // Only allow editing public contact/display fields. emoji/creature are read-only.
        apply_string_patch(entry, &patch, "displayName", "display_name");
        apply_string_patch(entry, &patch, "telegramUserId", "telegram_user_id");
        apply_string_patch(entry, &patch, "telegramUserName", "telegram_user_name");
        apply_string_patch(
            entry,
            &patch,
            "telegramDisplayName",
            "telegram_display_name",
        );
        break;
    }

    if !found {
        anyhow::bail!("person not found");
    }

    if body_param.is_some() {
        doc.body = body_param
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    save_people_md(&doc)?;

    // Return the latest parsed doc (including body), so UI can re-render.
    let out = people_get()?;
    Ok(out)
}

fn apply_string_patch(
    entry: &mut serde_yaml::Mapping,
    patch: &serde_json::Value,
    json_key: &str,
    yaml_key: &str,
) {
    let Some(value) = patch.get(json_key) else {
        return;
    };
    let yaml_key = serde_yaml::Value::String(yaml_key.to_string());
    let next = value.as_str().map(str::trim).unwrap_or("");
    if next.is_empty() {
        entry.remove(&yaml_key);
        return;
    }
    entry.insert(yaml_key, serde_yaml::Value::String(next.to_string()));
}

pub(crate) fn people_sync_from_identities() -> anyhow::Result<()> {
    moltis_config::sync_people_md_from_identities()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_update_entry_preserves_body() {
        let _guard = crate::test_support::TestDirsGuard::new();

        // Ensure default is seeded so sync/get paths exist.
        moltis_config::ensure_default_person_seeded().unwrap();
        moltis_config::ensure_people_md_seeded().unwrap();
        moltis_config::sync_people_md_from_identities().unwrap();

        // Add a custom body line to verify it survives.
        let path = moltis_config::people_path();
        let raw = std::fs::read_to_string(&path).unwrap();
        let Some((prefix, inner, body)) = split_yaml_frontmatter(&raw) else {
            panic!("expected frontmatter");
        };
        let new_raw = format!("{prefix}---\n{inner}\n---\n{body}\nKEEP THIS BODY LINE\n");
        std::fs::write(&path, new_raw).unwrap();

        let updated = people_update_entry(&serde_json::json!({
            "name": "default",
            "patch": { "displayName": "默认2" }
        }))
        .unwrap();
        assert!(
            updated["people"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "default" && p["displayName"] == "默认2")
        );

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("KEEP THIS BODY LINE"));
    }

    #[test]
    fn people_update_entry_updates_body() {
        let _guard = crate::test_support::TestDirsGuard::new();

        moltis_config::ensure_default_person_seeded().unwrap();
        moltis_config::ensure_people_md_seeded().unwrap();
        moltis_config::sync_people_md_from_identities().unwrap();

        let updated = people_update_entry(&serde_json::json!({
            "name": "default",
            "patch": {},
            "body": "\n# PEOPLE.md\n\nNew body.\n"
        }))
        .unwrap();
        assert_eq!(
            updated["body"].as_str().unwrap(),
            "\n# PEOPLE.md\n\nNew body.\n"
        );

        let after = std::fs::read_to_string(moltis_config::people_path()).unwrap();
        assert!(after.contains("\n# PEOPLE.md\n\nNew body.\n"));
    }
}
