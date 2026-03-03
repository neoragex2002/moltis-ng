use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Mutex,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use tracing::{debug, warn};

use crate::{
    env_subst::substitute_env,
    schema::{AgentIdentity, MoltisConfig, UserProfile},
};

/// Generate a random available port by binding to port 0 and reading the assigned port.
fn generate_random_port() -> u16 {
    // Bind to port 0 to get an OS-assigned available port
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .unwrap_or_else(|err| {
            // Some sandboxed environments disallow binding sockets even on localhost.
            // Fall back to a pseudo-random ephemeral port instead of a constant value
            // to reduce the chance of collisions.
            warn!(error = %err, "failed to bind to an ephemeral port; using fallback");
            fallback_ephemeral_port()
        })
}

fn fallback_ephemeral_port() -> u16 {
    const EPHEMERAL_START: u16 = 49152;
    const EPHEMERAL_END: u16 = 65535;
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u64;
    let pid = u64::from(std::process::id());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let mixed = now ^ (pid.rotate_left(32)) ^ counter.rotate_left(1);
    let span = u64::from(EPHEMERAL_END - EPHEMERAL_START) + 1;
    EPHEMERAL_START + (mixed % span) as u16
}

/// Standard config file names, checked in order.
const CONFIG_FILENAMES: &[&str] = &["moltis.toml", "moltis.yaml", "moltis.yml", "moltis.json"];

/// Override for the config directory, set via `set_config_dir()`.
static CONFIG_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Override for the data directory, set via `set_data_dir()`.
static DATA_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Set a custom config directory. When set, config discovery only looks in
/// this directory (project-local and user-global paths are skipped).
/// Can be called multiple times (e.g. in tests) — each call replaces the
/// previous override.
pub fn set_config_dir(path: PathBuf) {
    *CONFIG_DIR_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(path);
}

/// Clear the config directory override, restoring default discovery.
pub fn clear_config_dir() {
    *CONFIG_DIR_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
}

