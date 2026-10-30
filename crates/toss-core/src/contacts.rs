//! v0.5 "Danh bạ máy" (contracts/contacts-v0.5.md §2.1, §3): contacts
//! (paired devices, trusted by endpoint id), signed pairing invites and their
//! stores `<data_dir>/contacts.json` and `<data_dir>/invites.json`.

use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, bail, Context, Result};
use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, PublicKey, RelayUrl, SecretKey, Signature, TransportAddr};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    jobs::write_atomic,
    ticket::{exp_from_days, sig64, HostAddr, MAX_TICKET_ADDRS},
    util::{clean_text, now_unix},
};

/// Invite format version.
pub const INVITE_VERSION: u8 = 1;
/// Prefix of the invite string form.
pub const INVITE_PREFIX: &str = "tossinv1";
/// Longest device / contact name kept.
pub const MAX_NAME: usize = 64;
/// Default invite validity.
pub const DEFAULT_INVITE_DAYS: u32 = 7;
/// Page host used for `InviteInfo::web_url` when none is given
/// ([`crate::Node::parse_invite`]).
pub const DEFAULT_PAGE_HOST: &str = "toss-link.vercel.app";

/// A paired device (persisted in `<data_dir>/contacts.json`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    /// Endpoint id, hex (64 chars).
    pub id: String,
    /// Display name (editable locally).
    pub name: String,
    /// Name the device reports.
    pub their_name: String,
    /// "macos" | "windows" | "linux" | other.
    pub platform: String,
    /// Unix seconds.
    pub added_at: u64,
    /// Accept pushes from this device without asking (default true).
    pub auto_accept: bool,
    /// Runtime: answered the last presence ping / has a live connection.
    #[serde(default)]
    pub online: bool,
    /// Unix seconds of the last contact, 0 = never.
    #[serde(default)]
    pub last_seen: u64,
    /// Runtime: RTT of the last presence ping, ms.
    #[serde(default)]
    pub rtt_ms: Option<u32>,
}

/// A pairing link this device issued ([`crate::Node::create_invite`]) or
/// parsed ([`crate::Node::parse_invite`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InviteInfo {
    /// `tossinv1…`
    pub invite: String,
    /// `https://<page_host>/#tossinv1…`
    pub web_url: String,
    /// `toss://i/tossinv1…`
    pub deep_link: String,
    /// Expiry, unix seconds (0 = never).
    pub exp: u64,
    /// Name of the inviting device.
    pub name: String,
}

/// A push waiting for the user ([`crate::Node::answer_push`]): from a contact,
/// or (v0.8, `stranger`) from a nearby device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IncomingPush {
    /// hex(push id), 32 chars.
    pub push_id: String,
    /// Sender endpoint id (hex).
    pub from_id: String,
    /// Sender contact name.
    pub from_name: String,
    /// Job (folder) name.
    pub job: String,
    /// Files announced.
    pub files: u32,
    /// Bytes announced.
    pub bytes: u64,
    /// Unix seconds.
    pub at: u64,
    /// v0.8: the sender is not a contact (a device on this LAN, `PushFrom`).
    #[serde(default)]
    pub stranger: bool,
}

/// This platform as reported in invites and pairing ("macos" | "windows" | "linux" | …).
pub fn platform() -> String {
    std::env::consts::OS.to_string()
}

// ───────────────────────────── invites ─────────────────────────────

/// The signed part of an invite. Field order is the postcard wire order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InviteBody {
    /// Format version, = 1.
    pub v: u8,
    /// Inviter endpoint id (ed25519 public key).
    pub id: [u8; 32],
    /// Inviter device name (≤ 64 chars).
    pub name: String,
    /// Inviter platform.
    pub platform: String,
    /// Bearer secret, checked against the inviter's `invites.json`.
    pub secret: [u8; 16],
    /// Expiry, unix seconds; 0 = never.
    pub exp: u64,
    /// Inviter home relay URL (`""` = unknown).
    pub relay: String,
    /// Inviter direct addresses, best first (same rules as tickets).
    pub addrs: Vec<String>,
}

/// An invite: body plus ed25519 signature by the inviter key over `postcard(body)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// Signed content.
    pub body: InviteBody,
    /// Signature over `postcard(body)`.
    #[serde(with = "sig64")]
    pub sig: [u8; 64],
}

