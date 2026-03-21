use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct FrontmatterDoc {
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

fn person_dir(name: &str) -> anyhow::Result<PathBuf> {
    if name.is_empty() {
        anyhow::bail!("missing 'name'");
    }
    if !moltis_config::is_valid_agent_id(name) {
        anyhow::bail!("invalid name");
    }
    Ok(moltis_config::agents_dir().join(name))
}

fn person_paths(name: &str) -> anyhow::Result<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let dir = person_dir(name)?;
    Ok((
        dir.join("IDENTITY.md"),
        dir.join("SOUL.md"),
        dir.join("TOOLS.md"),
        dir.join("AGENTS.md"),
    ))
}

fn load_frontmatter_doc(path: &Path) -> anyhow::Result<FrontmatterDoc> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let (prefix, inner, body, has_frontmatter) = match split_yaml_frontmatter(&raw) {
        Some((p, i, b)) => (p.to_string(), i.to_string(), b.to_string(), true),
        None => ("".to_string(), "".to_string(), raw, false),
    };

    let mut frontmatter: serde_yaml::Value = if inner.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(&inner)?
    };

    if !matches!(frontmatter, serde_yaml::Value::Mapping(_)) {
        frontmatter = serde_yaml::Value::Mapping(Default::default());
    }

    Ok(FrontmatterDoc {
        prefix,
        body,
        has_frontmatter,
        frontmatter,
    })
}

fn save_frontmatter_doc(path: &Path, doc: &FrontmatterDoc) -> anyhow::Result<()> {
    let inner = normalize_yaml_doc(serde_yaml::to_string(&doc.frontmatter)?);
    let content = if doc.has_frontmatter {
        format!("{}---\n{inner}\n---\n{}", doc.prefix, doc.body)
    } else {
        format!("---\n{inner}\n---\n{}", doc.body)
    };
    atomic_write_if_changed(path, &content)?;
    Ok(())
}

fn ensure_person_seeded(name: &str) -> anyhow::Result<()> {
    if name == "default" {
        // Keep existing default seeding behavior centralized.
        moltis_config::ensure_default_agent_seeded()?;
        return Ok(());
    }

    let (identity_path, soul_path, tools_path, agents_path) = person_paths(name)?;

    if let Some(parent) = identity_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if !identity_path.exists() {
        let content = format!(
            "---\nname: {name}\nemoji: 🤖\ncreature: Assistant\nvibe: Direct, clear, efficient\n---\n\n# IDENTITY.md\n\nWrite your longer self-definition here.\n"
        );
        std::fs::write(&identity_path, content)?;
    }

    for path in [soul_path, tools_path, agents_path] {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            std::fs::write(&path, "")?;
        }
    }

    Ok(())
}

fn yaml_scalar_value(v: &serde_json::Value) -> anyhow::Result<Option<serde_yaml::Value>> {
    if v.is_null() {
        return Ok(None);
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        return Ok(Some(serde_yaml::Value::String(s.to_string())));
    }
    anyhow::bail!("expected string or null");
}

pub(crate) fn person_list() -> anyhow::Result<serde_json::Value> {
    moltis_config::ensure_default_agent_seeded()?;

    let mut names: Vec<String> = Vec::new();
    let root = moltis_config::agents_dir();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if moltis_config::is_valid_agent_id(&name) {
                names.push(name);
            }
        }
    }
    names.sort();

    let agents = names
        .into_iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "isDefault": name == "default",
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::json!({ "agents": agents }))
}

