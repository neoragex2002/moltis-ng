use crate::config::{DmScope, GroupScope, TelegramAccountConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgInboundKind {
    Dm,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgInboundMode {
    Dispatch,
    RecordOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgContent {
    pub text: String,
    pub has_attachments: bool,
    pub has_location: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgPrivateSource {
    pub account_handle: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
    pub peer: String,
    pub sender: Option<String>,
    pub addressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgInbound {
    pub kind: TgInboundKind,
    pub mode: TgInboundMode,
    pub body: TgContent,
    pub private_source: TgPrivateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgRoute {
    pub peer: String,
    pub sender: Option<String>,
    pub bucket_key: String,
    pub addressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgPrivateTarget {
    pub account_handle: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgReply {
    pub output: String,
    pub private_target: TgPrivateTarget,
}

pub fn resolve_tg_route(config: &TelegramAccountConfig, inbound: &TgInbound) -> TgRoute {
    let peer = inbound.private_source.peer.clone();
    let sender = inbound.private_source.sender.clone();
    let branch = inbound.private_source.thread_id.as_deref();
    let bucket_key = match inbound.kind {
        TgInboundKind::Dm => resolve_dm_bucket_key(
            &config.dm_scope,
            &inbound.private_source.account_handle,
            &peer,
        ),
        TgInboundKind::Group => resolve_group_bucket_key(
            &config.group_scope,
            &inbound.private_source.account_handle,
            &peer,
            sender.as_deref(),
            branch,
        ),
    };

    TgRoute {
        peer,
        sender,
        bucket_key,
        addressed: inbound.private_source.addressed,
    }
}

pub fn resolve_dm_bucket_key(dm_scope: &DmScope, account_handle: &str, peer: &str) -> String {
    match dm_scope {
        DmScope::Main => "dm:main".to_string(),
        DmScope::PerPeer => format!("dm:peer:{peer}"),
        DmScope::PerChannel => format!("dm:channel:telegram:peer:{peer}"),
        DmScope::PerAccount => format!("dm:account:{account_handle}:peer:{peer}"),
    }
}

pub fn resolve_group_bucket_key(
    group_scope: &GroupScope,
    account_handle: &str,
    peer: &str,
    sender: Option<&str>,
    branch: Option<&str>,
) -> String {
    let prefix = format!("group:account:{account_handle}:peer:{peer}");
    match group_scope {
        GroupScope::Group => prefix,
        GroupScope::PerSender => sender
            .map(|sender| format!("{prefix}:sender:{sender}"))
            .unwrap_or(prefix),
        GroupScope::PerBranch => branch
            .map(|branch| format!("{prefix}:branch:{branch}"))
            .unwrap_or(prefix),
        GroupScope::PerBranchSender => match (branch, sender) {
            (Some(branch), Some(sender)) => format!("{prefix}:branch:{branch}:sender:{sender}"),
            (Some(branch), None) => format!("{prefix}:branch:{branch}"),
            (None, Some(sender)) => format!("{prefix}:sender:{sender}"),
            (None, None) => prefix,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_bucket_key_follows_scope() {
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::Main, "telegram:a", "peer-1"),
            "dm:main"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerPeer, "telegram:a", "peer-1"),
            "dm:peer:peer-1"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerChannel, "telegram:a", "peer-1"),
            "dm:channel:telegram:peer:peer-1"
        );
        assert_eq!(
            resolve_dm_bucket_key(&DmScope::PerAccount, "telegram:a", "peer-1"),
            "dm:account:telegram:a:peer:peer-1"
        );
    }

    #[test]
    fn group_bucket_key_degrades_when_sender_or_branch_missing() {
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerSender,
                "telegram:a",
                "peer-1",
                None,
                Some("7"),
            ),
            "group:account:telegram:a:peer:peer-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranch,
                "telegram:a",
                "peer-1",
                Some("sender-1"),
                None,
            ),
            "group:account:telegram:a:peer:peer-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranchSender,
                "telegram:a",
                "peer-1",
                Some("sender-1"),
                None,
            ),
            "group:account:telegram:a:peer:peer-1:sender:sender-1"
        );
        assert_eq!(
            resolve_group_bucket_key(
                &GroupScope::PerBranchSender,
                "telegram:a",
                "peer-1",
                None,
                Some("7"),
            ),
            "group:account:telegram:a:peer:peer-1:branch:7"
        );
    }

    #[test]
    fn resolve_route_uses_configured_scope() {
        let config = TelegramAccountConfig {
            dm_scope: DmScope::PerAccount,
            group_scope: GroupScope::PerBranchSender,
            ..Default::default()
        };
        let inbound = TgInbound {
            kind: TgInboundKind::Group,
            mode: TgInboundMode::Dispatch,
            body: TgContent {
                text: "hello".into(),
                has_attachments: false,
                has_location: false,
            },
            private_source: TgPrivateSource {
                account_handle: "telegram:test".into(),
                chat_id: "-1001".into(),
                message_id: Some("99".into()),
                thread_id: Some("7".into()),
                peer: "-1001".into(),
                sender: Some("u-1".into()),
                addressed: true,
            },
        };
        let route = resolve_tg_route(&config, &inbound);
        assert_eq!(route.peer, "-1001");
        assert_eq!(route.sender.as_deref(), Some("u-1"));
        assert_eq!(
            route.bucket_key,
            "group:account:telegram:test:peer:-1001:branch:7:sender:u-1"
        );
        assert!(route.addressed);
    }
}