impl Invite {
    /// Signs a new invite for `secret_key`'s endpoint.
    pub fn sign(
        secret_key: &SecretKey,
        name: &str,
        secret: [u8; 16],
        exp: u64,
        addr: &HostAddr,
    ) -> Invite {
        let body = InviteBody {
            v: INVITE_VERSION,
            id: *secret_key.public().as_bytes(),
            name: clean_text(name, MAX_NAME),
            platform: platform(),
            secret,
            exp,
            relay: addr.relay.clone(),
            addrs: addr.addrs.iter().take(MAX_TICKET_ADDRS).cloned().collect(),
        };
        let bytes = postcard::to_allocvec(&body).unwrap_or_default();
        let sig = secret_key.sign(&bytes).to_bytes();
        Invite { body, sig }
    }

    /// Signature valid (by the key in the body) and not expired.
    pub fn verify(&self) -> Result<()> {
        let pk = self.inviter()?;
        let bytes = postcard::to_allocvec(&self.body).context("encode invite")?;
        pk.verify(&bytes, &Signature::from_bytes(&self.sig))
            .map_err(|_| anyhow!("lời mời không hợp lệ (chữ ký sai)"))?;
        if self.body.exp != 0 && now_unix() >= self.body.exp {
            bail!("lời mời đã hết hạn — xin một lời mời mới");
        }
        Ok(())
    }

    /// The inviter's endpoint id.
    pub fn inviter(&self) -> Result<PublicKey> {
        PublicKey::from_bytes(&self.body.id).map_err(|_| anyhow!("lời mời hỏng (khoá)"))
    }

    /// What to dial: inviter id + relay + direct addresses.
    pub fn endpoint_addr(&self) -> Result<EndpointAddr> {
        Ok(addr_from_hint(self.inviter()?, &self.host_addr()))
    }

    /// The relay/addresses hint.
    pub fn host_addr(&self) -> HostAddr {
        HostAddr {
            relay: self.body.relay.clone(),
            addrs: self.body.addrs.clone(),
        }
    }

    /// `toss://i/tossinv1…`
    pub fn deep_link(&self) -> String {
        format!("toss://i/{self}")
    }

    /// `https://<page_host>/#tossinv1…`
    pub fn web_url(&self, page_host: &str) -> String {
        let host = page_host
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let host = if host.is_empty() {
            DEFAULT_PAGE_HOST
        } else {
            host
        };
        format!("https://{host}/#{self}")
    }

    /// UI summary.
    pub fn info(&self, page_host: &str) -> InviteInfo {
        InviteInfo {
            invite: self.to_string(),
            web_url: self.web_url(page_host),
            deep_link: self.deep_link(),
            exp: self.body.exp,
            name: self.body.name.clone(),
        }
    }
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = postcard::to_allocvec(self).map_err(|_| fmt::Error)?;
        f.write_str(INVITE_PREFIX)?;
        f.write_str(&BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
    }
}

impl FromStr for Invite {
    type Err = anyhow::Error;

    /// Tolerant like `Ticket::from_str`: case, whitespace, `toss://i/…` and
    /// `https://<any host>/#…` prefixes. Does not check signature or expiry.
    fn from_str(s: &str) -> Result<Invite> {
        let s = s.trim();
        let s = if let Some(pos) = s.find('#') {
            &s[pos + 1..]
        } else if s
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("toss://i/"))
        {
            &s[9..]
        } else {
            s
        };
        let cleaned: String = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .trim_end_matches('/')
            .to_ascii_uppercase();
        let data = cleaned
            .strip_prefix(&INVITE_PREFIX.to_ascii_uppercase())
            .ok_or_else(|| anyhow!("không phải lời mời Toss (cần 'tossinv1…')"))?;
        let bytes = BASE32_NOPAD
            .decode(data.as_bytes())
            .map_err(|_| anyhow!("lời mời bị hỏng (thiếu ký tự?)"))?;
        match postcard::take_from_bytes::<Invite>(&bytes) {
            Ok((inv, [])) if inv.body.v == INVITE_VERSION => Ok(inv),
            Ok((inv, [])) => bail!("lời mời phiên bản {} chưa được hỗ trợ", inv.body.v),
            _ => bail!("lời mời bị hỏng"),
        }
    }
}

/// `EndpointAddr` from an id and a relay/addresses hint (bad entries skipped).
pub(crate) fn addr_from_hint(id: PublicKey, hint: &HostAddr) -> EndpointAddr {
    let mut addrs = Vec::new();
    if let Ok(u) = hint.relay.parse::<RelayUrl>() {
        addrs.push(TransportAddr::Relay(u));
    }
    for a in hint.addrs.iter().take(MAX_TICKET_ADDRS) {
        if let Ok(sa) = a.parse::<SocketAddr>() {
            addrs.push(TransportAddr::Ip(sa));
        }
    }
    EndpointAddr::from_parts(id, addrs)
}

