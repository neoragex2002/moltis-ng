use std::path::PathBuf;

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

fn user_path() -> PathBuf {
    moltis_config::data_dir().join("USER.md")
}

fn user_body(raw: &str) -> String {
    match split_yaml_frontmatter(raw) {
        Some((_prefix, _inner, body)) => body.to_string(),
        None => raw.to_string(),
    }
}

pub(crate) fn user_get() -> anyhow::Result<serde_json::Value> {
    let path = user_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let body = user_body(&raw);

    let profile = moltis_config::load_user().unwrap_or_default();
    let timezone = profile.timezone.as_ref().map(|tz| tz.name().to_string());
    let location = profile.location.as_ref().map(|loc| {
        serde_json::json!({
            "latitude": loc.latitude,
            "longitude": loc.longitude,
            "place": loc.place,
            "updatedAt": loc.updated_at,
        })
    });

    Ok(serde_json::json!({
        "name": profile.name,
        "timezone": timezone,
        "location": location,
        "body": body,
    }))
}

pub(crate) fn user_update(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let patch = params
        .get("patch")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !patch.is_object() {
        anyhow::bail!("patch must be an object");
    }

    let body_param = params.get("body");
    if let Some(v) = body_param {
        if !(v.is_string() || v.is_null()) {
            anyhow::bail!("body must be a string or null");
        }
    }

    let mut profile = moltis_config::load_user().unwrap_or_default();

    if let Some(v) = patch.get("name") {
        let name = v.as_str().unwrap_or("").trim().to_string();
        profile.name = if name.is_empty() { None } else { Some(name) };
    }

    if let Some(v) = patch.get("timezone") {
        let tz_raw = v.as_str().unwrap_or("").trim();
        if tz_raw.is_empty() {
            profile.timezone = None;
        } else {
            let tz = tz_raw
                .parse::<moltis_config::Timezone>()
                .map_err(|e| anyhow::anyhow!(e))?;
            profile.timezone = Some(tz);
        }
    }

    moltis_config::save_user(&profile)?;

    if body_param.is_some() {
        let next_body = body_param.and_then(|v| v.as_str()).unwrap_or("");
        let path = user_path();
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = match split_yaml_frontmatter(&existing) {
            Some((prefix, inner, _body)) => format!("{prefix}---\n{inner}\n---\n{next_body}"),
            None => next_body.to_string(),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, updated)?;
    }

    user_get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_update_preserves_body() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let raw = "---\nname: Alice\ncustom: keep\n---\n\n# USER.md\n\nBody stays.\n";
        std::fs::write(user_path(), raw).unwrap();

        let _ = user_update(&serde_json::json!({
            "patch": {"timezone": "Asia/Shanghai"}
        }))
        .unwrap();

        let after = std::fs::read_to_string(user_path()).unwrap();
        assert!(after.contains("\n# USER.md\n\nBody stays.\n"));
        assert!(after.contains("custom: keep"));
    }

    #[test]
    fn user_update_updates_body() {
        let _guard = crate::test_support::TestDirsGuard::new();

        let raw = "---\nname: Alice\ncustom: keep\n---\n\n# USER.md\n\nOld body.\n";
        std::fs::write(user_path(), raw).unwrap();

        let _ = user_update(&serde_json::json!({
            "patch": {},
            "body": "\n# USER.md\n\nNew body.\n"
        }))
        .unwrap();

        let after = std::fs::read_to_string(user_path()).unwrap();
        assert!(after.contains("\n# USER.md\n\nNew body.\n"));
        assert!(after.contains("custom: keep"));
        assert!(after.contains("name: Alice"));
    }
}
