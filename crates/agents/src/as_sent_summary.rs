use serde_json::Value;
use sha2::{Digest, Sha256};

pub const DEFAULT_MAX_PREVIEW_BYTES: usize = 4096;
pub const DEFAULT_MAX_LIST_ITEMS: usize = 8;

#[must_use]
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let bytes = hasher.finalize();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[must_use]
pub fn truncate_utf8_to_bytes(input: &str, max_bytes: usize) -> (String, bool) {
    if input.len() <= max_bytes {
        return (input.to_string(), false);
    }

    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }

    (input[..end].to_string(), true)
}

#[must_use]
pub fn text_preview_value(input: &str) -> Value {
    text_preview_value_with_limit(input, DEFAULT_MAX_PREVIEW_BYTES)
}

#[must_use]
pub fn text_preview_value_with_limit(input: &str, max_bytes: usize) -> Value {
    let (preview, truncated) = truncate_utf8_to_bytes(input, max_bytes);
    serde_json::json!({
        "sha256": sha256_hex(input),
        "bytes": input.len(),
        "truncated": truncated,
        "preview": preview,
    })
}
