#[must_use]
pub fn floor_char_boundary(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        text.len()
    } else {
        text.floor_char_boundary(max_bytes)
    }
}

#[must_use]
pub fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    &text[..floor_char_boundary(text, max_bytes)]
}

#[must_use]
pub fn truncate_utf8_with_suffix(text: &str, max_bytes: usize, suffix: &str) -> String {
    if text.len() <= max_bytes {
        text.to_string()
    } else {
        let mut truncated = truncate_utf8(text, max_bytes).to_string();
        truncated.push_str(suffix);
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::{floor_char_boundary, truncate_utf8, truncate_utf8_with_suffix};

    #[test]
    fn floor_char_boundary_clamps_ascii_and_unicode() {
        assert_eq!(floor_char_boundary("hello", 10), 5);
        assert_eq!(floor_char_boundary("héllo", 2), 1);
        assert_eq!(floor_char_boundary("a😀b", 3), 1);
    }

    #[test]
    fn truncate_utf8_handles_chinese_and_emoji() {
        assert_eq!(truncate_utf8("你好世界", 5), "你");
        assert_eq!(truncate_utf8("a😀b", 5), "a😀");
    }

    #[test]
    fn truncate_utf8_with_suffix_only_appends_when_truncated() {
        assert_eq!(truncate_utf8_with_suffix("hello", 10, "..."), "hello");
        assert_eq!(truncate_utf8_with_suffix("你好世界", 5, "..."), "你...");
        assert_eq!(truncate_utf8_with_suffix("a😀b", 5, "..."), "a😀...");
    }
}