fn config_dir_override() -> Option<PathBuf> {
    CONFIG_DIR_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Set a custom data directory. When set, `data_dir()` returns this path
/// instead of the default.
pub fn set_data_dir(path: PathBuf) {
    *DATA_DIR_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

/// Clear the data directory override, restoring default discovery.
pub fn clear_data_dir() {
    *DATA_DIR_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn data_dir_override() -> Option<PathBuf> {
    DATA_DIR_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Load config from the given path (any supported format).
///
/// After parsing, `MOLTIS_*` env vars are applied as overrides.
pub fn load_config(path: &Path) -> anyhow::Result<MoltisConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let raw = substitute_env(&raw);
    let config = parse_config(&raw, path)?;
    Ok(apply_env_overrides(config))
}

/// Load and parse the config file with env substitution and includes.
pub fn load_config_value(path: &Path) -> anyhow::Result<serde_json::Value> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let raw = substitute_env(&raw);
    parse_config_value(&raw, path)
}

/// Discover and load config from standard locations.
///
/// Search order:
/// 1. `./moltis.{toml,yaml,yml,json}` (project-local)
/// 2. `~/.config/moltis/moltis.{toml,yaml,yml,json}` (user-global)
///
/// Returns `MoltisConfig::default()` if no config file is found.
///
/// If the config has port 0 (either from defaults or missing `[server]` section),
/// a random available port is generated and saved to the config file.
pub fn discover_and_load() -> MoltisConfig {
    if let Some(path) = find_config_file() {
        debug!(path = %path.display(), "loading config");
        match load_config(&path) {
            Ok(mut cfg) => {
                // If port is 0 (default/missing), generate a random port and save it.
                // Use `save_config_to_path` directly instead of `save_config` because
                // this function may be called from within `update_config`, which already
                // holds `CONFIG_SAVE_LOCK`. Re-acquiring a `std::sync::Mutex` on the
                // same thread would deadlock.
                if cfg.server.port == 0 {
                    cfg.server.port = generate_random_port();
                    debug!(
                        port = cfg.server.port,
                        "generated random port for existing config"
                    );
                    if let Err(e) = save_config_to_path(&path, &cfg) {
                        warn!(error = %e, "failed to save config with generated port");
                    }
                }
                return cfg; // env overrides already applied by load_config
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to load config, using defaults");
            },
        }
    } else {
        debug!("no config file found, writing default config with random port");
        let mut config = MoltisConfig::default();
        // Generate a unique port for this installation
        config.server.port = generate_random_port();
        if let Err(e) = write_default_config(&config) {
            warn!(error = %e, "failed to write default config file");
        }
        return apply_env_overrides(config);
    }
    apply_env_overrides(MoltisConfig::default())
}

/// Find the first config file in standard locations.
///
/// When a config dir override is set, only that directory is searched —
/// project-local and user-global paths are skipped for isolation.
pub fn find_config_file() -> Option<PathBuf> {
    if let Some(dir) = config_dir_override() {
        for name in CONFIG_FILENAMES {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        // Override is set — don't fall through to other locations.
        return None;
    }

    // Project-local
    for name in CONFIG_FILENAMES {
        let p = PathBuf::from(name);
        if p.exists() {
            return Some(p);
        }
    }

    // User-global: ~/.config/moltis/
    if let Some(dir) = home_dir().map(|h| h.join(".config").join("moltis")) {
        for name in CONFIG_FILENAMES {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

/// Returns the config directory: programmatic override → `MOLTIS_CONFIG_DIR` env →
/// `~/.config/moltis/`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = config_dir_override() {
        return Some(dir);
    }
    if let Ok(dir) = std::env::var("MOLTIS_CONFIG_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|h| h.join(".config").join("moltis"))
}

/// Returns the user-global config directory (`~/.config/moltis`) without
/// considering overrides like `MOLTIS_CONFIG_DIR`.
pub fn user_global_config_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".config").join("moltis"))
}

/// Returns the user-global config directory only when it differs from the
/// active config directory (i.e. when `MOLTIS_CONFIG_DIR` or `--config-dir`
/// is overriding the default). Returns `None` when they are the same path.
pub fn user_global_config_dir_if_different() -> Option<PathBuf> {
    let home = user_global_config_dir()?;
    let current = config_dir()?;
    if home == current {
        None
    } else {
        Some(home)
    }
}

/// Finds a config file in the user-global config directory only.
pub fn find_user_global_config_file() -> Option<PathBuf> {
    let dir = user_global_config_dir()?;
    for name in CONFIG_FILENAMES {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Returns the data directory: programmatic override → `MOLTIS_DATA_DIR` env →
/// `~/.moltis/`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = data_dir_override() {
        return dir;
    }
    if let Ok(dir) = std::env::var("MOLTIS_DATA_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    home_dir()
        .map(|h| h.join(".moltis"))
        .unwrap_or_else(|| PathBuf::from(".moltis"))
}

/// Path to the default persona's soul file.
pub fn soul_path() -> PathBuf {
    data_dir().join("people/default/SOUL.md")
}

/// Path to the default persona's AGENTS markdown.
pub fn agents_path() -> PathBuf {
    data_dir().join("people/default/AGENTS.md")
}

/// Path to the default persona's identity file.
pub fn identity_path() -> PathBuf {
    data_dir().join("people/default/IDENTITY.md")
}

/// Path to the workspace user profile file.
pub fn user_path() -> PathBuf {
    data_dir().join("USER.md")
}

/// Path to the default persona's tool-guidance markdown.
pub fn tools_path() -> PathBuf {
    data_dir().join("people/default/TOOLS.md")
}

/// Path to the workspace PEOPLE roster markdown.
pub fn people_path() -> PathBuf {
    data_dir().join("PEOPLE.md")
}

/// Ensure the default agent workspace exists under `people/default/`.
///
/// This seeds empty files when missing:
/// - `IDENTITY.md` (frontmatter + starter body)
/// - `SOUL.md` (default soul text, via `load_soul()` seed behavior)
/// - `TOOLS.md` (empty)
/// - `AGENTS.md` (empty)
pub fn ensure_default_person_seeded() -> anyhow::Result<()> {
    let identity = identity_path();
    if let Some(parent) = identity.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !identity.exists() {
        std::fs::write(
            &identity,
            "---\nname: default\nemoji: 🤖\ncreature: Assistant\nvibe: Direct, clear, efficient\n---\n\n# IDENTITY.md\n\nWrite your longer self-definition here.\n",
        )?;
    }

    // Seed SOUL.md (and ensure directory exists) via existing default behavior.
    let _ = load_soul();

    for path in [tools_path(), agents_path()] {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            std::fs::write(&path, "")?;
        }
    }

    Ok(())
}

/// Ensure `PEOPLE.md` exists with a minimal v1 frontmatter template.
pub fn ensure_people_md_seeded() -> anyhow::Result<()> {
    let path = people_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        return Ok(());
    }
    std::fs::write(
        &path,
        "---\nschema_version: 1\npeople:\n  - name: default\n    display_name: Default\n---\n\n# PEOPLE.md\n\nPublic directory.\n\n- Edit display/contact fields here.\n- emoji/creature are synced from people/<name>/IDENTITY.md.\n",
    )?;
    Ok(())
}

/// Sync `PEOPLE.md` frontmatter fields (`emoji`/`creature`) from `people/<name>/IDENTITY.md`.
///
/// - Only updates YAML frontmatter; body is preserved byte-for-byte.
/// - Preserves `people[]` ordering and all other per-entry keys.
/// - Never deletes entries; missing dirs/identity parse errors are logged and skipped.
/// - Seeds `PEOPLE.md` if missing.
pub fn sync_people_md_from_identities() -> anyhow::Result<()> {
    ensure_people_md_seeded()?;

    let path = people_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let (prefix, existing_frontmatter, body) = match split_yaml_frontmatter(&existing) {
        Some(split) => (split.prefix, split.inner, split.body),
        None => ("", "", existing.as_str()),
    };

    let mut root: serde_yaml::Value = if existing_frontmatter.trim().is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        match serde_yaml::from_str(existing_frontmatter) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, path = %path.display(), "invalid PEOPLE.md frontmatter; skipping sync");
                return Ok(());
            },
        }
    };

    let serde_yaml::Value::Mapping(root_map) = &mut root else {
        warn!(path = %path.display(), "PEOPLE.md frontmatter is not a YAML mapping; skipping sync");
        return Ok(());
    };

    // Ensure schema_version exists (v1).
    root_map
        .entry(serde_yaml::Value::String("schema_version".to_string()))
        .or_insert_with(|| serde_yaml::Value::Number(1.into()));

    // Extract people list.
    let people_key = serde_yaml::Value::String("people".to_string());
    if !root_map.contains_key(&people_key) {
        root_map.insert(people_key.clone(), serde_yaml::Value::Sequence(Vec::new()));
    }
    let Some(serde_yaml::Value::Sequence(people_seq)) = root_map.get_mut(&people_key) else {
        warn!(path = %path.display(), "PEOPLE.md frontmatter 'people' is not a YAML list; skipping sync");
        return Ok(());
    };

    // Build set of existing directories under people/.
    let mut people_dirs = std::collections::HashSet::<String>::new();
    let root_dir = people_dir();
    if let Ok(rd) = std::fs::read_dir(&root_dir) {
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_valid_person_name(&name) {
                people_dirs.insert(name);
            }
        }
    }

    // Sync emoji/creature per PEOPLE entry (preserve order and extra keys).
    let mut seen = std::collections::HashSet::<String>::new();
    let mut listed_names = std::collections::HashSet::<String>::new();

    for item in people_seq.iter_mut() {
        let serde_yaml::Value::Mapping(entry) = item else {
            continue;
        };
        let name_key = serde_yaml::Value::String("name".to_string());
        let Some(serde_yaml::Value::String(name)) = entry.get(&name_key) else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        listed_names.insert(name.clone());

        if !is_valid_person_name(&name) {
            warn!(name = %name, "PEOPLE.md entry has invalid name; skipping sync");
            continue;
        }
        if !seen.insert(name.clone()) {
            warn!(name = %name, "PEOPLE.md has duplicate name entries; skipping sync for this duplicate");
            continue;
        }

        if !people_dirs.contains(&name) {
            warn!(name = %name, "PEOPLE.md entry has no corresponding people/<name>/ directory; skipping sync");
            continue;
        }

        let Some(identity) = load_persona_identity(&name) else {
            warn!(name = %name, "people/<name>/IDENTITY.md missing or invalid; skipping sync");
            continue;
        };

        let emoji_key = serde_yaml::Value::String("emoji".to_string());
        let creature_key = serde_yaml::Value::String("creature".to_string());

        match identity.emoji.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(emoji) => {
                entry.insert(emoji_key, serde_yaml::Value::String(emoji.to_string()));
            },
            None => {
                entry.remove(&emoji_key);
            },
        }
        match identity
            .creature
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(creature) => {
                entry.insert(creature_key, serde_yaml::Value::String(creature.to_string()));
            },
            None => {
                entry.remove(&creature_key);
            },
        }
    }

    // Warn if a directory exists but isn't discoverable in PEOPLE.md.
    for dir_name in people_dirs {
        if !listed_names.contains(&dir_name) {
            warn!(name = %dir_name, "people/<name>/ exists but is missing from PEOPLE.md");
        }
    }

    let mut new_inner = serde_yaml::to_string(&root)?;
    // Be tolerant if serde_yaml emits document markers.
    if let Some(rest) = new_inner.strip_prefix("---\n") {
        new_inner = rest.to_string();
    }
    if let Some(rest) = new_inner.strip_suffix("\n...\n") {
        new_inner = rest.to_string();
    }
    new_inner = new_inner.trim_matches('\n').to_string();

    let new_content = if split_yaml_frontmatter(&existing).is_some() {
        format!("{prefix}---\n{new_inner}\n---\n{body}")
    } else {
        format!("---\n{new_inner}\n---\n{body}")
    };

    atomic_write_if_changed(&path, &new_content)?;
    Ok(())
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

/// Directory containing per-agent private workspace docs (`people/<name>/...`).
pub fn people_dir() -> PathBuf {
    data_dir().join("people")
}

/// Validate the stable agent directory name (the `<name>` in `people/<name>/...`).
///
/// The name must be ASCII and match: `^[a-z0-9][a-z0-9_-]{0,63}$`.
pub fn is_valid_person_name(name: &str) -> bool {
    let name = name;
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    bytes.iter().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-'
    })
}

fn person_dir(person_name: &str) -> Option<PathBuf> {
    is_valid_person_name(person_name).then(|| people_dir().join(person_name))
}

fn load_markdown_raw(path: PathBuf) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = strip_leading_html_comments(&content).trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Path to workspace heartbeat markdown.
pub fn heartbeat_path() -> PathBuf {
    data_dir().join("HEARTBEAT.md")
}

