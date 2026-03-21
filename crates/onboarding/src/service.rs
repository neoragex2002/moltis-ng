//! Live onboarding service that backs the `wizard.*` RPC methods.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{Value, json};

use moltis_config::MoltisConfig;

use crate::state::{WizardState, WizardStep};

const DEFAULT_PERSON_NAME: &str = "default";

/// Live onboarding service backed by a `WizardState` and config persistence.
pub struct LiveOnboardingService {
    state: Mutex<Option<WizardState>>,
    config_path: PathBuf,
}

impl LiveOnboardingService {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            state: Mutex::new(None),
            config_path,
        }
    }

    /// Save config to the service's config path.
    fn save(&self, config: &MoltisConfig) -> anyhow::Result<()> {
        moltis_config::loader::save_config_to_path(&self.config_path, config)?;
        Ok(())
    }

    /// Check whether onboarding has been completed.
    ///
    /// Returns `true` when the `.onboarded` sentinel file exists in the data
    /// directory (written after the wizard finishes) **or** the
    /// `SKIP_ONBOARDING` environment variable is set to a non-empty value.
    /// Pre-existing identity/user data alone no longer auto-skips.
    fn is_already_onboarded(&self) -> bool {
        if std::env::var("SKIP_ONBOARDING")
            .ok()
            .is_some_and(|v| !v.is_empty())
        {
            return true;
        }
        onboarded_sentinel().exists()
    }

    /// Mark onboarding as complete by writing the sentinel file.
    fn mark_onboarded(&self) {
        let path = onboarded_sentinel();
        let _ = std::fs::write(&path, "");
    }

    /// Start the wizard. Returns current step info.
    ///
    /// If `force` is true, the wizard starts even when already onboarded,
    /// allowing the user to reconfigure their identity.
    pub fn wizard_start(&self, force: bool) -> Value {
        if !force && self.is_already_onboarded() {
            return json!({
                "onboarded": true,
                "step": "done",
                "prompt": "Already onboarded!",
            });
        }

        // Ensure workspace files exist before pre-populating.
        let _ = moltis_config::ensure_default_agent_seeded();
        let _ = moltis_config::ensure_people_md_seeded();

        let mut ws = WizardState::new();

        // Pre-populate from existing workspace files so the user can keep values.
        if let Some(file_identity) = moltis_config::load_identity() {
            ws.identity = file_identity;
        }
        if let Some(file_user) = moltis_config::load_user() {
            ws.user = file_user;
        }
        ws.agent_display_name = load_people_display_name(DEFAULT_PERSON_NAME);

        let resp = step_response(&ws);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(ws);
        resp
    }

    /// Advance the wizard with user input.
    pub fn wizard_next(&self, input: &str) -> Result<Value, String> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let ws = guard.as_mut().ok_or("no active wizard session")?;
        ws.advance(input);

        if ws.is_done() {
            // Ensure the config file exists (onboarding writes it on completion).
            let config = if self.config_path.exists() {
                moltis_config::loader::load_config(&self.config_path).unwrap_or_default()
            } else {
                MoltisConfig::default()
            };
            self.save(&config)
                .map_err(|e| format!("failed to save config: {e}"))?;

            // Persist workspace identity/user.
            let mut identity = ws.identity.clone();
            identity.name = Some(DEFAULT_PERSON_NAME.to_string());
            if let Err(e) = moltis_config::save_identity(&identity) {
                return Err(format!("failed to save IDENTITY.md: {e}"));
            }
            if let Err(e) = moltis_config::save_user(&ws.user) {
                return Err(format!("failed to save USER.md: {e}"));
            }
            if let Err(e) =
                save_people_display_name(DEFAULT_PERSON_NAME, ws.agent_display_name.as_deref())
            {
                return Err(format!("failed to save PEOPLE.md display_name: {e}"));
            }
            if let Err(e) = moltis_config::sync_people_md_from_identities() {
                return Err(format!("failed to sync PEOPLE.md: {e}"));
            }
            self.mark_onboarded();

            let effective_display_name = ws
                .agent_display_name
                .clone()
                .unwrap_or_else(|| "moltis".to_string());
            let resp = json!({
                "step": "done",
                "prompt": ws.prompt(),
                "done": true,
                "identity": {
                    "name": effective_display_name,
                    "emoji": identity.emoji,
                    "creature": identity.creature,
                    "vibe": identity.vibe,
                },
                "user": {
                    "name": ws.user.name,
                    "timezone": ws.user.timezone,
                },
            });
            *guard = None;
            return Ok(resp);
        }

        Ok(step_response(ws))
    }

    /// Cancel an active wizard session.
    pub fn wizard_cancel(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Return the current wizard status.
    pub fn wizard_status(&self) -> Value {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let onboarded = self.is_already_onboarded();
        match guard.as_ref() {
            Some(ws) => json!({
                "active": true,
                "step": ws.step,
                "onboarded": onboarded,
            }),
            None => json!({
                "active": false,
                "onboarded": onboarded,
            }),
        }
    }

    /// Update identity fields by merging partial JSON into the existing config.
    ///
    /// Accepts: `{name?, emoji?, creature?, vibe?, soul?, user_name?}`
    pub fn identity_update(&self, params: Value) -> anyhow::Result<Value> {
        // Ensure workspace files exist.
        let _ = moltis_config::ensure_default_agent_seeded();
        let _ = moltis_config::ensure_people_md_seeded();

        let mut identity = moltis_config::load_identity().unwrap_or_default();
        identity.name = Some(DEFAULT_PERSON_NAME.to_string());
        let mut user = moltis_config::load_user().unwrap_or_default();

        /// Extract an optional non-empty string from JSON, mapping `""` to `None`.
        fn str_field(params: &Value, key: &str) -> Option<Option<String>> {
            params
                .get(key)
                .and_then(|v| v.as_str())
                .map(|v| (!v.is_empty()).then(|| v.to_string()))
        }

        if let Some(v) = str_field(&params, "name") {
            save_people_display_name(DEFAULT_PERSON_NAME, v.as_deref())?;
        }
        if let Some(v) = str_field(&params, "emoji") {
            identity.emoji = v;
        }
        if let Some(v) = str_field(&params, "creature") {
            identity.creature = v;
        }
        if let Some(v) = str_field(&params, "vibe") {
            identity.vibe = v;
        }
        if let Some(v) = params.get("soul") {
            let soul = if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            };
            moltis_config::save_soul(soul.as_deref())?;
        }
        if let Some(v) = str_field(&params, "user_name") {
            user.name = v;
        }
        moltis_config::save_identity(&identity)?;
        moltis_config::save_user(&user)?;
        moltis_config::sync_people_md_from_identities()?;

        // Mark onboarding complete once both names are present.
        let display_name = load_people_display_name(DEFAULT_PERSON_NAME);
        if display_name.is_some() && user.name.is_some() {
            self.mark_onboarded();
        }

        Ok(json!({
            "name": display_name,
            "emoji": identity.emoji,
            "creature": identity.creature,
            "vibe": identity.vibe,
            "soul": moltis_config::load_soul(),
            "user_name": user.name,
        }))
    }

    /// Update SOUL.md in the workspace root.
    pub fn identity_update_soul(&self, soul: Option<String>) -> anyhow::Result<Value> {
        moltis_config::save_soul(soul.as_deref())?;
        Ok(json!({}))
    }

    /// Read identity from workspace-backed sources (for `agent.identity.get`).
    pub fn identity_get(&self) -> moltis_config::ResolvedIdentity {
        let mut id = moltis_config::ResolvedIdentity::default();
        id.name = load_people_display_name(DEFAULT_PERSON_NAME).unwrap_or_else(|| id.name.clone());
        if let Some(file_identity) = moltis_config::load_identity() {
            id.emoji = file_identity.emoji;
            id.creature = file_identity.creature;
            id.vibe = file_identity.vibe;
        }
        if let Some(file_user) = moltis_config::load_user() {
            id.user_name = file_user.name;
        }
        id.soul = moltis_config::load_soul();
        id
    }
}