// ───────────────────────────── invites.json ─────────────────────────────

/// One pairing made with an invite.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Pairing {
    id: String,
    name: String,
    at: u64,
}

/// An invite this device issued.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Issued {
    /// hex(secret)
    secret: String,
    created: u64,
    exp: u64,
    #[serde(default)]
    revoked: bool,
    #[serde(default)]
    pairings: Vec<Pairing>,
}

/// `<data_dir>/invites.json`, re-read on every access (a CLI `toss-cli invite`
/// may add one while a host runs).
#[derive(Debug)]
pub(crate) struct InviteStore {
    path: PathBuf,
    lock: Mutex<()>,
}

/// Issued invites older than this past their expiry are dropped from the file.
const KEEP_EXPIRED_S: u64 = 30 * 86_400;

impl InviteStore {
    pub fn open(data_dir: &Path) -> InviteStore {
        InviteStore {
            path: data_dir.join("invites.json"),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Vec<Issued> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, v: &[Issued]) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(v)?)
    }

    /// Records a newly issued secret.
    pub fn add(&self, secret: [u8; 16], exp: u64) -> Result<()> {
        let _g = self.lock.lock();
        let now = now_unix();
        let mut v = self.load();
        v.retain(|i| i.exp == 0 || i.exp + KEEP_EXPIRED_S > now);
        v.push(Issued {
            secret: hex::encode(secret),
            created: now,
            exp,
            revoked: false,
            pairings: Vec::new(),
        });
        self.save(&v)
    }

    /// Marks every issued invite revoked.
    pub fn revoke_all(&self) -> Result<()> {
        let _g = self.lock.lock();
        let mut v = self.load();
        for i in &mut v {
            i.revoked = true;
        }
        self.save(&v)
    }

    /// Checks a pairing secret (valid, not revoked, not expired) and records
    /// the pairing. Err = Vietnamese reason for `Denied`.
    pub fn redeem(&self, secret: &[u8; 16], id: &str, name: &str) -> Result<(), String> {
        let _g = self.lock.lock();
        let mut v = self.load();
        let want = hex::encode(secret);
        let Some(inv) = v.iter_mut().find(|i| ct_eq(&i.secret, &want)) else {
            return Err("lời mời không đúng hoặc đã bị huỷ".into());
        };
        if inv.revoked {
            return Err("lời mời đã bị huỷ".into());
        }
        if inv.exp != 0 && now_unix() >= inv.exp {
            return Err("lời mời đã hết hạn".into());
        }
        inv.pairings.retain(|p| p.id != id);
        inv.pairings.push(Pairing {
            id: id.to_string(),
            name: name.to_string(),
            at: now_unix(),
        });
        if let Err(e) = self.save(&v) {
            tracing::warn!("could not record pairing in invites.json: {e:#}");
        }
        Ok(())
    }
}

/// Constant-time comparison of two equal-length strings.
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ───────────────────────────── contacts.json ─────────────────────────────

/// A contact as stored: the public record plus the last known address hint.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct StoredContact {
    #[serde(flatten)]
    pub c: Contact,
    #[serde(default)]
    pub addr: HostAddr,
}

/// `<data_dir>/contacts.json` with the runtime presence state in memory.
#[derive(Debug)]
pub(crate) struct ContactBook {
    path: PathBuf,
    map: Mutex<BTreeMap<String, StoredContact>>,
}

