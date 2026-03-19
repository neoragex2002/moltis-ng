use {
    moltis_channels::gating::{DmPolicy, MentionMode},
    secrecy::{ExposeSecret, Secret},
    serde::{Deserialize, Serialize},
};

/// How `dm` messages are bucketed into logical sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DmScope {
    /// All DM messages for the same agent share one bucket.
    #[default]
    Main,
    /// Bucket by logical peer.
    PerPeer,
    /// Bucket by logical peer + channel.
    PerChannel,
    /// Bucket by logical peer + account.
    PerAccount,
}

/// How `group` messages are bucketed into logical sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupScope {
    /// Bucket by shared group peer.
    #[default]
    Group,
    /// Bucket by shared group peer + sender.
    PerSender,
    /// Bucket by shared group peer + branch.
    PerBranch,
    /// Bucket by shared group peer + branch + sender.
    PerBranchSender,
}

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

/// How Telegram *group* inbound/mirror/relay messages are formatted when written into
/// the session transcript (LLM-visible `content`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupSessionTranscriptFormat {
    /// Legacy behavior (self-mention stripping / whitespace normalization / mirror+relay prefixes).
    #[default]
    Legacy,
    /// TG-GST v1 (Telegram Group Session Transcript v1).
    TgGstV1,
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

    /// Session bucketing mode for DM conversations.
    pub dm_scope: DmScope,

    /// Session bucketing mode for group conversations.
    pub group_scope: GroupScope,

    /// Whether group relay can trigger chained bot-to-bot delegations.
    pub relay_chain_enabled: bool,

    /// Maximum number of relay hops for a single chain.
    ///
    /// Default: 3.
    pub relay_hop_limit: u32,

    /// Maximum number of relay injections allowed in a single relay epoch.
    ///
    /// Short-term definition of epoch: `relayChainId`.
    ///
    /// Default: 128.
    pub epoch_relay_budget: u32,

    /// Relay parsing strictness.
    pub relay_strictness: RelayStrictness,

    /// Group session transcript format for LLM-visible `content`.
    ///
    /// Default: `legacy`.
    pub group_session_transcript_format: GroupSessionTranscriptFormat,

    /// How streaming responses are delivered.
    pub stream_mode: StreamMode,

    /// Minimum interval between edit-in-place updates (ms).
    pub edit_throttle_ms: u64,

    /// Maximum number of outbound attempts for retryable Telegram text delivery.
    pub outbound_max_attempts: u32,

    /// Base retry backoff for retryable Telegram text delivery (ms).
    pub outbound_retry_base_delay_ms: u64,

    /// Maximum retry backoff for retryable Telegram text delivery (ms).
    pub outbound_retry_max_delay_ms: u64,

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
    pub account_handle: String,
    pub chan_user_name: Option<String>,
    pub dm_scope: DmScope,
    pub group_scope: GroupScope,
    pub relay_chain_enabled: bool,
    pub relay_hop_limit: u32,
    pub epoch_relay_budget: u32,
    pub relay_strictness: RelayStrictness,
    pub group_session_transcript_format: GroupSessionTranscriptFormat,
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
            dm_scope: DmScope::default(),
            group_scope: GroupScope::default(),
            relay_chain_enabled: true,
            relay_hop_limit: 3,
            epoch_relay_budget: 128,
            relay_strictness: RelayStrictness::default(),
            group_session_transcript_format: GroupSessionTranscriptFormat::default(),
            stream_mode: StreamMode::default(),
            edit_throttle_ms: 300,
            outbound_max_attempts: 3,
            outbound_retry_base_delay_ms: 500,
            outbound_retry_max_delay_ms: 5000,
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
        assert_eq!(cfg.dm_scope, DmScope::Main);
        assert_eq!(cfg.group_scope, GroupScope::Group);
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 300);
        assert_eq!(cfg.outbound_max_attempts, 3);
        assert_eq!(cfg.outbound_retry_base_delay_ms, 500);
        assert_eq!(cfg.outbound_retry_max_delay_ms, 5000);
        assert_eq!(cfg.relay_chain_enabled, true);
        assert_eq!(cfg.relay_hop_limit, 3);
        assert_eq!(cfg.epoch_relay_budget, 128);
        assert_eq!(cfg.relay_strictness, RelayStrictness::Strict);
        assert_eq!(
            cfg.group_session_transcript_format,
            GroupSessionTranscriptFormat::Legacy
        );
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
        assert_eq!(cfg.outbound_max_attempts, 3);
        assert_eq!(cfg.outbound_retry_base_delay_ms, 500);
        assert_eq!(cfg.outbound_retry_max_delay_ms, 5000);
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
