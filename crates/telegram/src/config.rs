use {
    moltis_channels::gating::{DmPolicy, GroupPolicy, MentionMode},
    secrecy::{ExposeSecret, Secret},
    serde::{Deserialize, Serialize},
};

/// Whether inbound Group/Supergroup messages should be written into the bot's
/// session history (for later context), independent of whether the bot replies.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupIngestMode {
    /// Only ingest messages that are addressed to this bot (the current behavior).
    #[default]
    MentionedOnly,
    /// Ingest every group message the bot receives (listen/sidecar mode).
    AllMessages,
    /// Do not ingest group messages.
    None,
}

/// How streaming responses are delivered.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    /// Edit a placeholder message in place as tokens arrive.
    #[default]
    EditInPlace,
    /// No streaming — send the final response as a single message.
    Off,
}

/// Configuration for a single Telegram bot account.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramAccountConfig {
    /// Bot token from @BotFather.
    #[serde(serialize_with = "serialize_secret")]
    pub token: Secret<String>,

    /// DM access policy.
    pub dm_policy: DmPolicy,

    /// Group access policy.
    pub group_policy: GroupPolicy,

    /// Mention activation mode for groups.
    pub mention_mode: MentionMode,

    /// Whether to ingest group messages into session history.
    ///
    /// Scope: **Group/Supergroup only**. Channel semantics are handled separately.
    pub group_ingest_mode: GroupIngestMode,

    /// Mirror this bot's **successful group replies** into other Telegram bot
    /// sessions on the Moltis side (ingest-only), so they can "know" what this
    /// bot said even though Telegram does not deliver bot-to-bot updates.
    ///
    /// Scope: **Group/Supergroup only** (V1 gating uses `chat_id < 0`).
    /// Default: false.
    pub group_outbound_mirror_enabled: bool,

    /// User/peer allowlist for DMs.
    pub allowlist: Vec<String>,

    /// Group/chat ID allowlist.
    pub group_allowlist: Vec<String>,

    /// How streaming responses are delivered.
    pub stream_mode: StreamMode,

    /// Minimum interval between edit-in-place updates (ms).
    pub edit_throttle_ms: u64,

    /// Default model ID for this bot's sessions (e.g. "claude-sonnet-4-5-20250929").
    /// When set, channel messages use this model instead of the first registered provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Provider name associated with `model` (e.g. "anthropic").
    /// Stored alongside the model ID for display and debugging; the registry
    /// resolves the provider from the model ID at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Enable OTP self-approval for non-allowlisted DM users (default: true).
    pub otp_self_approval: bool,

    /// Cooldown in seconds after 3 failed OTP attempts (default: 300).
    pub otp_cooldown_secs: u64,

    /// Send bot responses as Telegram replies to the user's message.
    /// When false (default), responses are sent as standalone messages.
    pub reply_to_message: bool,
}

impl TelegramAccountConfig {
    /// Normalize a config for runtime behavior, preserving backward compatibility.
    ///
    /// Rules (frozen by issue docs):
    /// - `mention_mode=always` implies `group_ingest_mode=all_messages`
    /// - `mention_mode=none` forbids `group_ingest_mode=mentioned_only` (clamp to `none`)
    pub fn normalize_in_place(&mut self) {
        // Keep reply/ingest combinations coherent.
        match self.mention_mode {
            MentionMode::Always => {
                self.group_ingest_mode = GroupIngestMode::AllMessages;
            },
            MentionMode::None => {
                if self.group_ingest_mode == GroupIngestMode::MentionedOnly {
                    self.group_ingest_mode = GroupIngestMode::None;
                }
            },
            MentionMode::Mention => {},
        }
    }
}

impl std::fmt::Debug for TelegramAccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramAccountConfig")
            .field("token", &"[REDACTED]")
            .field("dm_policy", &self.dm_policy)
            .field("group_policy", &self.group_policy)
            .finish_non_exhaustive()
    }
}

fn serialize_secret<S: serde::Serializer>(
    secret: &Secret<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

impl Default for TelegramAccountConfig {
    fn default() -> Self {
        Self {
            token: Secret::new(String::new()),
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            mention_mode: MentionMode::default(),
            group_ingest_mode: GroupIngestMode::default(),
            group_outbound_mirror_enabled: false,
            allowlist: Vec::new(),
            group_allowlist: Vec::new(),
            stream_mode: StreamMode::default(),
            edit_throttle_ms: 300,
            model: None,
            model_provider: None,
            otp_self_approval: true,
            otp_cooldown_secs: 300,
            reply_to_message: false,
        }
    }
}

/// Safe, non-secret snapshot of a Telegram account's mirror-related config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramMirrorConfigSnapshot {
    pub group_outbound_mirror_enabled: bool,
    pub group_allowlist: Vec<String>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = TelegramAccountConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.group_policy, GroupPolicy::Open);
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
        assert_eq!(cfg.group_ingest_mode, GroupIngestMode::MentionedOnly);
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 300);
    }

    #[test]
    fn deserialize_from_json() {
        let json = r#"{
            "token": "123:ABC",
            "dm_policy": "allowlist",
            "stream_mode": "off",
            "allowlist": ["user1", "user2"]
        }"#;
        let cfg: TelegramAccountConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.token.expose_secret(), "123:ABC");
        assert_eq!(cfg.dm_policy, DmPolicy::Allowlist);
        assert_eq!(cfg.stream_mode, StreamMode::Off);
        assert_eq!(cfg.allowlist, vec!["user1", "user2"]);
        // defaults for unspecified fields
        assert_eq!(cfg.group_policy, GroupPolicy::Open);
        assert_eq!(cfg.group_ingest_mode, GroupIngestMode::MentionedOnly);
    }

    #[test]
    fn normalize_clamps_incompatible_group_ingest_modes() {
        // Backward-compat: old configs can set `mention_mode=none` without any
        // group ingest setting; default mentioned_only would be invalid.
        let mut cfg = TelegramAccountConfig::default();
        cfg.mention_mode = MentionMode::None;
        cfg.group_ingest_mode = GroupIngestMode::MentionedOnly;
        cfg.normalize_in_place();
        assert_eq!(cfg.group_ingest_mode, GroupIngestMode::None);

        // Coherence: `always` implies `all_messages`.
        let mut cfg = TelegramAccountConfig::default();
        cfg.mention_mode = MentionMode::Always;
        cfg.group_ingest_mode = GroupIngestMode::None;
        cfg.normalize_in_place();
        assert_eq!(cfg.group_ingest_mode, GroupIngestMode::AllMessages);
    }

    #[test]
    fn serialize_roundtrip() {
        let cfg = TelegramAccountConfig {
            token: Secret::new("tok".into()),
            dm_policy: DmPolicy::Disabled,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let cfg2: TelegramAccountConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg2.dm_policy, DmPolicy::Disabled);
        assert_eq!(cfg2.token.expose_secret(), "tok");
    }
}