impl ContactBook {
    pub fn open(data_dir: &Path) -> ContactBook {
        let path = data_dir.join("contacts.json");
        let list: Vec<StoredContact> = match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(v) => v,
                Err(e) => {
                    let bad = path.with_extension(format!("json.bad-{}", now_unix()));
                    tracing::warn!("contacts.json is damaged ({e}); moved to {}", bad.display());
                    let _ = std::fs::rename(&path, &bad);
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        let map = list
            .into_iter()
            .map(|mut s| {
                s.c.online = false;
                s.c.rtt_ms = None;
                (s.c.id.clone(), s)
            })
            .collect();
        ContactBook {
            path,
            map: Mutex::new(map),
        }
    }

    fn save_locked(&self, map: &BTreeMap<String, StoredContact>) {
        let mut v: Vec<&StoredContact> = map.values().collect();
        v.sort_by_key(|s| (s.c.added_at, s.c.id.clone()));
        let res = serde_json::to_vec_pretty(&v)
            .map_err(anyhow::Error::from)
            .and_then(|b| write_atomic(&self.path, &b));
        if let Err(e) = res {
            tracing::warn!("could not save contacts.json: {e:#}");
        }
    }

    /// All contacts, oldest first.
    pub fn list(&self) -> Vec<Contact> {
        let map = self.map.lock();
        let mut v: Vec<Contact> = map.values().map(|s| s.c.clone()).collect();
        v.sort_by_key(|c| (c.added_at, c.id.clone()));
        v
    }

    pub fn get(&self, id: &str) -> Option<StoredContact> {
        self.map.lock().get(id).cloned()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.map.lock().contains_key(id)
    }

    /// Adds a contact or refreshes `their_name` / `platform` / hint of an
    /// existing one (keeping its local name and settings). Returns it.
    pub fn upsert(&self, id: &str, their_name: &str, platform: &str, addr: HostAddr) -> Contact {
        let mut map = self.map.lock();
        let their_name = clean_text(their_name, MAX_NAME);
        let platform = clean_text(platform, 16);
        let now = now_unix();
        let entry = map.entry(id.to_string()).or_insert_with(|| StoredContact {
            c: Contact {
                id: id.to_string(),
                name: if their_name.trim().is_empty() {
                    format!("Máy {}", &id[..id.len().min(6)])
                } else {
                    their_name.clone()
                },
                their_name: their_name.clone(),
                platform: platform.clone(),
                added_at: now,
                auto_accept: true,
                online: false,
                last_seen: 0,
                rtt_ms: None,
            },
            addr: HostAddr::default(),
        });
        if !their_name.is_empty() {
            entry.c.their_name = their_name;
        }
        if !platform.is_empty() {
            entry.c.platform = platform;
        }
        if !addr.is_empty() {
            entry.addr = addr;
        }
        entry.c.last_seen = now;
        let c = entry.c.clone();
        self.save_locked(&map);
        c
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Contact)) -> Result<()> {
        let mut map = self.map.lock();
        let s = map
            .get_mut(id)
            .ok_or_else(|| anyhow!("không có máy {id} trong danh bạ"))?;
        f(&mut s.c);
        self.save_locked(&map);
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut map = self.map.lock();
        if map.remove(id).is_none() {
            bail!("không có máy {id} trong danh bạ");
        }
        self.save_locked(&map);
        Ok(())
    }

    /// Remembers a fresher address hint (not persisted on its own).
    pub fn set_hint(&self, id: &str, addr: HostAddr) {
        if addr.is_empty() {
            return;
        }
        if let Some(s) = self.map.lock().get_mut(id) {
            s.addr = addr;
        }
    }

    /// Records a presence result. Returns `Some(contact)` when the online
    /// flag or the RTT changed (an event is due). `last_seen` is persisted
    /// when the contact comes online.
    pub fn set_presence(&self, id: &str, online: bool, rtt_ms: Option<u32>) -> Option<Contact> {
        let mut map = self.map.lock();
        let s = map.get_mut(id)?;
        let was = (s.c.online, s.c.rtt_ms);
        s.c.online = online;
        if online {
            s.c.last_seen = now_unix();
            if rtt_ms.is_some() {
                s.c.rtt_ms = rtt_ms;
            }
        } else {
            s.c.rtt_ms = None;
        }
        let changed = was != (s.c.online, s.c.rtt_ms);
        let c = s.c.clone();
        if !was.0 && online {
            self.save_locked(&map);
        }
        changed.then_some(c)
    }
}

/// Reads `<data_dir>/contacts.json` without a node (CLI). Presence fields are
/// runtime only (offline).
pub fn read_contacts(data_dir: &Path) -> Vec<Contact> {
    ContactBook::open(data_dir).list()
}

/// Finds a contact by id, name (case-insensitive) or unique id prefix (≥ 6 hex chars).
pub fn resolve_in(list: &[Contact], needle: &str) -> Option<Contact> {
    let n = needle.trim();
    if let Some(c) = list.iter().find(|c| c.id == n.to_ascii_lowercase()) {
        return Some(c.clone());
    }
    let by_name: Vec<&Contact> = list
        .iter()
        .filter(|c| {
            let l = n.to_lowercase();
            c.name.to_lowercase() == l || c.their_name.to_lowercase() == l
        })
        .collect();
    if by_name.len() == 1 {
        return Some(by_name[0].clone());
    }
    if n.len() >= 6 && n.chars().all(|c| c.is_ascii_hexdigit()) {
        let p = n.to_ascii_lowercase();
        let hits: Vec<&Contact> = list.iter().filter(|c| c.id.starts_with(&p)).collect();
        if hits.len() == 1 {
            return Some(hits[0].clone());
        }
    }
    None
}

