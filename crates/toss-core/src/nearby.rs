//! v0.8 "Gần đây" (contracts/nearby-v0.8.md §1.1): Toss devices seen on this
//! LAN via mDNS, the receive policy, and the mDNS user data that carries a
//! device's name / platform / version.
//!
//! Every node with `net.mdns` advertises `n=<name>;p=<platform>;v=<version>`
//! (≤ 245 bytes) next to its addresses under `_toss._udp.local`. The node
//! subscribes to the mDNS lookup: `Discovered` upserts an entry, `Expired`
//! removes it, and entries not re-seen for [`STALE_AFTER`] are dropped. A
//! non-contact may push or send text to this device only while it is in this
//! store (see `peer.rs`).
//!
//! The user data is only given to the mDNS service (not to the endpoint, which
//! would also publish it through pkarr to the n0 DNS servers): see
//! `mdns::Advertiser`.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::util::{clean_text, now_unix};

/// Entries not re-seen (mDNS announcement or successful lookup) for this long
/// are dropped.
pub const STALE_AFTER: Duration = Duration::from_secs(180);

/// Longest mDNS user data (iroh `UserData::MAX_LENGTH`).
pub const MAX_USER_DATA: usize = 245;

/// A Toss device seen on this LAN (`Node::nearby`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NearbyPeer {
    /// Endpoint id, hex (same format as `Contact.id`).
    pub id: String,
    /// Device name it advertises ("" for a Toss < 0.8, then the contact name
    /// or "Máy xxxxxx").
    pub name: String,
    /// "macos" | "windows" | "linux" | … ("" = unknown, Toss < 0.8).
    pub platform: String,
    /// App version it advertises ("" = Toss < 0.8: it cannot receive pushes
    /// or text from non-contacts).
    pub version: String,
    /// Direct socket addrs seen via mDNS ("192.168.1.5:51234").
    pub addrs: Vec<String>,
    /// Unix seconds of the last announcement / successful lookup.
    pub last_seen: u64,
    /// Already in contacts.
    pub contact: bool,
}

/// Who may send to this device without being asked (`Node::set_receive_policy`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceivePolicy {
    /// Every push asks, contacts too.
    Off,
    /// Contacts with `auto_accept` are accepted at once; others ask (default).
    #[default]
    Contacts,
    /// Like `Contacts`, and nearby strangers are accepted at once too
    /// (without being added to contacts).
    Everyone,
}

impl ReceivePolicy {
    /// Whether a push is accepted without asking the user.
    ///
    /// `contact_auto` = the sender is a contact with `auto_accept` (None = not
    /// a contact). Strangers reach this only when they are nearby.
    pub fn auto_accepts(self, contact_auto: Option<bool>) -> bool {
        match (self, contact_auto) {
            (ReceivePolicy::Off, _) => false,
            (_, Some(auto)) => auto,
            (ReceivePolicy::Contacts, None) => false,
            (ReceivePolicy::Everyone, None) => true,
        }
    }
}

/// `n=<name>;p=<platform>;v=<version>`, at most [`MAX_USER_DATA`] bytes (the
/// name is cut at a character boundary; `;` and `=` in it become `,` / `-`).
pub fn encode_user_data(name: &str, platform: &str, version: &str) -> String {
    let field = |s: &str, max: usize| -> String {
        clean_text(s, max)
            .chars()
            .map(|c| match c {
                ';' => ',',
                '=' => '-',
                c => c,
            })
            .collect()
    };
    let platform = field(platform, 16);
    let version = field(version, 32);
    let tail = format!(";p={platform};v={version}");
    let budget = MAX_USER_DATA.saturating_sub(tail.len() + 2);
    let mut name = field(name, crate::contacts::MAX_NAME);
    while name.len() > budget {
        name.pop();
    }
    format!("n={name}{tail}")
}

