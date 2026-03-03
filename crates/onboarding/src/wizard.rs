//! Terminal-based onboarding wizard using the shared state machine.

use std::io::{BufRead, Write};

use moltis_config::{AgentIdentity, MoltisConfig, UserProfile, find_or_default_config_path};

use crate::state::WizardState;

const DEFAULT_PERSON_NAME: &str = "default";

/// Run the interactive onboarding wizard in the terminal.
pub async fn run_onboarding() -> anyhow::Result<()> {
    let config_path = find_or_default_config_path();
    let onboarded = moltis_config::data_dir().join(".onboarded");
    if onboarded.exists() {
        println!("Already onboarded.");
        return Ok(());
    }

    let mut state = WizardState::new();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();

    while !state.is_done() {
        println!("{}", state.prompt());
        print!("> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        state.advance(&line);
    }

    // Ensure config exists; identity/user are workspace-backed (not in moltis.toml).
    let config = if config_path.exists() {
        moltis_config::loader::load_config(&config_path).unwrap_or_default()
    } else {
        let cfg = MoltisConfig::default();
        moltis_config::loader::save_config_to_path(&config_path, &cfg)?;
        cfg
    };
    // Preserve any current config file contents by rewriting via merge helper.
    moltis_config::loader::save_config_to_path(&config_path, &config)?;

    // Persist workspace files.
    let mut identity = AgentIdentity::default();
    identity.name = Some(DEFAULT_PERSON_NAME.to_string());
    identity.emoji = state.identity.emoji;
    identity.creature = state.identity.creature;
    identity.vibe = state.identity.vibe;
    moltis_config::save_identity(&identity)?;

    let mut user = UserProfile::default();
    user.name = state.user.name;
    user.timezone = state.user.timezone;
    user.location = state.user.location;
    moltis_config::save_user(&user)?;

    // Seed + patch PEOPLE.md display name and then sync emoji/creature from IDENTITY.
    moltis_config::ensure_people_md_seeded()?;
    set_people_display_name(DEFAULT_PERSON_NAME, state.agent_display_name.as_deref())?;
    moltis_config::sync_people_md_from_identities()?;

    let _ = std::fs::write(&onboarded, "");
    println!("Config saved to {}", config_path.display());
    println!("Onboarding complete!");
    Ok(())
}

fn set_people_display_name(person_name: &str, display_name: Option<&str>) -> anyhow::Result<()> {
    let path = moltis_config::people_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let (prefix, inner, body, has_frontmatter) = match split_yaml_frontmatter(&raw) {
        Some((p, i, b)) => (p, i, b, true),
        None => ("", "", raw.as_str(), false),
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
    let entry = found.unwrap();
    let key = serde_yaml::Value::String("display_name".to_string());
    let next = display_name.unwrap_or("").trim();
    if next.is_empty() {
        entry.remove(&key);
    } else {
        entry.insert(key, serde_yaml::Value::String(next.to_string()));
    }

    let mut inner_out = serde_yaml::to_string(&yaml)?;
    if let Some(rest) = inner_out.strip_prefix("---\n") {
        inner_out = rest.to_string();
    }
    if let Some(rest) = inner_out.strip_suffix("\n...\n") {
        inner_out = rest.to_string();
    }
    let inner_out = inner_out.trim_matches('\n');
    let out = if has_frontmatter {
        format!("{prefix}---\n{inner_out}\n---\n{body}")
    } else {
        format!("---\n{inner_out}\n---\n{body}")
    };
    std::fs::write(&path, out)?;
    Ok(())
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
