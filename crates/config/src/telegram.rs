use {
    moltis_channels::gating::DmPolicy,
    secrecy::{ExposeSecret, Secret},
    serde::{Deserialize, Serialize},
    std::collections::HashMap,
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

/// Telegram channel-wide configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramChannelsConfig {
    #[serde(flatten)]
    pub accounts: HashMap<String, TelegramAccountConfig>,
}

impl Default for TelegramChannelsConfig {
    fn default() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }
}

/// Configuration for a single Telegram bot account.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct TelegramAccountConfig {
    /// Bot token from @BotFather.
    #[serde(serialize_with = "serialize_secret")]
    pub token: Secret<String>,

    /// DM access policy.
    pub dm_policy: DmPolicy,

    /// User/peer allowlist for DMs.
    pub allowlist: Vec<String>,

    /// Session bucketing mode for DM conversations.
    pub dm_scope: DmScope,

    /// Session bucketing mode for group conversations.
    pub group_scope: GroupScope,

    /// Whether line-start mentions dispatch the target bot in groups.
    pub group_line_start_mention_dispatch: bool,

    /// Whether reply-to-bot dispatches the target bot in groups.
    pub group_reply_to_dispatch: bool,

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

    /// Default model ID for this bot's sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Provider name associated with `model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Enable OTP self-approval for non-allowlisted DM users.
    pub otp_self_approval: bool,

    /// Cooldown in seconds after 3 failed OTP attempts.
    pub otp_cooldown_secs: u64,

    /// Optional agent ID bound to this Telegram bot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Telegram bot user id from `getMe.id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_user_id: Option<u64>,

    /// Telegram bot username from `getMe.username`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_user_name: Option<String>,

    /// Telegram bot display name from `getMe.first_name/last_name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chan_nickname: Option<String>,
}

/// Safe, non-secret snapshot of Telegram bot identity + group bus config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramBusAccountSnapshot {
    pub account_handle: String,
    pub agent_id: Option<String>,
    pub chan_user_id: Option<u64>,
    pub chan_user_name: Option<String>,
    pub chan_nickname: Option<String>,
    pub dm_scope: DmScope,
    pub group_scope: GroupScope,
}

/// Read-only Telegram identity link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelegramIdentityLink {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_user_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_display_name: Option<String>,
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
            allowlist: Vec::new(),
            dm_scope: DmScope::default(),
            group_scope: GroupScope::default(),
            group_line_start_mention_dispatch: true,
            group_reply_to_dispatch: true,
            stream_mode: StreamMode::default(),
            edit_throttle_ms: 300,
            outbound_max_attempts: 3,
            outbound_retry_base_delay_ms: 500,
            outbound_retry_max_delay_ms: 5000,
            model: None,
            model_provider: None,
            otp_self_approval: true,
            otp_cooldown_secs: 300,
            agent_id: None,
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
    fn telegram_account_config_defaults_match_hard_cut_shape() {
        let cfg = TelegramAccountConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert!(cfg.group_line_start_mention_dispatch);
        assert!(cfg.group_reply_to_dispatch);
        assert_eq!(cfg.dm_scope, DmScope::Main);
        assert_eq!(cfg.group_scope, GroupScope::Group);
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.outbound_max_attempts, 3);
    }

    #[test]
    fn telegram_account_config_rejects_legacy_fields() {
        let json = r#"
        {
            "token": "123:ABC",
            "mention_mode": "mention"
        }
        "#;
        let err = serde_json::from_str::<TelegramAccountConfig>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field `mention_mode`"),
            "unexpected error: {err}"
        );
    }
}
