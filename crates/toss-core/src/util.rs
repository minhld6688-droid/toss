//! Small helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current unix time in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix seconds of a `SystemTime` (0 if before the epoch).
pub fn unix_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Random 16-byte id.
pub fn random_id() -> [u8; 16] {
    rand::random()
}

/// 4 random hex chars.
pub fn random_suffix() -> String {
    hex::encode(rand::random::<[u8; 2]>())
}

/// Peer-supplied free text (device names, messages) made safe to log, persist
/// and show: control characters removed, at most `max_chars` characters.
pub fn clean_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_and_caps() {
        assert_eq!(clean_text("a\nb\u{1b}[31mc", 10), "ab[31mc");
        assert_eq!(clean_text(&"x".repeat(500), 64).len(), 64);
        assert_eq!(clean_text("Máy ảnh", 3), "Máy");
    }
}
