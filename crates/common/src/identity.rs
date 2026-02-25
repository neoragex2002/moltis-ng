#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountHandleParts<'a> {
    pub channel: &'a str,
    pub chan_user_id: &'a str,
}

pub fn parse_account_handle(handle: &str) -> Option<AccountHandleParts<'_>> {
    let (channel, chan_user_id) = handle.split_once(':')?;
    let channel = channel.trim();
    let chan_user_id = chan_user_id.trim();
    if channel.is_empty() || chan_user_id.is_empty() {
        return None;
    }
    Some(AccountHandleParts {
        channel,
        chan_user_id,
    })
}

pub fn chan_user_id_from_account_handle<'a>(
    handle: &'a str,
    expected_channel: Option<&str>,
) -> Option<&'a str> {
    let parts = parse_account_handle(handle)?;
    if let Some(expected) = expected_channel {
        if parts.channel != expected {
            return None;
        }
    }
    Some(parts.chan_user_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKeyParts<'a> {
    pub channel: &'a str,
    pub chan_user_id: &'a str,
    pub chat_id: &'a str,
    pub thread_id: Option<&'a str>,
}

pub fn format_session_key(
    channel: &str,
    chan_user_id: &str,
    chat_id: &str,
    thread_id: Option<&str>,
) -> String {
    match thread_id {
        Some(tid) if !tid.trim().is_empty() => {
            format!(
                "{}:{}:{}:{}",
                channel.trim(),
                chan_user_id.trim(),
                chat_id.trim(),
                tid.trim()
            )
        },
        _ => format!(
            "{}:{}:{}",
            channel.trim(),
            chan_user_id.trim(),
            chat_id.trim()
        ),
    }
}

pub fn parse_session_key(key: &str) -> Option<SessionKeyParts<'_>> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.split(':');
    let channel = parts.next()?;
    let chan_user_id = parts.next()?;
    let chat_id = parts.next()?;
    let thread_id = parts.next();
    if parts.next().is_some() {
        return None;
    }
    if channel.is_empty() || chan_user_id.is_empty() || chat_id.is_empty() {
        return None;
    }
    let thread_id = thread_id.filter(|s| !s.trim().is_empty());
    Some(SessionKeyParts {
        channel,
        chan_user_id,
        chat_id,
        thread_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_handle() {
        let p = parse_account_handle("telegram:123").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.chan_user_id, "123");
        assert_eq!(
            chan_user_id_from_account_handle("telegram:123", Some("telegram")),
            Some("123")
        );
        assert_eq!(
            chan_user_id_from_account_handle("telegram:123", Some("discord")),
            None
        );
    }

    #[test]
    fn parses_session_key_three_or_four_parts() {
        let p = parse_session_key("telegram:123:-100").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.chan_user_id, "123");
        assert_eq!(p.chat_id, "-100");
        assert_eq!(p.thread_id, None);

        let p = parse_session_key("telegram:123:-100:12").unwrap();
        assert_eq!(p.thread_id, Some("12"));
        assert_eq!(
            format_session_key("telegram", "123", "-100", Some("12")),
            "telegram:123:-100:12"
        );
    }

    #[test]
    fn rejects_invalid_session_key() {
        assert!(parse_session_key("").is_none());
        assert!(parse_session_key("a:b").is_none());
        assert!(parse_session_key("a:b:c:d:e").is_none());
    }
}