/// Path to the `.onboarded` sentinel file in the data directory.
fn onboarded_sentinel() -> std::path::PathBuf {
    moltis_config::data_dir().join(".onboarded")
}

fn step_response(ws: &WizardState) -> Value {
    json!({
        "step": ws.step,
        "prompt": ws.prompt(),
        "done": ws.step == WizardStep::Done,
        "onboarded": false,
        "current": current_value(ws),
    })
}

/// Returns the current (pre-populated) value for the active step, if any.
fn current_value(ws: &WizardState) -> Option<&str> {
    use WizardStep::*;
    match ws.step {
        UserName => ws.user.name.as_deref(),
        AgentName => ws.agent_display_name.as_deref(),
        AgentEmoji => ws.identity.emoji.as_deref(),
        AgentCreature => ws.identity.creature.as_deref(),
        AgentVibe => ws.identity.vibe.as_deref(),
        _ => None,
    }
}

fn extract_yaml_frontmatter(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n')?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
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

fn load_people_display_name(person_name: &str) -> Option<String> {
    moltis_config::ensure_people_md_seeded().ok()?;
    let raw = std::fs::read_to_string(moltis_config::people_path()).ok()?;
    let frontmatter = extract_yaml_frontmatter(&raw)?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let people = yaml.get("people")?.as_sequence()?;
    for item in people {
        let entry = item.as_mapping()?;
        let name = entry
            .get(&serde_yaml::Value::String("name".to_string()))?
            .as_str()?
            .trim();
        if name == person_name {
            return entry
                .get(&serde_yaml::Value::String("display_name".to_string()))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

fn save_people_display_name(person_name: &str, display_name: Option<&str>) -> anyhow::Result<()> {
    moltis_config::ensure_people_md_seeded()?;
    let path = moltis_config::people_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let (prefix, inner, body, has_frontmatter) = match split_yaml_frontmatter(&existing) {
        Some((p, i, b)) => (p, i, b, true),
        None => ("", "", existing.as_str(), false),
    };

    let mut yaml: serde_yaml::Value = if inner.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(inner)?
    };
    let serde_yaml::Value::Mapping(root) = &mut yaml else {
        anyhow::bail!("PEOPLE.md frontmatter is not a mapping");
    };
    root.entry(serde_yaml::Value::String("schema_version".to_string()))
        .or_insert_with(|| serde_yaml::Value::Number(1.into()));
    root.entry(serde_yaml::Value::String("people".to_string()))
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));
    let Some(seq) = root
        .get_mut(&serde_yaml::Value::String("people".to_string()))
        .and_then(|v| v.as_sequence_mut())
    else {
        anyhow::bail!("PEOPLE.md frontmatter 'people' is not a list");
    };

    // Find or append the entry.
    let mut found = None;
    for item in seq.iter_mut() {
        let Some(entry) = item.as_mapping_mut() else {
            continue;
        };
        let Some(name) = entry
            .get(&serde_yaml::Value::String("name".to_string()))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if name.trim() == person_name {
            found = Some(entry);
            break;
        }
    }
    if found.is_none() {
        let mut entry = serde_yaml::Mapping::new();
        entry.insert(
            serde_yaml::Value::String("name".to_string()),
            serde_yaml::Value::String(person_name.to_string()),
        );
        seq.push(serde_yaml::Value::Mapping(entry));
        let Some(serde_yaml::Value::Mapping(last)) = seq.last_mut() else {
            unreachable!();
        };
        found = Some(last);
    }

    let entry = found.ok_or_else(|| anyhow::anyhow!("PEOPLE.md entry insertion failed"))?;
    let key = serde_yaml::Value::String("display_name".to_string());
    let next = display_name.unwrap_or("").trim();
    if next.is_empty() {
        entry.remove(&key);
    } else {
        entry.insert(key, serde_yaml::Value::String(next.to_string()));
    }

    let mut new_inner = serde_yaml::to_string(&yaml)?;
    if let Some(rest) = new_inner.strip_prefix("---\n") {
        new_inner = rest.to_string();
    }
    if let Some(rest) = new_inner.strip_suffix("\n...\n") {
        new_inner = rest.to_string();
    }
    let new_inner = new_inner.trim_matches('\n');
    let new_content = if has_frontmatter {
        format!("{prefix}---\n{new_inner}\n---\n{body}")
    } else {
        format!("---\n{new_inner}\n---\n{body}")
    };

    atomic_write_if_changed(&path, &new_content)?;
    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, std::io::Write};

    struct TestDataDirState {
        _data_dir: Option<PathBuf>,
    }

    static DATA_DIR_TEST_LOCK: std::sync::Mutex<TestDataDirState> =
        std::sync::Mutex::new(TestDataDirState { _data_dir: None });

    #[test]
    fn wizard_round_trip() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let config_path = dir.path().join("moltis.toml");
        let svc = LiveOnboardingService::new(config_path.clone());

        // Start
        let resp = svc.wizard_start(false);
        assert_eq!(resp["onboarded"], false);
        assert_eq!(resp["step"], "welcome");

        // Advance through all steps
        svc.wizard_next("").unwrap(); // welcome → user_name
        svc.wizard_next("Alice").unwrap(); // → agent_name
        svc.wizard_next("Rex").unwrap(); // → emoji
        svc.wizard_next("\u{1f436}").unwrap(); // → creature
        svc.wizard_next("dog").unwrap(); // → vibe
        svc.wizard_next("chill").unwrap(); // → confirm
        let done = svc.wizard_next("").unwrap(); // → done

        assert_eq!(done["done"], true);
        assert_eq!(done["identity"]["name"], "Rex");
        assert_eq!(done["user"]["name"], "Alice");

        // Config file should exist
        assert!(config_path.exists());

        // Should report as onboarded now
        let status = svc.wizard_status();
        assert_eq!(status["onboarded"], true);

        assert!(dir.path().join("agents/default/IDENTITY.md").exists());
        assert!(dir.path().join("USER.md").exists());
        moltis_config::clear_data_dir();
    }

    #[test]
    fn config_data_alone_does_not_skip_onboarding() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let config_path = dir.path().join("moltis.toml");
        // Write a config — but no sentinel file.
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "[server]\nbind = \"127.0.0.1\"\nport = 18789").unwrap();

        let svc = LiveOnboardingService::new(config_path);
        // Should NOT be onboarded — data alone isn't enough.
        let resp = svc.wizard_start(false);
        assert_eq!(resp["onboarded"], false);
        assert_eq!(resp["step"], "welcome");
        moltis_config::clear_data_dir();
    }

    #[test]
    fn sentinel_file_marks_onboarded() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let config_path = dir.path().join("moltis.toml");
        // Write sentinel file.
        std::fs::write(dir.path().join(".onboarded"), "").unwrap();

        let svc = LiveOnboardingService::new(config_path);
        let resp = svc.wizard_start(false);
        assert_eq!(resp["onboarded"], true);
        moltis_config::clear_data_dir();
    }

    #[test]
    fn cancel_wizard() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let svc = LiveOnboardingService::new(dir.path().join("moltis.toml"));
        svc.wizard_start(false);
        assert_eq!(svc.wizard_status()["active"], true);
        svc.wizard_cancel();
        assert_eq!(svc.wizard_status()["active"], false);
        moltis_config::clear_data_dir();
    }

    #[test]
    fn identity_update_partial() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let config_path = dir.path().join("moltis.toml");
        let svc = LiveOnboardingService::new(config_path.clone());

        // Create initial identity
        let res = svc
            .identity_update(json!({
                "name": "Rex",
                "emoji": "\u{1f436}",
                "creature": "dog",
                "vibe": "chill",
                "user_name": "Alice",
            }))
            .unwrap();
        assert_eq!(res["name"], "Rex");
        assert_eq!(res["user_name"], "Alice");

        // Partial update: only change vibe
        let res = svc.identity_update(json!({ "vibe": "playful" })).unwrap();
        assert_eq!(res["name"], "Rex");
        assert_eq!(res["vibe"], "playful");
        assert_eq!(res["emoji"], "\u{1f436}");

        // Verify identity_get reflects updates
        let id = svc.identity_get();
        assert_eq!(id.name, "Rex");
        assert_eq!(id.vibe.as_deref(), Some("playful"));
        assert_eq!(id.user_name.as_deref(), Some("Alice"));

        // Update soul
        let res = svc
            .identity_update(json!({ "soul": "Be helpful." }))
            .unwrap();
        assert_eq!(res["soul"], "Be helpful.");

        // Clear soul with null
        let res = svc.identity_update(json!({ "soul": null })).unwrap();
        assert!(res["soul"].is_null());

        let soul_path = dir.path().join("agents/default/SOUL.md");
        // save_soul(None) writes an empty file (not deleted) to prevent re-seeding
        assert!(soul_path.exists());
        assert!(std::fs::read_to_string(&soul_path).unwrap().is_empty());

        // Reports as onboarded
        assert_eq!(svc.wizard_status()["onboarded"], true);

        moltis_config::clear_data_dir();
    }

    #[test]
    fn identity_update_empty_fields() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        moltis_config::set_data_dir(dir.path().to_path_buf());
        let svc = LiveOnboardingService::new(dir.path().join("moltis.toml"));

        // Set name, then clear it
        svc.identity_update(json!({ "name": "Rex" })).unwrap();
        let res = svc.identity_update(json!({ "name": "" })).unwrap();
        assert!(res["name"].is_null());
        moltis_config::clear_data_dir();
    }
}
