use moltis_telegram::config::TelegramBusAccountSnapshot;

pub(crate) fn resolve_telegram_bot_username<'a>(
    snapshots: &'a [TelegramBusAccountSnapshot],
    account_handle: &str,
) -> Option<&'a str> {
    snapshots
        .iter()
        .find(|s| s.account_handle == account_handle)
        .and_then(|s| s.chan_user_name.as_deref())
}

pub(crate) fn format_telegram_session_label(
    account_handle: &str,
    bot_username: Option<&str>,
    chat_id: &str,
) -> String {
    let bot_label = bot_username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| format!("@{}", u.trim_start_matches('@')))
        .unwrap_or_else(|| {
            account_handle
                .strip_prefix("telegram:")
                .unwrap_or(account_handle)
                .to_string()
        });
    let kind = match chat_id.parse::<i64>() {
        Ok(id) if id < 0 => "grp",
        Ok(_) => "dm",
        Err(_) => "chat",
    };
    format!("TG {bot_label} · {kind}:{chat_id}")
}
