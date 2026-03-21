#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChanAccountKeyParts<'a> {
    pub channel: &'a str,
    pub chan_user_id: &'a str,
}

pub fn parse_chan_account_key(key: &str) -> Option<ChanAccountKeyParts<'_>> {
    let (channel, chan_user_id) = key.split_once(':')?;
    let channel = channel.trim();
    let chan_user_id = chan_user_id.trim();
    if channel.is_empty() || chan_user_id.is_empty() {
        return None;
    }
    Some(ChanAccountKeyParts {
        channel,
        chan_user_id,
    })
}

pub fn chan_user_id_from_chan_account_key<'a>(
    key: &'a str,
    expected_channel: Option<&str>,
) -> Option<&'a str> {
    let parts = parse_chan_account_key(key)?;
    if let Some(expected) = expected_channel {
        if parts.channel != expected {
            return None;
        }
    }
    Some(parts.chan_user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_handle() {
        let p = parse_chan_account_key("telegram:123").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.chan_user_id, "123");
        assert_eq!(
            chan_user_id_from_chan_account_key("telegram:123", Some("telegram")),
            Some("123")
        );
        assert_eq!(
            chan_user_id_from_chan_account_key("telegram:123", Some("discord")),
            None
        );
    }
}