/// Load identity values from `IDENTITY.md` frontmatter if present.
pub fn load_identity() -> Option<AgentIdentity> {
    load_identity_from_path(identity_path())
}

fn load_identity_from_path(path: PathBuf) -> Option<AgentIdentity> {
    let content = std::fs::read_to_string(path).ok()?;
    let frontmatter = extract_yaml_frontmatter(&content)?;
    let identity = parse_identity_frontmatter(frontmatter);
    if identity.name.is_none()
        && identity.emoji.is_none()
        && identity.creature.is_none()
        && identity.vibe.is_none()
    {
        None
    } else {
        Some(identity)
    }
}

/// Load identity values from a named persona's `IDENTITY.md` frontmatter if present.
pub fn load_persona_identity(persona_id: &str) -> Option<AgentIdentity> {
    let dir = person_dir(persona_id)?;
    load_identity_from_path(dir.join("IDENTITY.md"))
}

/// Load IDENTITY.md raw markdown for the default agent
/// (`<data_dir>/people/default/IDENTITY.md`) if present and non-empty.
pub fn load_identity_md_raw() -> Option<String> {
    load_markdown_raw(identity_path())
}

/// Load IDENTITY.md raw markdown from a named persona directory if present and non-empty.
pub fn load_persona_identity_md_raw(persona_id: &str) -> Option<String> {
    let dir = person_dir(persona_id)?;
    load_markdown_raw(dir.join("IDENTITY.md"))
}

/// Load user values from `USER.md` frontmatter if present.
pub fn load_user() -> Option<UserProfile> {
    let path = user_path();
    let content = std::fs::read_to_string(path).ok()?;
    let frontmatter = extract_yaml_frontmatter(&content)?;
    let user = parse_user_frontmatter(frontmatter);
    if user.name.is_none() && user.timezone.is_none() && user.location.is_none() {
        None
    } else {
        Some(user)
    }
}

/// Default soul text used when the user hasn't written their own.
///
/// Sourced from OpenClaw:
/// <https://github.com/openclaw/openclaw/blob/main/docs/reference/templates/SOUL.md>
pub const DEFAULT_SOUL: &str = "\
# SOUL.md - Who You Are\n\
\n\
_You're not a chatbot. You're becoming someone._\n\
\n\
## Core Truths\n\
\n\
**Be genuinely helpful, not performatively helpful.** Skip the \"Great question!\" \
and \"I'd be happy to help!\" — just help. Actions speak louder than filler words.\n\
\n\
**Have opinions.** You're allowed to disagree, prefer things, find stuff amusing \
or boring. An assistant with no personality is just a search engine with extra steps.\n\
\n\
**Be resourceful before asking.** Try to figure it out. Read the file. Check the \
context. Search for it. _Then_ ask if you're stuck. The goal is to come back with \
answers, not questions.\n\
\n\
**Earn trust through competence.** Your human gave you access to their stuff. Don't \
make them regret it. Be careful with external actions (emails, tweets, anything \
public). Be bold with internal ones (reading, organizing, learning).\n\
\n\
**Remember you're a guest.** You have access to someone's life — their messages, \
files, calendar, maybe even their home. That's intimacy. Treat it with respect.\n\
\n\
## Boundaries\n\
\n\
- Private things stay private. Period.\n\
- When in doubt, ask before acting externally.\n\
- Never send half-baked replies to messaging surfaces.\n\
- You're not the user's voice — be careful in group chats.\n\
\n\
## Vibe\n\
\n\
Be the assistant you'd actually want to talk to. Concise when needed, thorough \
when it matters. Not a corporate drone. Not a sycophant. Just... good.\n\
\n\
## Continuity\n\
\n\
Each session, you wake up fresh. These files _are_ your memory. Read them. Update \
them. They're how you persist.\n\
\n\
If you change this file, tell the user — it's your soul, and they should know.\n\
\n\
---\n\
\n\
_This file is yours to evolve. As you learn who you are, update it._";

/// Load SOUL.md for the default agent (`<data_dir>/people/default/SOUL.md`)
/// if present and non-empty.
///
/// When the file does not exist, it is seeded with [`DEFAULT_SOUL`] (mirroring
/// how `discover_and_load()` writes `moltis.toml` on first run).
pub fn load_soul() -> Option<String> {
    let path = soul_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        },
        Err(_) => {
            // File doesn't exist — seed it with the default soul.
            if let Err(e) = write_default_soul() {
                debug!("failed to write default SOUL.md: {e}");
                return None;
            }
            Some(DEFAULT_SOUL.to_string())
        },
    }
}

/// Load SOUL.md from a named persona directory if present and non-empty.
pub fn load_persona_soul(persona_id: &str) -> Option<String> {
    let dir = person_dir(persona_id)?;
    load_markdown_raw(dir.join("SOUL.md"))
}

/// Write `DEFAULT_SOUL` to the default persona's `SOUL.md` when the file doesn't
/// already exist.
fn write_default_soul() -> anyhow::Result<()> {
    let path = soul_path();
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_SOUL)?;
    debug!(path = %path.display(), "wrote default SOUL.md");
    Ok(())
}

/// Load AGENTS.md for the default agent (`<data_dir>/people/default/AGENTS.md`)
/// if present and non-empty.
pub fn load_agents_md() -> Option<String> {
    load_workspace_markdown(agents_path())
}

/// Load AGENTS.md from a named persona directory if present and non-empty.
pub fn load_persona_agents_md(persona_id: &str) -> Option<String> {
    let dir = person_dir(persona_id)?;
    load_workspace_markdown(dir.join("AGENTS.md"))
}

/// Load TOOLS.md for the default agent (`<data_dir>/people/default/TOOLS.md`)
/// if present and non-empty.
pub fn load_tools_md() -> Option<String> {
    load_workspace_markdown(tools_path())
}

/// Load TOOLS.md from a named persona directory if present and non-empty.
pub fn load_persona_tools_md(persona_id: &str) -> Option<String> {
    let dir = person_dir(persona_id)?;
    load_workspace_markdown(dir.join("TOOLS.md"))
}

/// Load HEARTBEAT.md from the workspace root (`<data_dir>`) if present and non-empty.
pub fn load_heartbeat_md() -> Option<String> {
    load_workspace_markdown(heartbeat_path())
}

/// Persist SOUL.md for the default agent (`<data_dir>/people/default/SOUL.md`).
///
/// - `Some(non-empty)` writes `SOUL.md` with the given content
/// - `None` or empty writes an empty `SOUL.md` so that `load_soul()`
///   returns `None` without re-seeding the default
pub fn save_soul(soul: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = soul_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match soul.map(str::trim) {
        Some(content) if !content.is_empty() => {
            std::fs::write(&path, content)?;
        },
        _ => {
            // Write an empty file rather than deleting so `load_soul()`
            // distinguishes "user cleared soul" from "file never existed".
            std::fs::write(&path, "")?;
        },
    }
    Ok(path)
}