/// Creates an invite: random secret, recorded in `invites.json`, signed with
/// `secret_key`, carrying `addr` (the live or last known addresses).
pub(crate) fn issue_invite(
    store: &InviteStore,
    secret_key: &SecretKey,
    name: &str,
    days: u32,
    addr: &HostAddr,
) -> Result<Invite> {
    let secret: [u8; 16] = rand::random();
    let exp = exp_from_days(days);
    store.add(secret, exp)?;
    Ok(Invite::sign(secret_key, name, secret, exp, addr))
}

/// Creates an invite without a running node (`toss-cli invite` while
/// `toss-cli host` runs): loads the key from `data_dir` and records the secret
/// in its `invites.json`; `addr` = the running node's last known addresses.
pub fn create_invite_offline(
    data_dir: &Path,
    name: &str,
    days: u32,
    page_host: &str,
    addr: &HostAddr,
) -> Result<InviteInfo> {
    let key = crate::node::load_or_create_secret_key(data_dir)?;
    let inv = issue_invite(&InviteStore::open(data_dir), &key, name, days, addr)?;
    Ok(inv.info(page_host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_roundtrip_tolerant() {
        let sk = SecretKey::generate();
        let addr = HostAddr {
            relay: "https://relay.example./".into(),
            addrs: vec!["192.168.1.5:41820".into()],
        };
        let inv = Invite::sign(&sk, "Đạt – Mac", [7; 16], 0, &addr);
        let s = inv.to_string();
        assert!(s.starts_with(INVITE_PREFIX));
        for variant in [
            s.clone(),
            s.to_ascii_uppercase(),
            format!("  toss://i/{s}/ \n"),
            format!("https://x.example/#{s}"),
            format!("{} {}", &s[..20], &s[20..]),
        ] {
            let p: Invite = variant.parse().unwrap();
            assert_eq!(p, inv);
            p.verify().unwrap();
        }
        let mut bad = inv.clone();
        bad.body.name = "Mallory".into();
        assert!(bad.verify().is_err());
        assert!("toss1abc".parse::<Invite>().is_err());
        let ea = inv.endpoint_addr().unwrap();
        assert_eq!(ea.id, sk.public());
        assert_eq!(ea.ip_addrs().count(), 1);
    }

    #[test]
    fn invite_store_multi_use_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let st = InviteStore::open(dir.path());
        st.add([1; 16], 0).unwrap();
        st.add([2; 16], now_unix() - 1).unwrap();
        assert!(st.redeem(&[1; 16], "a", "A").is_ok());
        assert!(st.redeem(&[1; 16], "b", "B").is_ok());
        assert!(st.redeem(&[2; 16], "a", "A").is_err());
        assert!(st.redeem(&[3; 16], "a", "A").is_err());
        st.revoke_all().unwrap();
        assert!(st.redeem(&[1; 16], "c", "C").is_err());
    }

    #[test]
    fn contact_book_persists_and_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let id = "ab".repeat(32);
        {
            let b = ContactBook::open(dir.path());
            b.upsert(&id, "Studio PC", "windows", HostAddr::default());
            b.update(&id, |c| c.name = "Máy tổng".into()).unwrap();
            assert!(b.set_presence(&id, true, Some(3)).is_some());
            assert!(b.set_presence(&id, true, Some(3)).is_none());
            // Re-pairing keeps the local name.
            b.upsert(&id, "Studio PC 2", "windows", HostAddr::default());
        }
        let b = ContactBook::open(dir.path());
        let c = b.get(&id).unwrap().c;
        assert_eq!(c.name, "Máy tổng");
        assert_eq!(c.their_name, "Studio PC 2");
        assert!(!c.online, "online is runtime only");
        assert!(c.auto_accept);
        let list = b.list();
        assert_eq!(resolve_in(&list, "máy TỔNG").unwrap().id, id);
        assert_eq!(resolve_in(&list, "Máy tổng").unwrap().id, id);
        assert_eq!(resolve_in(&list, "ababab").unwrap().id, id);
        assert_eq!(resolve_in(&list, &id.to_uppercase()).unwrap().id, id);
        assert!(resolve_in(&list, "abab").is_none());
        b.remove(&id).unwrap();
        assert!(read_contacts(dir.path()).is_empty());
    }
}
