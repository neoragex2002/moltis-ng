use {
    moltis_channels::gating::{DmPolicy, MentionMode},
    secrecy::{ExposeSecret, Secret},
    serde::{Deserialize, Serialize},
};

/// How strict the group relay parser should be when interpreting bot@bot mentions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelayStrictness {
    /// Conservative: only relay when the mention looks like an explicit directive.
    #[default]
    Strict,
    /// More permissive: relay on weaker signals (still skips code blocks/quotes).
    Loose,
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

    /// Mention activation mode for groups.
    pub mention_mode: MentionMode,

    /// User/peer allowlist for DMs.
    pub allowlist: Vec<String>,

    /// Whether group relay can trigger chained bot-to-bot delegations.
    pub relay_chain_enabled: bool,

    /// Maximum number of relay hops for a single chain.
    ///
    /// Default: 3.
    pub relay_hop_limit: u8,

    /// Relay parsing strictness.
    pub relay_strictness: RelayStrictness,

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

    /// Optional persona ID bound to this Telegram bot (named persona directory).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona_id: Option<String>,

    /// Telegram bot user id from `getMe.id` (stable primary key).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_user_id: Option<u64>,

    /// Telegram bot username from `getMe.username` (without `@`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_user_name: Option<String>,

    /// Telegram bot display name from `getMe.first_name/last_name` (human-friendly).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_nickname: Option<String>,
}

/// Safe, non-secret snapshot of Telegram bot identity + group bus config.
///
/// Used by the gateway to implement group mirror/relay behavior without
/// depending on secret config fields (token).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramBusAccountSnapshot {
    pub account_id: String,
    pub bot_username: Option<String>,
    pub relay_chain_enabled: bool,
    pub relay_hop_limit: u8,
    pub relay_strictness: RelayStrictness,
}

impl std::fmt::Debug for TelegramAccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramAccountConfig")
            .field("token", &"[REDACTED]")
            .field("dm_policy", &self.dm_policy)
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
            mention_mode: MentionMode::default(),
            allowlist: Vec::new(),
            relay_chain_enabled: true,
            relay_hop_limit: 3,
            relay_strictness: RelayStrictness::default(),
            stream_mode: StreamMode::default(),
            edit_throttle_ms: 300,
            model: None,
            model_provider: None,
            otp_self_approval: true,
            otp_cooldown_secs: 300,
            persona_id: None,
            chan_user_id: None,
            chan_user_name: None,
            chan_nickname: None,
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = TelegramAccountConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 300);
        assert_eq!(cfg.relay_chain_enabled, true);
        assert_eq!(cfg.relay_hop_limit, 3);
        assert_eq!(cfg.relay_strictness, RelayStrictness::Strict);
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
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
    }

    #[test]
    fn deserialize_mention_mode_none_is_accepted_as_mention() {
        let json = r#"{
            "token": "123:ABC",
            "mention_mode": "none"
        }"#;
        let cfg: TelegramAccountConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
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
