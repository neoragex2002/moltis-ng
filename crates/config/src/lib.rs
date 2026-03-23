//! Configuration loading, validation, env substitution, and legacy migration.
//!
//! Config files: `moltis.toml`, `moltis.yaml`, or `moltis.json`
//! Searched in `./` then `~/.moltis/config/`.
//!
//! Supports `${ENV_VAR}` substitution in all string values.

pub mod env_subst;
pub mod loader;
pub mod migrate;
pub mod prompt_subst;
pub mod schema;
pub mod template;
pub mod validate;

pub use {
    loader::{
        DEFAULT_SOUL, agents_dir, agents_path, apply_env_overrides, clear_config_dir,
        clear_data_dir, config_dir, data_dir, discover_and_load, ensure_default_agent_seeded,
        ensure_people_md_seeded, find_or_default_config_path, find_user_global_config_file,
        heartbeat_path, identity_path, is_valid_agent_id, load_agent_agents_md,
        load_agent_identity, load_agent_identity_md_raw, load_agent_soul, load_agent_tools_md,
        load_agents_md, load_heartbeat_md, load_identity, load_identity_md_raw, load_soul,
        load_tools_md, load_user, people_path, project_local_dir, save_config, save_identity,
        save_raw_config, save_soul, save_user, set_config_dir, set_data_dir, soul_path,
        sync_people_md_from_identities, tools_path, update_config, user_global_config_dir,
        user_global_config_dir_if_different, user_path,
    },
    schema::{
        AgentIdentity, AuthConfig, ChatConfig, GeoLocation, MessageQueueMode, MoltisConfig,
        ResolvedIdentity, Timezone, UserProfile, VoiceConfig, VoiceElevenLabsConfig,
        VoiceOpenAiConfig, VoiceSttConfig, VoiceSttProvider, VoiceTtsConfig, VoiceWhisperConfig,
    },
    validate::{Diagnostic, Severity, ValidationResult},
};
