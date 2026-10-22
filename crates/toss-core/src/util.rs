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