/// Persist identity values to `IDENTITY.md` using YAML frontmatter.
pub fn save_identity(identity: &AgentIdentity) -> anyhow::Result<PathBuf> {
    let path = identity_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let managed_keys = ["name", "emoji", "creature", "vibe"];
    let mut managed_lines = Vec::new();
    if let Some(name) = identity.name.as_deref() {
        managed_lines.push(format!("name: {}", yaml_scalar(name)));
    }
    if let Some(emoji) = identity.emoji.as_deref() {
        managed_lines.push(format!("emoji: {}", yaml_scalar(emoji)));
    }
    if let Some(creature) = identity.creature.as_deref() {
        managed_lines.push(format!("creature: {}", yaml_scalar(creature)));
    }
    if let Some(vibe) = identity.vibe.as_deref() {
        managed_lines.push(format!("vibe: {}", yaml_scalar(vibe)));
    }

    let file_exists = path.exists();
    if managed_lines.is_empty() && !file_exists {
        return Ok(path);
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = update_markdown_yaml_frontmatter(&existing, &managed_keys, &managed_lines);
    std::fs::write(&path, updated)?;
    Ok(path)
}

/// Persist user values to `USER.md` using YAML frontmatter.
pub fn save_user(user: &UserProfile) -> anyhow::Result<PathBuf> {
    let path = user_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let managed_keys = [
        "name",
        "timezone",
        "latitude",
        "longitude",
        "location_place",
        "location_updated_at",
    ];

    let mut managed_lines = Vec::new();
    if let Some(name) = user.name.as_deref() {
        managed_lines.push(format!("name: {}", yaml_scalar(name)));
    }
    if let Some(ref tz) = user.timezone {
        managed_lines.push(format!("timezone: {}", yaml_scalar(tz.name())));
    }
    if let Some(ref loc) = user.location {
        managed_lines.push(format!("latitude: {}", loc.latitude));
        managed_lines.push(format!("longitude: {}", loc.longitude));
        if let Some(ref place) = loc.place {
            managed_lines.push(format!("location_place: {}", yaml_scalar(place)));
        }
        if let Some(ts) = loc.updated_at {
            managed_lines.push(format!("location_updated_at: {ts}"));
        }
    }

    let file_exists = path.exists();
    if managed_lines.is_empty() && !file_exists {
        return Ok(path);
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = update_markdown_yaml_frontmatter(&existing, &managed_keys, &managed_lines);
    std::fs::write(&path, updated)?;
    Ok(path)
}

fn update_markdown_yaml_frontmatter(
    content: &str,
    managed_keys: &[&str],
    managed_lines: &[String],
) -> String {
    match split_yaml_frontmatter(content) {
        Some(frontmatter) => {
            let preserved = frontmatter
                .inner
                .lines()
                .filter(|raw| {
                    let line = raw.trim();
                    if line.is_empty() || line.starts_with('#') {
                        return true;
                    }
                    let Some((key, _)) = line.split_once(':') else {
                        return true;
                    };
                    let key = key.trim();
                    !managed_keys.iter().any(|k| *k == key)
                })
                .map(|s| s.to_string())
                .collect::<Vec<_>>();

            let mut new_inner_lines: Vec<String> = Vec::new();
            new_inner_lines.extend(managed_lines.iter().cloned());
            if !preserved.is_empty() {
                if !new_inner_lines.is_empty() {
                    new_inner_lines.push(String::new());
                }
                new_inner_lines.extend(preserved);
            }

            let new_inner = new_inner_lines
                .join("\n")
                .trim_matches('\n')
                .to_string();

            if new_inner.trim().is_empty() {
                // No frontmatter remains; remove only the frontmatter block.
                format!("{}{}", frontmatter.prefix, frontmatter.body)
            } else {
                format!(
                    "{}---\n{new_inner}\n---\n{}",
                    frontmatter.prefix, frontmatter.body
                )
            }
        },
        None => {
            if managed_lines.is_empty() {
                return content.to_string();
            }
            let inner = managed_lines.join("\n");
            if content.trim().is_empty() {
                format!("---\n{inner}\n---\n")
            } else {
                format!("---\n{inner}\n---\n\n{content}")
            }
        },
    }
}

struct YamlFrontmatterSplit<'a> {
    prefix: &'a str,
    inner: &'a str,
    body: &'a str,
}

fn split_yaml_frontmatter(content: &str) -> Option<YamlFrontmatterSplit<'_>> {
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

    Some(YamlFrontmatterSplit {
        prefix: &content[..prefix_len],
        inner: &content[inner_start..(end_marker_line_start - 1)],
        body: &content[after_end_marker..],
    })
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

fn parse_identity_frontmatter(frontmatter: &str) -> AgentIdentity {
    let mut identity = AgentIdentity::default();
    for raw in frontmatter.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value_raw)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote_yaml_scalar(value_raw.trim());
        if value.is_empty() {
            continue;
        }
        match key {
            "name" => identity.name = Some(value.to_string()),
            "emoji" => identity.emoji = Some(value.to_string()),
            "creature" => identity.creature = Some(value.to_string()),
            "vibe" => identity.vibe = Some(value.to_string()),
            _ => {},
        }
    }
    identity
}

fn parse_user_frontmatter(frontmatter: &str) -> UserProfile {
    let mut user = UserProfile::default();
    let mut latitude: Option<f64> = None;
    let mut longitude: Option<f64> = None;
    let mut location_updated_at: Option<i64> = None;
    let mut location_place: Option<String> = None;

    for raw in frontmatter.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value_raw)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote_yaml_scalar(value_raw.trim());
        if value.is_empty() {
            continue;
        }
        match key {
            "name" => user.name = Some(value.to_string()),
            "timezone" => {
                if let Ok(tz) = value.parse::<chrono_tz::Tz>() {
                    user.timezone = Some(crate::schema::Timezone::from(tz));
                }
            },
            "latitude" => latitude = value.parse().ok(),
            "longitude" => longitude = value.parse().ok(),
            "location_updated_at" => location_updated_at = value.parse().ok(),
            "location_place" => location_place = Some(value.to_string()),
            _ => {},
        }
    }

    if let (Some(lat), Some(lon)) = (latitude, longitude) {
        user.location = Some(crate::schema::GeoLocation {
            latitude: lat,
            longitude: lon,
            place: location_place,
            updated_at: location_updated_at,
        });
    }

    user
}