pub(crate) fn person_get(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        anyhow::bail!("missing 'name'");
    }
    if !moltis_config::is_valid_agent_id(&name) {
        anyhow::bail!("invalid name");
    }

    ensure_person_seeded(&name)?;
    let (identity_path, soul_path, tools_path, agents_path) = person_paths(&name)?;
    if !identity_path.exists() {
        anyhow::bail!("person not found");
    }

    let identity_doc = load_frontmatter_doc(&identity_path)?;
    let Some(map) = identity_doc.frontmatter.as_mapping() else {
        anyhow::bail!("IDENTITY.md frontmatter is not a mapping");
    };

    let get_str = |k: &str| {
        map.get(&serde_yaml::Value::String(k.to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    let soul = std::fs::read_to_string(&soul_path).unwrap_or_default();
    let tools = std::fs::read_to_string(&tools_path).unwrap_or_default();
    let agents = std::fs::read_to_string(&agents_path).unwrap_or_default();

    Ok(serde_json::json!({
        "name": name,
        "isDefault": name == "default",
        "identity": {
            "name": get_str("name").unwrap_or_else(|| name.clone()),
            "emoji": get_str("emoji"),
            "creature": get_str("creature"),
            "vibe": get_str("vibe"),
            "body": identity_doc.body,
        },
        "soul": soul,
        "tools": tools,
        "agents": agents,
    }))
}

pub(crate) fn person_save(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        anyhow::bail!("missing 'name'");
    }
    if !moltis_config::is_valid_agent_id(&name) {
        anyhow::bail!("invalid name");
    }

    let (identity_path, soul_path, tools_path, agents_path) = person_paths(&name)?;
    let identity_existed_before = identity_path.exists();

    ensure_person_seeded(&name)?;

    let identity_patch_present = params.get("identityPatch").is_some();
    let identity_body_present = params.get("identityBody").is_some();
    let mut identity_written = false;

    // IDENTITY.md
    if identity_patch_present || identity_body_present {
        let mut doc = load_frontmatter_doc(&identity_path)?;
        if !doc.has_frontmatter {
            doc.has_frontmatter = true;
        }

        let serde_yaml::Value::Mapping(map) = &mut doc.frontmatter else {
            anyhow::bail!("IDENTITY.md frontmatter is not a mapping");
        };

        // Force name to match directory.
        map.insert(
            serde_yaml::Value::String("name".to_string()),
            serde_yaml::Value::String(name.clone()),
        );

        if let Some(patch) = params.get("identityPatch") {
            if !patch.is_object() {
                anyhow::bail!("identityPatch must be an object");
            }
            for (json_key, yaml_key) in [
                ("emoji", "emoji"),
                ("creature", "creature"),
                ("vibe", "vibe"),
            ] {
                if let Some(v) = patch.get(json_key) {
                    let key = serde_yaml::Value::String(yaml_key.to_string());
                    match yaml_scalar_value(v)? {
                        Some(next) => {
                            map.insert(key, next);
                        },
                        None => {
                            map.remove(&key);
                        },
                    }
                }
            }
        }

        if params.get("identityBody").is_some() {
            let next_body = params
                .get("identityBody")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            doc.body = next_body.to_string();
        }

        save_frontmatter_doc(&identity_path, &doc)?;
        identity_written = true;
    }

    // SOUL/TOOLS/AGENTS markdown
    for (key, path) in [
        ("soul", soul_path),
        ("tools", tools_path),
        ("agents", agents_path),
    ] {
        if params.get(key).is_some() {
            let next = params.get(key).and_then(|v| v.as_str()).unwrap_or("");
            atomic_write_if_changed(&path, next)?;
        }
    }

    // Keep Contacts read-only fields in sync when identity changes (or when
    // creating a new person directory).
    if identity_written || !identity_existed_before {
        moltis_config::sync_people_md_from_identities()?;
    }

    person_get(&serde_json::json!({ "name": name }))
}

pub(crate) fn person_delete(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        anyhow::bail!("missing 'name'");
    }
    if !moltis_config::is_valid_agent_id(&name) {
        anyhow::bail!("invalid name");
    }
    if name == "default" {
        anyhow::bail!("cannot delete default");
    }

    let dir = person_dir(&name)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }

    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_list_includes_default() {
        let _guard = crate::test_support::TestDirsGuard::new();
        let out = person_list().unwrap();
        let agents = out["agents"].as_array().unwrap();
        assert!(agents.iter().any(|p| p["name"] == "default"));
    }

    #[test]
    fn person_save_seeds_and_gets() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let out = person_save(&serde_json::json!({ "name": "ops" })).unwrap();
        assert_eq!(out["name"], "ops");
        assert_eq!(out["identity"]["name"], "ops");

        let dir = moltis_config::agents_dir().join("ops");
        assert!(dir.join("IDENTITY.md").exists());
        assert!(dir.join("SOUL.md").exists());
        assert!(dir.join("TOOLS.md").exists());
        assert!(dir.join("AGENTS.md").exists());
    }

    #[test]
    fn person_save_forces_identity_name_and_preserves_body_by_default() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let dir = moltis_config::agents_dir().join("research");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("IDENTITY.md"),
            "---\nname: WRONG\nemoji: 🧪\n---\n\n# IDENTITY.md\n\nKeep body.\n",
        )
        .unwrap();
        for f in ["SOUL.md", "TOOLS.md", "AGENTS.md"] {
            std::fs::write(dir.join(f), "").unwrap();
        }

        let out = person_save(&serde_json::json!({
            "name": "research",
            "identityPatch": { "vibe": "Calm" }
        }))
        .unwrap();

        assert_eq!(out["identity"]["name"], "research");
        assert_eq!(out["identity"]["vibe"], "Calm");
        assert!(
            out["identity"]["body"]
                .as_str()
                .unwrap()
                .contains("Keep body.")
        );

        let after = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        assert!(after.contains("name: research"));
        assert!(after.contains("Keep body."));
    }

    #[test]
    fn person_delete_rejects_default() {
        let _guard = crate::test_support::TestDirsGuard::new();
        let err = person_delete(&serde_json::json!({ "name": "default" })).unwrap_err();
        assert!(format!("{err}").contains("cannot delete default"));
    }

    #[test]
    fn saving_soul_does_not_modify_identity_or_people_md() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let dir = moltis_config::agents_dir().join("ops");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("IDENTITY.md"),
            "# IDENTITY.md\n\nPlain body without frontmatter.\n",
        )
        .unwrap();
        std::fs::write(dir.join("SOUL.md"), "old soul").unwrap();
        std::fs::write(dir.join("TOOLS.md"), "old tools").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "old agents").unwrap();

        let people_path = moltis_config::people_path();
        if let Some(parent) = people_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &people_path,
            "---\nschema_version: 1\npeople:\n  - name: ops\n    display_name: Ops\n---\n\n# PEOPLE.md\n\nKeep body.\n",
        )
        .unwrap();

        let identity_before = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        let people_before = std::fs::read_to_string(&people_path).unwrap();

        let out = person_save(&serde_json::json!({ "name": "ops", "soul": "new soul" })).unwrap();
        assert_eq!(out["soul"], "new soul");
        assert_eq!(out["tools"], "old tools");
        assert_eq!(out["agents"], "old agents");

        let identity_after = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        assert_eq!(
            identity_after, identity_before,
            "IDENTITY.md must not change"
        );

        let people_after = std::fs::read_to_string(&people_path).unwrap();
        assert_eq!(people_after, people_before, "PEOPLE.md must not change");
    }

    #[test]
    fn saving_tools_does_not_modify_identity_or_people_md() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let dir = moltis_config::agents_dir().join("ops");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("IDENTITY.md"),
            "# IDENTITY.md\n\nPlain body without frontmatter.\n",
        )
        .unwrap();
        std::fs::write(dir.join("SOUL.md"), "old soul").unwrap();
        std::fs::write(dir.join("TOOLS.md"), "old tools").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "old agents").unwrap();

        let people_path = moltis_config::people_path();
        if let Some(parent) = people_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &people_path,
            "---\nschema_version: 1\npeople:\n  - name: ops\n    display_name: Ops\n---\n\n# PEOPLE.md\n\nKeep body.\n",
        )
        .unwrap();

        let identity_before = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        let people_before = std::fs::read_to_string(&people_path).unwrap();

        let out = person_save(&serde_json::json!({ "name": "ops", "tools": "new tools" })).unwrap();
        assert_eq!(out["soul"], "old soul");
        assert_eq!(out["tools"], "new tools");
        assert_eq!(out["agents"], "old agents");

        let identity_after = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        assert_eq!(
            identity_after, identity_before,
            "IDENTITY.md must not change"
        );

        let people_after = std::fs::read_to_string(&people_path).unwrap();
        assert_eq!(people_after, people_before, "PEOPLE.md must not change");
    }

    #[test]
    fn saving_agents_does_not_modify_identity_or_people_md() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let dir = moltis_config::agents_dir().join("ops");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("IDENTITY.md"),
            "# IDENTITY.md\n\nPlain body without frontmatter.\n",
        )
        .unwrap();
        std::fs::write(dir.join("SOUL.md"), "old soul").unwrap();
        std::fs::write(dir.join("TOOLS.md"), "old tools").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "old agents").unwrap();

        let people_path = moltis_config::people_path();
        if let Some(parent) = people_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &people_path,
            "---\nschema_version: 1\npeople:\n  - name: ops\n    display_name: Ops\n---\n\n# PEOPLE.md\n\nKeep body.\n",
        )
        .unwrap();

        let identity_before = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        let people_before = std::fs::read_to_string(&people_path).unwrap();

        let out =
            person_save(&serde_json::json!({ "name": "ops", "agents": "new agents" })).unwrap();
        assert_eq!(out["soul"], "old soul");
        assert_eq!(out["tools"], "old tools");
        assert_eq!(out["agents"], "new agents");

        let identity_after = std::fs::read_to_string(dir.join("IDENTITY.md")).unwrap();
        assert_eq!(
            identity_after, identity_before,
            "IDENTITY.md must not change"
        );

        let people_after = std::fs::read_to_string(&people_path).unwrap();
        assert_eq!(people_after, people_before, "PEOPLE.md must not change");
    }
}
