/// Generate a trigger id suitable for correlating:
/// queueing → replay/merge → run → delivery/failure.
///
/// Format: `trg_<ULID>`
pub fn new_trigger_id() -> String {
    format!("trg_{}", new_ulid())
}

/// Generate a ULID string (26 Crockford Base32 chars).
///
/// This implementation avoids introducing a new dependency (offline builds).
fn new_ulid() -> String {
    // ULID layout: 48-bit timestamp (ms) + 80-bit randomness.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let ts = (now_ms & 0xFFFF_FFFF_FFFF) as u64; // 48 bits

    let mut bytes = [0u8; 16];
    bytes[0] = ((ts >> 40) & 0xFF) as u8;
    bytes[1] = ((ts >> 32) & 0xFF) as u8;
    bytes[2] = ((ts >> 24) & 0xFF) as u8;
    bytes[3] = ((ts >> 16) & 0xFF) as u8;
    bytes[4] = ((ts >> 8) & 0xFF) as u8;
    bytes[5] = (ts & 0xFF) as u8;

    let random: [u8; 10] = rand::random();
    bytes[6..].copy_from_slice(&random);

    // Crockford Base32 alphabet for ULID.
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    let value = u128::from_be_bytes(bytes);
    let mut out = [0u8; 26];
    for i in 0..26 {
        let shift = 125u32.saturating_sub((i as u32) * 5);
        let idx = ((value >> shift) & 0x1F) as usize;
        out[i] = CROCKFORD[idx];
    }

    // Safe: alphabet is ASCII.
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_id_has_expected_prefix_and_length() {
        let id = new_trigger_id();
        assert!(id.starts_with("trg_"));
        let ulid = id.trim_start_matches("trg_");
        assert_eq!(ulid.len(), 26);
        assert!(
            ulid.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
    }
}