/// Parses [`encode_user_data`] output: `(name, platform, version)`; unknown
/// keys are ignored, missing ones are "".
pub fn parse_user_data(s: &str) -> (String, String, String) {
    let (mut n, mut p, mut v) = (String::new(), String::new(), String::new());
    for part in s.split(';') {
        let Some((k, val)) = part.split_once('=') else {
            continue;
        };
        match k.trim() {
            "n" => n = clean_text(val.trim(), crate::contacts::MAX_NAME),
            "p" => p = clean_text(val.trim(), 16),
            "v" => v = clean_text(val.trim(), 32),
            _ => {}
        }
    }
    (n, p, v)
}

#[derive(Debug, Clone)]
struct Entry {
    peer: NearbyPeer,
    seen: Instant,
}

/// The nearby store (runtime only).
#[derive(Debug, Default)]
pub(crate) struct NearbyStore {
    map: Mutex<HashMap<String, Entry>>,
}

impl NearbyStore {
    /// Adds or refreshes an entry (its `contact` flag is ignored). Returns
    /// whether anything a UI shows changed (new entry, name, platform,
    /// version or addresses). Empty name/platform/version keep the old value
    /// (a lookup answer may lack the user data).
    pub fn upsert(&self, mut peer: NearbyPeer) -> bool {
        peer.contact = false;
        peer.last_seen = now_unix();
        let mut map = self.map.lock();
        let now = Instant::now();
        match map.get_mut(&peer.id) {
            Some(e) => {
                let old = e.peer.clone();
                if !peer.name.is_empty() {
                    e.peer.name = peer.name;
                }
                if !peer.platform.is_empty() {
                    e.peer.platform = peer.platform;
                }
                if !peer.version.is_empty() {
                    e.peer.version = peer.version;
                }
                if !peer.addrs.is_empty() {
                    e.peer.addrs = peer.addrs;
                }
                e.peer.last_seen = peer.last_seen;
                e.seen = now;
                let o = (&old.name, &old.platform, &old.version, &old.addrs);
                o != (
                    &e.peer.name,
                    &e.peer.platform,
                    &e.peer.version,
                    &e.peer.addrs,
                )
            }
            None => {
                map.insert(peer.id.clone(), Entry { peer, seen: now });
                true
            }
        }
    }

    pub fn remove(&self, id: &str) -> bool {
        self.map.lock().remove(id).is_some()
    }

    pub fn get(&self, id: &str) -> Option<NearbyPeer> {
        self.map.lock().get(id).map(|e| e.peer.clone())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.map.lock().contains_key(id)
    }

    /// Ids not seen for at least `age`.
    pub fn older_than(&self, age: Duration) -> Vec<String> {
        let now = Instant::now();
        self.map
            .lock()
            .values()
            .filter(|e| now.duration_since(e.seen) >= age)
            .map(|e| e.peer.id.clone())
            .collect()
    }

    /// Drops entries not seen for [`STALE_AFTER`] (as of `now`); returns their ids.
    pub fn sweep(&self, now: Instant) -> Vec<String> {
        let mut gone = Vec::new();
        self.map.lock().retain(|id, e| {
            let keep = now.saturating_duration_since(e.seen) < STALE_AFTER;
            if !keep {
                gone.push(id.clone());
            }
            keep
        });
        gone
    }

    /// Test hook: pretends `id` was last seen `secs` ago.
    pub fn age(&self, id: &str, secs: u64) {
        if let Some(e) = self.map.lock().get_mut(id) {
            if let Some(t) = Instant::now().checked_sub(Duration::from_secs(secs)) {
                e.seen = t;
            }
            e.peer.last_seen = now_unix().saturating_sub(secs);
        }
    }

    /// All entries sorted by name, then id (`contact` not filled in).
    pub fn list(&self) -> Vec<NearbyPeer> {
        let mut v: Vec<NearbyPeer> = self.map.lock().values().map(|e| e.peer.clone()).collect();
        v.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, name: &str) -> NearbyPeer {
        NearbyPeer {
            id: id.into(),
            name: name.into(),
            platform: "linux".into(),
            version: "0.8.0".into(),
            addrs: vec!["192.168.1.5:5000".into()],
            last_seen: 0,
            contact: true,
        }
    }