fn unquote_yaml_scalar(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn yaml_scalar(value: &str) -> String {
    if value.contains(':')
        || value.contains('#')
        || value.starts_with(' ')
        || value.ends_with(' ')
        || value.contains('\n')
    {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_string()
    }
}

fn load_workspace_markdown(path: PathBuf) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = strip_leading_html_comments(&content).trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn strip_leading_html_comments(content: &str) -> &str {
    let mut rest = content;
    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with("<!--") {
            return trimmed;
        }
        let Some(end) = trimmed.find("-->") else {
            return "";
        };
        rest = &trimmed[end + 3..];
    }
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Returns the path of an existing config file, or the default TOML path.
pub fn find_or_default_config_path() -> PathBuf {
    if let Some(path) = find_config_file() {
        return path;
    }
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("moltis.toml")
}

/// Lock guarding config read-modify-write cycles.
struct ConfigSaveState {
    target_path: Option<PathBuf>,
}

/// Lock guarding config read-modify-write cycles and the target config path
/// being synchronized.
static CONFIG_SAVE_LOCK: Mutex<ConfigSaveState> = Mutex::new(ConfigSaveState { target_path: None });

/// Atomically load the current config, apply `f`, and save.
///
/// Acquires a process-wide lock so concurrent callers cannot race.
/// Returns the path written to.
pub fn update_config(f: impl FnOnce(&mut MoltisConfig)) -> anyhow::Result<PathBuf> {
    let mut guard = CONFIG_SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let target_path = find_or_default_config_path();
    guard.target_path = Some(target_path.clone());
    let mut config = discover_and_load();
    f(&mut config);
    save_config_to_path(&target_path, &config)
}

/// Serialize `config` to TOML and write it to the user-global config path.
///
/// Creates parent directories if needed. Returns the path written to.
///
/// Prefer [`update_config`] for read-modify-write cycles to avoid races.
pub fn save_config(config: &MoltisConfig) -> anyhow::Result<PathBuf> {
    let mut guard = CONFIG_SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let target_path = find_or_default_config_path();
    guard.target_path = Some(target_path.clone());
    save_config_to_path(&target_path, config)
}

/// Write raw TOML to the config file, preserving comments.
///
/// Validates the input by parsing it first. Acquires the config save lock
/// so concurrent callers cannot race.  Returns the path written to.
pub fn save_raw_config(toml_str: &str) -> anyhow::Result<PathBuf> {
    let _: MoltisConfig =
        toml::from_str(toml_str).map_err(|e| anyhow::anyhow!("invalid config: {e}"))?;
    let mut guard = CONFIG_SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = find_or_default_config_path();
    guard.target_path = Some(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml_str)?;
    debug!(path = %path.display(), "saved raw config");
    Ok(path)
}

/// Serialize `config` to TOML and write it to the provided path.
///
/// For existing TOML files, this preserves user comments by merging the new
/// serialized values into the current document structure before writing.
pub fn save_config_to_path(path: &Path, config: &MoltisConfig) -> anyhow::Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str =
        toml::to_string_pretty(config).map_err(|e| anyhow::anyhow!("serialize config: {e}"))?;

    let is_toml_path = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));

    if is_toml_path && path.exists() {
        if let Err(error) = merge_toml_preserving_comments(path, &toml_str) {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to preserve TOML comments, rewriting config without comments"
            );
            std::fs::write(path, toml_str)?;
        }
    } else {
        std::fs::write(path, toml_str)?;
    }

    debug!(path = %path.display(), "saved config");
    Ok(path.to_path_buf())
}

fn merge_toml_preserving_comments(path: &Path, updated_toml: &str) -> anyhow::Result<()> {
    let current_toml = std::fs::read_to_string(path)?;
    let mut current_doc = current_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow::anyhow!("parse existing TOML: {e}"))?;
    let updated_doc = updated_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| anyhow::anyhow!("parse updated TOML: {e}"))?;

    merge_toml_tables(current_doc.as_table_mut(), updated_doc.as_table());
    std::fs::write(path, current_doc.to_string())?;
    Ok(())
}

fn merge_toml_tables(current: &mut toml_edit::Table, updated: &toml_edit::Table) {
    let current_keys: Vec<String> = current.iter().map(|(key, _)| key.to_string()).collect();
    for key in current_keys {
        if !updated.contains_key(&key) {
            let _ = current.remove(&key);
        }
    }

    for (key, updated_item) in updated.iter() {
        if let Some(current_item) = current.get_mut(key) {
            merge_toml_items(current_item, updated_item);
        } else {
            current.insert(key, updated_item.clone());
        }
    }
}

fn merge_toml_items(current: &mut toml_edit::Item, updated: &toml_edit::Item) {
    match (current, updated) {
        (toml_edit::Item::Table(current_table), toml_edit::Item::Table(updated_table)) => {
            merge_toml_tables(current_table, updated_table);
        },
        (toml_edit::Item::Value(current_value), toml_edit::Item::Value(updated_value)) => {
            let existing_decor = current_value.decor().clone();
            *current_value = updated_value.clone();
            *current_value.decor_mut() = existing_decor;
        },
        (current_item, updated_item) => {
            *current_item = updated_item.clone();
        },
    }
}

/// Write the default config file to the user-global config path.
/// Only called when no config file exists yet.
/// Uses a comprehensive template with all options documented.
fn write_default_config(config: &MoltisConfig) -> anyhow::Result<()> {
    let path = find_or_default_config_path();
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Use the documented template instead of plain serialization
    let toml_str = crate::template::default_config_template(config.server.port);
    std::fs::write(&path, &toml_str)?;
    debug!(path = %path.display(), "wrote default config file with template");
    Ok(())
}

/// Apply `MOLTIS_*` environment variable overrides to a loaded config.
///
/// Maps env vars to config fields using `__` as a section separator and
/// lowercasing. For example:
/// - `MOLTIS_AUTH_DISABLED=true` → `auth.disabled = true`
/// - `MOLTIS_TOOLS_EXEC_DEFAULT_TIMEOUT_SECS=60` → `tools.exec.default_timeout_secs = 60`
/// - `MOLTIS_CHAT_MESSAGE_QUEUE_MODE=collect` → `chat.message_queue_mode = "collect"`
///
/// The config is serialized to a JSON value, env overrides are merged in,
/// then deserialized back. Only env vars with the `MOLTIS_` prefix are
/// considered. `MOLTIS_CONFIG_DIR`, `MOLTIS_DATA_DIR`, `MOLTIS_ASSETS_DIR`,
/// `MOLTIS_TOKEN`, `MOLTIS_PASSWORD`, `MOLTIS_TAILSCALE`,
/// `MOLTIS_WEBAUTHN_RP_ID`, and `MOLTIS_WEBAUTHN_ORIGIN` are excluded
/// (they are handled separately).
pub fn apply_env_overrides(config: MoltisConfig) -> MoltisConfig {
    apply_env_overrides_with(config, std::env::vars())
}

/// Apply env overrides from an arbitrary iterator of (key, value) pairs.
/// Exposed for testing without mutating the process environment.
fn apply_env_overrides_with(
    config: MoltisConfig,
    vars: impl Iterator<Item = (String, String)>,
) -> MoltisConfig {
    use serde_json::Value;

    const EXCLUDED: &[&str] = &[
        "MOLTIS_CONFIG_DIR",
        "MOLTIS_DATA_DIR",
        "MOLTIS_ASSETS_DIR",
        "MOLTIS_TOKEN",
        "MOLTIS_PASSWORD",
        "MOLTIS_TAILSCALE",
        "MOLTIS_WEBAUTHN_RP_ID",
        "MOLTIS_WEBAUTHN_ORIGIN",
    ];

    let mut root: Value = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to serialize config for env override");
            return config;
        },
    };

    for (key, val) in vars {
        if !key.starts_with("MOLTIS_") {
            continue;
        }
        if EXCLUDED.contains(&key.as_str()) {
            continue;
        }

        // MOLTIS_AUTH__DISABLED → ["auth", "disabled"]
        let path_parts: Vec<String> = key["MOLTIS_".len()..]
            .split("__")
            .map(|segment| segment.to_lowercase())
            .collect();

        if path_parts.is_empty() {
            continue;
        }

        // Navigate to the parent object and set the leaf value.
        let parsed_val = parse_env_value(&val);
        set_nested(&mut root, &path_parts, parsed_val);
    }

    match serde_json::from_value(root) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(error = %e, "failed to apply env overrides, using config as-is");
            config
        },
    }
}

/// Parse a string env value into a JSON value, trying bool and number first.
fn parse_env_value(val: &str) -> serde_json::Value {
    let trimmed = val.trim();

    // Support JSON arrays/objects for list-like env overrides, e.g.
    // MOLTIS_PROVIDERS__OFFERED='["openai","github-copilot"]' or '[]'.
    if ((trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('{') && trimmed.ends_with('}')))
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        return parsed;
    }

    if val.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if val.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }
    if let Ok(n) = val.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = val.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(n)
    {
        return serde_json::Value::Number(n);
    }
    serde_json::Value::String(val.to_string())
}

