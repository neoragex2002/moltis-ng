//! Heartbeat logic: token stripping, empty-content detection, active-hours check.

use anyhow::{Result, bail};
use chrono::{NaiveTime, Timelike, Utc};

/// The sentinel token an LLM returns when nothing noteworthy is happening.
pub const HEARTBEAT_OK: &str = "HEARTBEAT_OK";

/// Result of stripping the `HEARTBEAT_OK` token from an LLM reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripResult {
    /// Whether the reply should be suppressed (not delivered to the user).
    pub should_skip: bool,
    /// The remaining text after stripping.
    pub text: String,
    /// Whether the token was found and removed.
    pub did_strip: bool,
}

/// How aggressively to strip the heartbeat token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripMode {
    /// Only strip if the entire reply is the token (possibly wrapped in bold).
    Exact,
    /// Strip the token from edges and check what remains.
    Trim,
}

/// Strip `HEARTBEAT_OK` from `text`, handling common LLM formatting wrappers
/// like `**HEARTBEAT_OK**` and `<b>HEARTBEAT_OK</b>`.
///
/// Returns a [`StripResult`] indicating whether the reply should be suppressed.
pub fn strip_heartbeat_token(text: &str, mode: StripMode) -> StripResult {
    let trimmed = text.trim();

    // Unwrap common bold wrappers.
    let unwrapped = unwrap_bold(trimmed);

    if unwrapped == HEARTBEAT_OK {
        return StripResult {
            should_skip: true,
            text: String::new(),
            did_strip: true,
        };
    }

    if mode == StripMode::Exact {
        return StripResult {
            should_skip: false,
            text: trimmed.to_string(),
            did_strip: false,
        };
    }

    // Trim mode: remove the token from edges.
    let mut result = trimmed.to_string();
    let mut did_strip = false;

    let patterns = [
        HEARTBEAT_OK.to_string(),
        format!("**{HEARTBEAT_OK}**"),
        format!("<b>{HEARTBEAT_OK}</b>"),
    ];
    for pattern in &patterns {
        if result.contains(pattern.as_str()) {
            result = result.replace(pattern.as_str(), "");
            did_strip = true;
        }
    }

    let result = result.trim().to_string();
    let should_skip = result.is_empty();

    StripResult {
        should_skip,
        text: result,
        did_strip,
    }
}

/// Returns `true` if a HEARTBEAT.md file's content is effectively empty
/// (only headers, blank lines, and empty list items).
pub fn is_heartbeat_content_empty(content: &str) -> bool {
    content.lines().all(|line| {
        let trimmed = line.trim();
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed == "-"
            || trimmed == "*"
            || trimmed == "- "
            || trimmed == "* "
    })
}