    #[test]
    fn user_data_roundtrip_and_limits() {
        let s = encode_user_data("Đạt – Mac", "macos", "0.8.0");
        assert_eq!(s, "n=Đạt – Mac;p=macos;v=0.8.0");
        assert_eq!(
            parse_user_data(&s),
            ("Đạt – Mac".into(), "macos".into(), "0.8.0".into())
        );
        // Separators in the name cannot break the format.
        let s = encode_user_data("a;p=evil=x", "linux", "1");
        assert_eq!(parse_user_data(&s).0, "a,p-evil-x");
        assert_eq!(parse_user_data(&s).1, "linux");
        // Long multi-byte names are cut to fit 245 bytes, at a char boundary.
        let long = "Ứ".repeat(200);
        let s = encode_user_data(&long, "windows", "0.8.0");
        assert!(s.len() <= MAX_USER_DATA, "{}", s.len());
        assert!(s.ends_with(";p=windows;v=0.8.0"));
        assert!(s.parse::<iroh::address_lookup::UserData>().is_ok());
        // Garbage and old formats parse to empty fields.
        assert_eq!(parse_user_data("hello"), Default::default());
        assert_eq!(parse_user_data("v=0.9;x=1").2, "0.9");
    }

    #[test]
    fn store_upsert_expire_and_staleness() {
        let st = NearbyStore::default();
        assert!(st.upsert(peer("a", "Máy A")));
        // Same data again: no change to report, but refreshed.
        assert!(!st.upsert(peer("a", "Máy A")));
        // A lookup answer without user data keeps the name.
        let mut bare = peer("a", "");
        bare.version.clear();
        bare.platform.clear();
        assert!(!st.upsert(bare));
        assert_eq!(st.get("a").unwrap().name, "Máy A");
        assert!(
            !st.get("a").unwrap().contact,
            "contact is filled in by the node"
        );
        let mut moved = peer("a", "Máy A");
        moved.addrs = vec!["192.168.1.9:5000".into()];
        assert!(st.upsert(moved));
        assert!(st.upsert(peer("b", "bé")));
        assert_eq!(
            st.list().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"],
            "sorted by name (case-insensitive)"
        );
        // Staleness: only entries older than 3 min go.
        st.age("a", 179);
        assert!(st.sweep(Instant::now()).is_empty());
        assert_eq!(
            st.older_than(Duration::from_secs(60)),
            vec!["a".to_string()]
        );
        st.age("a", 181);
        assert_eq!(st.sweep(Instant::now()), vec!["a".to_string()]);
        assert!(!st.contains("a"));
        // Seen again (announcement / lookup answer) keeps it alive.
        st.age("b", 170);
        assert!(!st.upsert(peer("b", "bé")));
        assert!(st
            .sweep(Instant::now() + Duration::from_secs(20))
            .is_empty());
        // Expired event.
        assert!(st.remove("b"));
        assert!(!st.remove("b"));
        assert!(st.list().is_empty());
    }

    #[test]
    fn policy_matrix() {
        use ReceivePolicy::*;
        // (policy, sender) -> auto?
        let cases = [
            (Off, Some(true), false),
            (Off, Some(false), false),
            (Off, None, false),
            (Contacts, Some(true), true),
            (Contacts, Some(false), false),
            (Contacts, None, false),
            (Everyone, Some(true), true),
            (Everyone, Some(false), false),
            (Everyone, None, true),
        ];
        for (p, who, want) in cases {
            assert_eq!(p.auto_accepts(who), want, "{p:?} {who:?}");
        }
        assert_eq!(ReceivePolicy::default(), Contacts);
        assert_eq!(serde_json::to_string(&Everyone).unwrap(), "\"everyone\"");
        assert_eq!(
            serde_json::from_str::<ReceivePolicy>("\"off\"").unwrap(),
            Off
        );
    }
}