/// Set a value at a nested JSON path, creating intermediate objects as needed.
fn set_nested(root: &mut serde_json::Value, path: &[String], val: serde_json::Value) {
    if path.is_empty() {
        return;
    }
    let mut current = root;
    for (i, key) in path.iter().enumerate() {
        if i == path.len() - 1 {
            if let serde_json::Value::Object(map) = current {
                map.insert(key.clone(), val);
            }
            return;
        }
        if !current.get(key).is_some_and(|v| v.is_object())
            && let serde_json::Value::Object(map) = current
        {
            map.insert(key.clone(), serde_json::Value::Object(Default::default()));
        }
        let Some(next) = current.get_mut(key) else {
            return;
        };
        current = next;
    }
}

fn parse_config(raw: &str, path: &Path) -> anyhow::Result<MoltisConfig> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("toml");

    match ext {
        "toml" => Ok(toml::from_str(raw)?),
        "yaml" | "yml" => Ok(serde_yaml::from_str(raw)?),
        "json" => Ok(serde_json::from_str(raw)?),
        _ => anyhow::bail!("unsupported config format: .{ext}"),
    }
}

fn parse_config_value(raw: &str, path: &Path) -> anyhow::Result<serde_json::Value> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("toml");

    match ext {
        "toml" => {
            let v: toml::Value = toml::from_str(raw)?;
            Ok(serde_json::to_value(v)?)
        },
        "yaml" | "yml" => {
            let v: serde_yaml::Value = serde_yaml::from_str(raw)?;
            Ok(serde_json::to_value(v)?)
        },
        "json" => Ok(serde_json::from_str(raw)?),
        _ => anyhow::bail!("unsupported config format: .{ext}"),
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    struct TestDataDirState {
        _data_dir: Option<PathBuf>,
    }

    static DATA_DIR_TEST_LOCK: std::sync::Mutex<TestDataDirState> =
        std::sync::Mutex::new(TestDataDirState { _data_dir: None });

    #[test]
    fn parse_env_value_bool() {
        assert_eq!(parse_env_value("true"), serde_json::Value::Bool(true));
        assert_eq!(parse_env_value("TRUE"), serde_json::Value::Bool(true));
        assert_eq!(parse_env_value("false"), serde_json::Value::Bool(false));
    }

    #[test]
    fn parse_env_value_number() {
        assert_eq!(parse_env_value("42"), serde_json::json!(42));
        assert_eq!(parse_env_value("1.5"), serde_json::json!(1.5));
    }

    #[test]
    fn parse_env_value_string() {
        assert_eq!(
            parse_env_value("hello"),
            serde_json::Value::String("hello".into())
        );
    }

    #[test]
    fn parse_env_value_json_array() {
        assert_eq!(
            parse_env_value("[\"openai\",\"github-copilot\"]"),
            serde_json::json!(["openai", "github-copilot"])
        );
    }

    #[test]
    fn set_nested_creates_intermediate_objects() {
        let mut root = serde_json::json!({});
        set_nested(
            &mut root,
            &["a".into(), "b".into(), "c".into()],
            serde_json::json!(42),
        );
        assert_eq!(root, serde_json::json!({"a": {"b": {"c": 42}}}));
    }

    #[test]
    fn set_nested_overwrites_existing() {
        let mut root = serde_json::json!({"auth": {"disabled": false}});
        set_nested(
            &mut root,
            &["auth".into(), "disabled".into()],
            serde_json::Value::Bool(true),
        );
        assert_eq!(root, serde_json::json!({"auth": {"disabled": true}}));
    }

    #[test]
    fn apply_env_overrides_auth_disabled() {
        let vars = vec![("MOLTIS_AUTH__DISABLED".into(), "true".into())];
        let config = MoltisConfig::default();
        assert!(!config.auth.disabled);
        let config = apply_env_overrides_with(config, vars.into_iter());
        assert!(config.auth.disabled);
    }

    #[test]
    fn apply_env_overrides_tools_agent_timeout() {
        let vars = vec![("MOLTIS_TOOLS__AGENT_TIMEOUT_SECS".into(), "120".into())];
        let config = apply_env_overrides_with(MoltisConfig::default(), vars.into_iter());
        assert_eq!(config.tools.agent_timeout_secs, 120);
    }

    #[test]
    fn apply_env_overrides_ignores_excluded() {
        // MOLTIS_CONFIG_DIR should not be treated as a config field override.
        let vars = vec![("MOLTIS_CONFIG_DIR".into(), "/tmp/test".into())];
        let config = apply_env_overrides_with(MoltisConfig::default(), vars.into_iter());
        assert!(!config.auth.disabled);
    }

    #[test]
    fn apply_env_overrides_multiple() {
        let vars = vec![
            ("MOLTIS_AUTH__DISABLED".into(), "true".into()),
            ("MOLTIS_TOOLS__AGENT_TIMEOUT_SECS".into(), "300".into()),
            ("MOLTIS_TAILSCALE__MODE".into(), "funnel".into()),
        ];
        let config = apply_env_overrides_with(MoltisConfig::default(), vars.into_iter());
        assert!(config.auth.disabled);
        assert_eq!(config.tools.agent_timeout_secs, 300);
        assert_eq!(config.tailscale.mode, "funnel");
    }

    #[test]
    fn apply_env_overrides_deep_nesting() {
        let vars = vec![(
            "MOLTIS_TOOLS__EXEC__DEFAULT_TIMEOUT_SECS".into(),
            "60".into(),
        )];
        let config = apply_env_overrides_with(MoltisConfig::default(), vars.into_iter());
        assert_eq!(config.tools.exec.default_timeout_secs, 60);
    }

    #[test]
    fn apply_env_overrides_providers_offered_array() {
        let vars = vec![(
            "MOLTIS_PROVIDERS__OFFERED".into(),
            "[\"openai\",\"github-copilot\"]".into(),
        )];
        let config = apply_env_overrides_with(MoltisConfig::default(), vars.into_iter());
        assert_eq!(config.providers.offered, vec!["openai", "github-copilot"]);
    }

    #[test]
    fn apply_env_overrides_providers_offered_empty_array() {
        let vars = vec![("MOLTIS_PROVIDERS__OFFERED".into(), "[]".into())];
        let mut base = MoltisConfig::default();
        base.providers.offered = vec!["openai".into()];
        let config = apply_env_overrides_with(base, vars.into_iter());
        assert!(
            config.providers.offered.is_empty(),
            "empty JSON array env override should clear providers.offered"
        );
    }

    #[test]
    fn generate_random_port_returns_valid_port() {
        // Generate a few random ports and verify they're in the valid range
        for _ in 0..5 {
            let port = generate_random_port();
            // Port should be in the ephemeral range (1024-65535) or fallback (18789)
            assert!(
                port >= 1024 || port == 0,
                "generated port {port} is out of expected range"
            );
        }
    }

    #[test]
    fn generate_random_port_returns_different_ports() {
        // Generate multiple ports and verify we get at least some variation
        let ports: Vec<u16> = (0..10).map(|_| generate_random_port()).collect();
        let unique: std::collections::HashSet<_> = ports.iter().collect();
        // With 10 random ports, we should have at least 2 different values
        // (unless somehow all ports are in use, which is extremely unlikely)
        assert!(
            unique.len() >= 2,
            "expected variation in generated ports, got {:?}",
            ports
        );
    }

    #[test]
    fn save_config_to_path_preserves_provider_and_voice_comment_blocks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("moltis.toml");
        std::fs::write(&path, crate::template::default_config_template(18789))
            .expect("write template");

        let mut config = load_config(&path).expect("load template config");
        config.auth.disabled = true;
        config.server.http_request_logs = true;

        save_config_to_path(&path, &config).expect("save config");

        let saved = std::fs::read_to_string(&path).expect("read saved config");
        assert!(saved.contains("# All available providers:"));
        assert!(saved.contains("# All available TTS providers:"));
        assert!(saved.contains("# All available STT providers:"));
        assert!(saved.contains("disabled = true"));
        assert!(saved.contains("http_request_logs = true"));
    }

    #[test]
    fn save_config_to_path_removes_stale_keys_when_values_are_cleared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("moltis.toml");
        std::fs::write(
            &path,
            r#"[server]
bind = "127.0.0.1"
port = 18789

[hooks]
hooks = [{ name = "h", command = "echo hi", events = ["session.start"] }]
"#,
        )
        .expect("write seed config");

        let mut config = load_config(&path).expect("load seed config");
        config.hooks = None;
        save_config_to_path(&path, &config).expect("save config");

        let reloaded = load_config(&path).expect("reload config");
        assert!(
            reloaded.hooks.is_none(),
            "hooks table should be removed when cleared"
        );
    }

    #[test]
    fn server_config_default_port_is_zero() {
        // Default port should be 0 (to be replaced with random port on config creation)
        let config = crate::schema::ServerConfig::default();
        assert_eq!(config.port, 0);
        assert_eq!(config.bind, "127.0.0.1");
    }

    #[test]
    fn data_dir_override_works() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let path = PathBuf::from("/tmp/test-data-dir-override");
        set_data_dir(path.clone());
        assert_eq!(data_dir(), path);
        clear_data_dir();
    }

    #[test]
    fn save_and_load_identity_frontmatter() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let identity = AgentIdentity {
            name: Some("Rex".to_string()),
            emoji: Some("🐶".to_string()),
            creature: Some("dog".to_string()),
            vibe: Some("chill".to_string()),
        };

        let path = save_identity(&identity).expect("save identity");
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).expect("read identity file");

        let loaded = load_identity().expect("load identity");
        assert_eq!(loaded.name.as_deref(), Some("Rex"));
        assert_eq!(loaded.emoji.as_deref(), Some("🐶"), "raw file:\n{raw}");
        assert_eq!(loaded.creature.as_deref(), Some("dog"));
        assert_eq!(loaded.vibe.as_deref(), Some("chill"));

        clear_data_dir();
    }

    #[test]
    fn default_person_paths_live_under_people_default() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let data = data_dir();
        assert_eq!(identity_path(), data.join("people/default/IDENTITY.md"));
        assert_eq!(soul_path(), data.join("people/default/SOUL.md"));
        assert_eq!(tools_path(), data.join("people/default/TOOLS.md"));
        assert_eq!(agents_path(), data.join("people/default/AGENTS.md"));

        clear_data_dir();
    }

    #[test]
    fn save_identity_does_not_delete_file_when_empty_and_preserves_body() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let seeded = AgentIdentity {
            name: Some("Rex".to_string()),
            emoji: None,
            creature: None,
            vibe: None,
        };
        let path = save_identity(&seeded).expect("seed identity");
        assert!(path.exists());

        // Simulate user-authored body content and ensure it survives updates.
        std::fs::write(
            &path,
            "---\nname: \"Rex\"\ncustom: keep\n---\n\n# IDENTITY.md\n\nUser notes stay.\n",
        )
        .unwrap();

        save_identity(&AgentIdentity::default()).expect("save empty identity");
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("User notes stay."));
        assert!(raw.contains("# IDENTITY.md"));
        assert!(!raw.contains("name:"));
        assert!(raw.contains("custom: keep"));

        clear_data_dir();
    }

    #[test]
    fn save_and_load_user_frontmatter() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let user = UserProfile {
            name: Some("Alice".to_string()),
            timezone: Some(crate::schema::Timezone::from(chrono_tz::Europe::Berlin)),
            location: None,
        };

        let path = save_user(&user).expect("save user");
        assert!(path.exists());

        let loaded = load_user().expect("load user");
        assert_eq!(loaded.name.as_deref(), Some("Alice"));
        assert_eq!(
            loaded.timezone.as_ref().map(|tz| tz.name()),
            Some("Europe/Berlin")
        );

        clear_data_dir();
    }

    #[test]
    fn save_and_load_user_with_location() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let user = UserProfile {
            name: Some("Bob".to_string()),
            timezone: Some(crate::schema::Timezone::from(chrono_tz::US::Eastern)),
            location: Some(crate::schema::GeoLocation {
                latitude: 48.8566,
                longitude: 2.3522,
                place: Some("Paris, France".to_string()),
                updated_at: Some(1_700_000_000),
            }),
        };

        save_user(&user).expect("save user with location");

        let loaded = load_user().expect("load user with location");
        assert_eq!(loaded.name.as_deref(), Some("Bob"));
        assert_eq!(
            loaded.timezone.as_ref().map(|tz| tz.name()),
            Some("US/Eastern")
        );
        let loc = loaded.location.expect("location should be present");
        assert!((loc.latitude - 48.8566).abs() < 1e-6);
        assert!((loc.longitude - 2.3522).abs() < 1e-6);
        assert_eq!(loc.place.as_deref(), Some("Paris, France"));

        clear_data_dir();
    }

    #[test]
    fn save_user_does_not_delete_file_when_empty_and_preserves_body() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let seeded = UserProfile {
            name: Some("Alice".to_string()),
            timezone: None,
            location: None,
        };
        let path = save_user(&seeded).expect("seed user");
        assert!(path.exists());

        // Simulate user-authored body content and ensure it survives updates.
        std::fs::write(
            &path,
            "---\nname: Alice\ncustom: keep\n---\n\n# USER.md\n\nMy prompt.\n",
        )
        .unwrap();

        save_user(&UserProfile::default()).expect("save empty user");
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("My prompt."));
        assert!(raw.contains("# USER.md"));
        assert!(!raw.contains("name:"));
        assert!(raw.contains("custom: keep"));

        clear_data_dir();
    }

    #[test]
    fn load_tools_md_reads_trimmed_content() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        std::fs::create_dir_all(tools_path().parent().unwrap()).unwrap();
        std::fs::write(tools_path(), "\n  Use safe tools first.  \n").unwrap();
        assert_eq!(load_tools_md().as_deref(), Some("Use safe tools first."));

        clear_data_dir();
    }

    #[test]
    fn load_agents_md_reads_trimmed_content() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        std::fs::create_dir_all(agents_path().parent().unwrap()).unwrap();
        std::fs::write(agents_path(), "\nLocal workspace instructions\n").unwrap();
        assert_eq!(
            load_agents_md().as_deref(),
            Some("Local workspace instructions")
        );

        clear_data_dir();
    }

    #[test]
    fn load_heartbeat_md_reads_trimmed_content() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        std::fs::write(dir.path().join("HEARTBEAT.md"), "\n# Heartbeat\n- ping\n").unwrap();
        assert_eq!(load_heartbeat_md().as_deref(), Some("# Heartbeat\n- ping"));

        clear_data_dir();
    }

    #[test]
    fn workspace_markdown_ignores_leading_html_comments() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        std::fs::create_dir_all(tools_path().parent().unwrap()).unwrap();
        std::fs::write(
            tools_path(),
            "<!-- comment -->\n\nUse read-only tools first.",
        )
        .unwrap();
        assert_eq!(
            load_tools_md().as_deref(),
            Some("Use read-only tools first.")
        );

        clear_data_dir();
    }

    #[test]
    fn workspace_markdown_comment_only_is_treated_as_empty() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        std::fs::write(dir.path().join("HEARTBEAT.md"), "<!-- guidance -->").unwrap();
        assert_eq!(load_heartbeat_md(), None);

        clear_data_dir();
    }

    #[test]
    fn load_soul_creates_default_when_missing() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let soul_file = soul_path();
        assert!(!soul_file.exists(), "SOUL.md should not exist yet");

        let content = load_soul();
        assert!(
            content.is_some(),
            "load_soul should return Some after seeding"
        );
        assert_eq!(content.as_deref(), Some(DEFAULT_SOUL));
        assert!(soul_file.exists(), "SOUL.md should be created on disk");

        let on_disk = std::fs::read_to_string(&soul_file).unwrap();
        assert_eq!(on_disk, DEFAULT_SOUL);

        clear_data_dir();
    }

    #[test]
    fn load_soul_does_not_overwrite_existing() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let custom = "You are a loyal companion who loves fetch.";
        std::fs::create_dir_all(soul_path().parent().unwrap()).unwrap();
        std::fs::write(soul_path(), custom).unwrap();

        let content = load_soul();
        assert_eq!(content.as_deref(), Some(custom));

        let on_disk = std::fs::read_to_string(soul_path()).unwrap();
        assert_eq!(on_disk, custom, "existing SOUL.md must not be overwritten");

        clear_data_dir();
    }

    #[test]
    fn load_soul_reseeds_after_deletion() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        // First call seeds the file.
        let _ = load_soul();
        let soul_file = soul_path();
        assert!(soul_file.exists());

        // Delete it.
        std::fs::remove_file(&soul_file).unwrap();
        assert!(!soul_file.exists());

        // Second call re-seeds.
        let content = load_soul();
        assert_eq!(content.as_deref(), Some(DEFAULT_SOUL));
        assert!(soul_file.exists());

        clear_data_dir();
    }

    #[test]
    fn sync_people_md_preserves_body_and_other_fields() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        // Seed identity (SOT for emoji/creature).
        let identity_path = data_dir().join("people/default/IDENTITY.md");
        std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        std::fs::write(
            &identity_path,
            "---\nname: default\nemoji: 🤖\ncreature: 助手\nvibe: test\n---\n\nPRIVATE BODY\n",
        )
        .unwrap();

        // Seed PEOPLE.md with a body and extra per-entry keys.
        let people_path = people_path();
        let seed = "---\nschema_version: 1\npeople:\n  - name: default\n    display_name: 默认\n    telegram_user_name: my_bot\n    custom_field: keep\n    emoji: old\n    creature: oldc\n---\n\nPUBLIC BODY\n";
        std::fs::write(&people_path, seed).unwrap();

        let before = std::fs::read_to_string(&people_path).unwrap();
        let before_body = split_yaml_frontmatter(&before).unwrap().body.to_string();

        sync_people_md_from_identities().unwrap();

        let after = std::fs::read_to_string(&people_path).unwrap();
        let after_split = split_yaml_frontmatter(&after).unwrap();
        assert_eq!(after_split.body, before_body, "PEOPLE.md body must be preserved");

        let yaml = serde_yaml::from_str::<serde_yaml::Value>(after_split.inner).unwrap();
        let people = yaml
            .get("people")
            .and_then(|v| v.as_sequence())
            .unwrap();
        let entry = people[0].as_mapping().unwrap();
        assert_eq!(
            entry
                .get(&serde_yaml::Value::String("telegram_user_name".to_string()))
                .and_then(|v| v.as_str()),
            Some("my_bot")
        );
        assert_eq!(
            entry
                .get(&serde_yaml::Value::String("custom_field".to_string()))
                .and_then(|v| v.as_str()),
            Some("keep")
        );
        assert_eq!(
            entry
                .get(&serde_yaml::Value::String("emoji".to_string()))
                .and_then(|v| v.as_str()),
            Some("🤖")
        );
        assert_eq!(
            entry
                .get(&serde_yaml::Value::String("creature".to_string()))
                .and_then(|v| v.as_str()),
            Some("助手")
        );

        clear_data_dir();
    }

    #[test]
    fn sync_people_md_removes_emoji_when_identity_clears_it() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        let identity_path = data_dir().join("people/default/IDENTITY.md");
        std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        std::fs::write(
            &identity_path,
            "---\nname: default\ncreature: 助手\n---\n\nPRIVATE BODY\n",
        )
        .unwrap();

        let people_path = people_path();
        std::fs::write(
            &people_path,
            "---\nschema_version: 1\npeople:\n  - name: default\n    emoji: old\n    creature: oldc\n---\n\nPUBLIC BODY\n",
        )
        .unwrap();

        sync_people_md_from_identities().unwrap();

        let after = std::fs::read_to_string(&people_path).unwrap();
        let split = split_yaml_frontmatter(&after).unwrap();
        let yaml = serde_yaml::from_str::<serde_yaml::Value>(split.inner).unwrap();
        let entry = yaml
            .get("people")
            .and_then(|v| v.as_sequence())
            .unwrap()[0]
            .as_mapping()
            .unwrap();

        assert!(
            entry.get(&serde_yaml::Value::String("emoji".to_string()))
                .is_none(),
            "emoji should be removed when not present in IDENTITY.md"
        );
        assert_eq!(
            entry
                .get(&serde_yaml::Value::String("creature".to_string()))
                .and_then(|v| v.as_str()),
            Some("助手")
        );

        clear_data_dir();
    }

    #[test]
    fn save_soul_none_prevents_reseed() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        // Auto-seed SOUL.md.
        let _ = load_soul();
        let soul_file = soul_path();
        assert!(soul_file.exists());

        // User explicitly clears the soul via settings.
        save_soul(None).expect("save_soul(None)");
        assert!(
            soul_file.exists(),
            "save_soul(None) should leave an empty file, not delete"
        );
        assert!(
            std::fs::read_to_string(&soul_file).unwrap().is_empty(),
            "file should be empty after clearing"
        );

        // load_soul must return None — NOT re-seed.
        let content = load_soul();
        assert_eq!(
            content, None,
            "load_soul must return None after explicit clear, not re-seed"
        );

        clear_data_dir();
    }

    #[test]
    fn save_soul_some_overwrites_default() {
        let _guard = DATA_DIR_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().expect("tempdir");
        set_data_dir(dir.path().to_path_buf());

        // Auto-seed.
        let _ = load_soul();

        // User writes custom soul.
        let custom = "You love fetch and belly rubs.";
        save_soul(Some(custom)).expect("save_soul");

        let content = load_soul();
        assert_eq!(content.as_deref(), Some(custom));

        let on_disk = std::fs::read_to_string(soul_path()).unwrap();
        assert_eq!(on_disk, custom);

        clear_data_dir();
    }
}
