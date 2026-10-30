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

/// A `&'static str` for `s`, one leaked allocation per distinct value (used
/// for the device name, which changes rarely but is read as `&str`).
pub(crate) fn intern(s: &str) -> &'static str {
    static NAMES: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashSet<&'static str>>> =
        std::sync::LazyLock::new(Default::default);
    let mut set = NAMES.lock();
    if let Some(v) = set.get(s) {
        return v;
    }
    let v: &'static str = Box::leak(s.to_string().into_boxed_str());
    set.insert(v);
    v
}

/// Text message body: control characters other than newline / tab removed,
/// cut to at most `max_bytes` bytes at a character boundary.
pub fn clean_message(s: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_bytes));
    for c in s.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if out.len() + c.len_utf8() > max_bytes {
            break;
        }
        out.push(c);
    }
    out
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