/// Check whether the current time falls within the active hours window.
///
/// Handles overnight windows (e.g. start=22:00, end=06:00).
/// Strict active-hours check.
///
/// Contract:
/// - Invalid start/end/timezone -> Err (caller must reject config, not silently "always active")
pub fn is_within_active_hours(start: &str, end: &str, timezone: &str) -> Result<bool> {
    let start_time = parse_hhmm_strict(start)?;
    if timezone.trim().is_empty() {
        bail!("timezone is required");
    }

    // "24:00" means end-of-day.
    let end_minutes = if end == "24:00" {
        24 * 60
    } else {
        let end_time = parse_hhmm_strict(end)?;
        end_time.hour() as u32 * 60 + end_time.minute() as u32
    };
    let start_minutes = start_time.hour() as u32 * 60 + start_time.minute() as u32;

    let now_minutes = current_minutes_strict(timezone)?;

    if start_minutes <= end_minutes {
        // Normal window: 08:00–24:00
        Ok(now_minutes >= start_minutes && now_minutes < end_minutes)
    } else {
        // Overnight window: 22:00–06:00
        Ok(now_minutes >= start_minutes || now_minutes < end_minutes)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn unwrap_bold(s: &str) -> &str {
    // **HEARTBEAT_OK**
    if let Some(inner) = s.strip_prefix("**").and_then(|s| s.strip_suffix("**")) {
        return inner;
    }
    // <b>HEARTBEAT_OK</b>
    if let Some(inner) = s.strip_prefix("<b>").and_then(|s| s.strip_suffix("</b>")) {
        return inner;
    }
    s
}

fn parse_hhmm_strict(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").map_err(|_| anyhow::anyhow!("invalid time: {s}"))
}

fn current_minutes_strict(timezone: &str) -> Result<u32> {
    let tz: chrono_tz::Tz = timezone
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown timezone: {timezone}"))?;
    let dt = Utc::now().with_timezone(&tz);
    Ok(dt.hour() * 60 + dt.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_heartbeat_token ────────────────────────────────────────────

    #[test]
    fn strip_exact_heartbeat_ok() {
        let r = strip_heartbeat_token("HEARTBEAT_OK", StripMode::Exact);
        assert!(r.should_skip);
        assert!(r.did_strip);
        assert!(r.text.is_empty());
    }

    #[test]
    fn strip_bold_wrapped() {
        let r = strip_heartbeat_token("**HEARTBEAT_OK**", StripMode::Exact);
        assert!(r.should_skip);
        assert!(r.did_strip);
    }

    #[test]
    fn strip_html_bold_wrapped() {
        let r = strip_heartbeat_token("<b>HEARTBEAT_OK</b>", StripMode::Exact);
        assert!(r.should_skip);
        assert!(r.did_strip);
    }

    #[test]
    fn strip_with_whitespace() {
        let r = strip_heartbeat_token("  HEARTBEAT_OK  \n", StripMode::Exact);
        assert!(r.should_skip);
        assert!(r.did_strip);
    }

    #[test]
    fn strip_exact_with_extra_text() {
        let r = strip_heartbeat_token("HEARTBEAT_OK but also check email", StripMode::Exact);
        assert!(!r.should_skip);
        assert!(!r.did_strip);
    }

    #[test]
    fn strip_trim_removes_token() {
        let r = strip_heartbeat_token("HEARTBEAT_OK\nYou have a meeting at 3pm", StripMode::Trim);
        assert!(!r.should_skip);
        assert!(r.did_strip);
        assert!(r.text.contains("meeting"));
        assert!(!r.text.contains("HEARTBEAT_OK"));
    }

    #[test]
    fn strip_trim_only_token() {
        let r = strip_heartbeat_token("**HEARTBEAT_OK**\n", StripMode::Trim);
        assert!(r.should_skip);
        assert!(r.did_strip);
    }

    // ── is_heartbeat_content_empty ───────────────────────────────────────

    #[test]
    fn empty_content() {
        assert!(is_heartbeat_content_empty(""));
        assert!(is_heartbeat_content_empty("  \n\n  "));
    }

    #[test]
    fn headers_only() {
        assert!(is_heartbeat_content_empty("# Heartbeat\n## Inbox\n- \n"));
    }

    #[test]
    fn has_content() {
        assert!(!is_heartbeat_content_empty(
            "# Heartbeat\n- Check email from Bob"
        ));
    }

    // ── is_within_active_hours ───────────────────────────────────────────

    #[test]
    fn invalid_time_rejects() {
        assert!(is_within_active_hours("invalid", "24:00", "UTC").is_err());
    }

    #[test]
    fn active_hours_start_24_00_is_rejected() {
        assert!(is_within_active_hours("24:00", "24:00", "UTC").is_err());
    }

    #[test]
    fn local_timezone_alias_is_rejected() {
        assert!(is_within_active_hours("08:00", "24:00", "local").is_err());
    }

    #[test]
    fn active_hours_normal_window() {
        // We can't assert exact behavior without controlling time,
        // but we can verify it doesn't panic.
        let _ = is_within_active_hours("08:00", "24:00", "Asia/Shanghai").unwrap();
        let _ = is_within_active_hours("09:00", "17:00", "UTC").unwrap();
    }

    #[test]
    fn active_hours_overnight_window() {
        let _ = is_within_active_hours("22:00", "06:00", "Asia/Shanghai").unwrap();
    }
}
