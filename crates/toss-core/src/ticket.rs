//! Signed tickets (wire.md §1).

use std::{
    collections::BTreeSet,
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use data_encoding::BASE32_NOPAD;
use iroh::{EndpointAddr, PublicKey, RelayUrl, SecretKey, Signature, TransportAddr};
use serde::{Deserialize, Serialize};

use crate::util::now_unix;

/// Current ticket format version (2: `relay` + `addrs` appended to the body).
pub const TICKET_VERSION: u8 = 2;
/// Maximum number of direct addresses carried in a ticket.
pub const MAX_TICKET_ADDRS: usize = 8;
/// Prefix of the string form.
pub const TICKET_PREFIX: &str = "toss1";
/// Maximum length of `TicketBody::job` in characters.
pub const MAX_JOB_LEN: usize = 80;

/// The signed part of a ticket. Field order is the postcard wire order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TicketBody {
    /// Format version, = 2.
    pub v: u8,
    /// Random job id; `hex(id)` is the job id shown in UIs.
    pub id: [u8; 16],
    /// Host `EndpointId` (ed25519 public key).
    pub host: [u8; 32],
    /// Human job name (≤ 80 chars), used as folder name after sanitizing.
    pub job: String,
    /// Host root alias, e.g. `RAW`.
    pub root: String,
    /// Path under root, `/`-separated, may be empty.
    pub path: String,
    /// Path under the host's return root where puts land; `""` = `<job>`.
    pub ret: String,
    /// Client may download `root/path`.
    pub get: bool,
    /// Client may upload into `return_root/ret`.
    pub put: bool,
    /// Control-channel access; `get`/`put` are false when set.
    pub admin: bool,
    /// Expiry, unix seconds; 0 = never.
    pub exp: u64,
    /// v2: the host's home relay URL when the ticket was signed; `""` = unknown.
    pub relay: String,
    /// v2: the host's direct socket addresses (`ip:port` / `[ip6]:port`) when
    /// the ticket was signed, best first, at most [`MAX_TICKET_ADDRS`]; may be empty.
    pub addrs: Vec<String>,
}

/// Where a host can be reached: the `relay` + `addrs` hint carried by v2
/// tickets (wire.md §1). Clients dial these directly; n0 DNS/pkarr lookup
/// stays active as a fallback.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct HostAddr {
    /// Home relay URL, `""` = none known.
    #[serde(default)]
    pub relay: String,
    /// Direct socket addresses as text, best first, ≤ [`MAX_TICKET_ADDRS`].
    #[serde(default)]
    pub addrs: Vec<String>,
}

impl HostAddr {
    /// Picks the relay URL and up to [`MAX_TICKET_ADDRS`] direct addresses of
    /// `addr`: public IPv4, public IPv6, private (LAN) IPv4, then the rest;
    /// loopback, unspecified and link-local addresses are left out.
    pub fn from_endpoint_addr(addr: &EndpointAddr) -> HostAddr {
        let relay = addr
            .relay_urls()
            .next()
            .map(|u| u.to_string())
            .unwrap_or_default();
        let mut ranked: Vec<(u8, SocketAddr)> = addr
            .ip_addrs()
            .filter_map(|a| addr_rank(a).map(|r| (r, *a)))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        ranked.sort_by_key(|(r, _)| *r);
        HostAddr {
            relay,
            addrs: ranked
                .into_iter()
                .take(MAX_TICKET_ADDRS)
                .map(|(_, a)| a.to_string())
                .collect(),
        }
    }

    /// True when neither a relay nor a direct address is known.
    pub fn is_empty(&self) -> bool {
        self.relay.is_empty() && self.addrs.is_empty()
    }

    /// `relay=<url|-> direct=<n>` for logs and CLI output.
    pub fn summary(&self) -> String {
        format!(
            "relay={} direct={}",
            if self.relay.is_empty() {
                "-"
            } else {
                &self.relay
            },
            self.addrs.len()
        )
    }
}

/// Sort rank of a direct address for tickets (lower = better), `None` = useless remotely.
fn addr_rank(a: &SocketAddr) -> Option<u8> {
    if a.port() == 0 {
        return None;
    }
    match a.ip().to_canonical() {
        IpAddr::V4(ip) => {
            if ip.is_unspecified() || ip.is_loopback() || ip.is_link_local() || ip.is_broadcast() {
                None
            } else if ip.is_private() || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64) {
                // RFC 1918 and carrier-grade NAT (100.64/10): LAN only.
                Some(2)
            } else {
                Some(0)
            }
        }
        IpAddr::V6(ip) => {
            let seg0 = ip.segments()[0];
            if ip.is_unspecified() || ip.is_loopback() || (seg0 & 0xffc0) == 0xfe80 {
                None
            } else if (seg0 & 0xfe00) == 0xfc00 {
                // Unique local (fc00::/7): LAN only.
                Some(3)
            } else {
                Some(1)
            }
        }
    }
}

/// A ticket: body plus ed25519 signature by the host key over `postcard(body)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    /// Signed content.
    pub body: TicketBody,
    /// ed25519 signature over `postcard(body)`.
    #[serde(with = "sig64")]
    pub sig: [u8; 64],
}

/// What a host needs to mint a job ticket (wire.md §4).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TicketSpec {
    /// Human job name.
    pub job: String,
    /// Root alias.
    pub root: String,
    /// Path under root.
    pub path: String,
    /// Return sub-path for puts (`""` = job name).
    pub ret: String,
    /// Allow download.
    pub get: bool,
    /// Allow upload.
    pub put: bool,
    /// Validity in days; 0 = never expires.
    pub days: u32,
}

/// Errors from parsing or verifying a ticket.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TicketError {
    /// Not a `toss1…` string.
    #[error("not a Toss ticket (expected 'toss1…')")]
    BadPrefix,
    /// Base32 decoding failed.
    #[error("ticket is damaged (base32): {0}")]
    BadEncoding(String),
    /// Postcard decoding failed.
    #[error("ticket is damaged (decode): {0}")]
    BadFormat(String),
    /// Unknown version (v1 tickets are no longer accepted).
    #[error(
        "ticket format v{0} is not supported by this app (expected v2); ask the host for a new link"
    )]
    BadVersion(u8),
    /// Signature does not verify or host key mismatch.
    #[error("ticket signature is invalid")]
    BadSignature,
    /// The ticket was issued by another host.
    #[error("ticket belongs to another host")]
    WrongHost,
    /// `exp` has passed.
    #[error("ticket expired")]
    Expired,
}

mod sig64 {
    use serde::{
        de::{Error, SeqAccess, Visitor},
        ser::SerializeTuple,
        Deserializer, Serializer,
    };

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        let mut t = s.serialize_tuple(64)?;
        for b in v {
            t.serialize_element(b)?;
        }
        t.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = [u8; 64];
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("64 bytes")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<[u8; 64], A::Error> {
                let mut out = [0u8; 64];
                for (i, b) in out.iter_mut().enumerate() {
                    *b = seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(i, &self))?;
                }
                Ok(out)
            }
        }
        d.deserialize_tuple(64, V)
    }
}

impl TicketBody {
    /// Postcard encoding of the body (the signed message).
    pub fn signing_bytes(&self) -> Vec<u8> {
        // Serializing plain structs/strings into a Vec cannot fail.
        postcard::to_allocvec(self).unwrap_or_default()
    }

    /// The host key as an iroh [`PublicKey`].
    pub fn host_key(&self) -> Result<PublicKey, TicketError> {
        PublicKey::from_bytes(&self.host).map_err(|_| TicketError::BadSignature)
    }

    /// Sets the `relay` / `addrs` hint (capped at [`MAX_TICKET_ADDRS`]).
    pub fn set_host_addr(&mut self, addr: &HostAddr) {
        self.relay = addr.relay.clone();
        self.addrs = addr.addrs.iter().take(MAX_TICKET_ADDRS).cloned().collect();
    }

    /// The `relay` / `addrs` hint.
    pub fn host_addr(&self) -> HostAddr {
        HostAddr {
            relay: self.relay.clone(),
            addrs: self.addrs.clone(),
        }
    }
}

impl Ticket {
    /// Signs `body` with `secret`. `body.host` should be `secret.public()`.
    pub fn sign(body: TicketBody, secret: &SecretKey) -> Ticket {
        let sig = secret.sign(&body.signing_bytes()).to_bytes();
        Ticket { body, sig }
    }

    /// Checks wire.md §1 steps 1–2: issued by `host_pk`, signature valid, not expired.
    /// Steps 3–4 (revocation, roots, paths) are checked by the host.
    pub fn verify(&self, host_pk: &PublicKey) -> Result<(), TicketError> {
        if &self.body.host != host_pk.as_bytes() {
            return Err(TicketError::WrongHost);
        }
        self.verify_signature()?;
        if self.is_expired() {
            return Err(TicketError::Expired);
        }
        Ok(())
    }

    /// Checks the signature against the host key embedded in the body.
    pub fn verify_signature(&self) -> Result<(), TicketError> {
        let pk = self.body.host_key()?;
        let sig = Signature::from_bytes(&self.sig);
        pk.verify(&self.body.signing_bytes(), &sig)
            .map_err(|_| TicketError::BadSignature)
    }

    /// `"https://<page_host>/#toss1…"`.
    pub fn web_url(&self, page_host: &str) -> String {
        let host = page_host
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        format!("https://{host}/#{self}")
    }

    /// `"toss://t/toss1…"`.
    pub fn deep_link(&self) -> String {
        format!("toss://t/{self}")
    }

    /// `hex(body.id)`.
    pub fn id_hex(&self) -> String {
        hex::encode(self.body.id)
    }

    /// True when `exp != 0 && now >= exp`.
    pub fn is_expired(&self) -> bool {
        self.body.exp != 0 && now_unix() >= self.body.exp
    }

    /// The host endpoint id.
    pub fn host(&self) -> Result<iroh::EndpointId, TicketError> {
        self.body.host_key()
    }

    /// What to dial: the host id plus the ticket's relay URL and direct
    /// addresses. Unparsable entries are skipped (logged); at most
    /// [`MAX_TICKET_ADDRS`] direct addresses are used.
    pub fn endpoint_addr(&self) -> Result<EndpointAddr, TicketError> {
        let id = self.host()?;
        let mut addrs = Vec::new();
        if !self.body.relay.is_empty() {
            match self.body.relay.parse::<RelayUrl>() {
                Ok(u) => addrs.push(TransportAddr::Relay(u)),
                Err(e) => {
                    tracing::warn!(relay = %self.body.relay, "ignoring bad relay URL in ticket: {e}")
                }
            }
        }
        for a in self.body.addrs.iter().take(MAX_TICKET_ADDRS) {
            match a.parse::<SocketAddr>() {
                Ok(sa) => addrs.push(TransportAddr::Ip(sa)),
                Err(e) => tracing::warn!(addr = %a, "ignoring bad address in ticket: {e}"),
            }
        }
        Ok(EndpointAddr::from_parts(id, addrs))
    }

    /// Tolerant parse (see [`FromStr`]); provided as an inherent fn per core-api.md.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Ticket, TicketError> {
        <Ticket as FromStr>::from_str(s)
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = postcard::to_allocvec(self).map_err(|_| fmt::Error)?;
        f.write_str(TICKET_PREFIX)?;
        f.write_str(&BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
    }
}

impl FromStr for Ticket {
    type Err = TicketError;

    /// Accepts upper/lower case, surrounding/embedded whitespace, and the
    /// `toss://t/…` or `https://<any host>/#…` prefixes.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let s = if let Some(pos) = s.find('#') {
            &s[pos + 1..]
        } else if s
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("toss://t/"))
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
            .strip_prefix(&TICKET_PREFIX.to_ascii_uppercase())
            .ok_or(TicketError::BadPrefix)?;
        let bytes = BASE32_NOPAD
            .decode(data.as_bytes())
            .map_err(|e| TicketError::BadEncoding(e.to_string()))?;
        // `v` is the first byte of postcard(Ticket) (a u8 is one raw byte).
        // A v1 ticket (no `relay`/`addrs`) or a newer format does not decode
        // as v2, so a plausible other version byte is reported as such rather
        // than as damage.
        let v = bytes.first().copied().unwrap_or(0);
        let other_version = v != TICKET_VERSION && (1..=15).contains(&v);
        let ticket = match postcard::take_from_bytes::<Ticket>(&bytes) {
            Ok((t, [])) if t.body.v == TICKET_VERSION => t,
            _ if other_version => return Err(TicketError::BadVersion(v)),
            Ok((t, [])) => return Err(TicketError::BadVersion(t.body.v)),
            Ok(_) => return Err(TicketError::BadFormat("trailing bytes".into())),
            Err(e) => return Err(TicketError::BadFormat(e.to_string())),
        };
        Ok(ticket)
    }
}

/// Builds the body for a job ticket from a spec (no validation of roots/paths);
/// `relay`/`addrs` are empty (see [`TicketBody::set_host_addr`]).
pub fn body_from_spec(host: &PublicKey, spec: &TicketSpec, id: [u8; 16]) -> TicketBody {
    TicketBody {
        v: TICKET_VERSION,
        id,
        host: *host.as_bytes(),
        job: spec.job.chars().take(MAX_JOB_LEN).collect(),
        root: spec.root.clone(),
        path: spec.path.clone(),
        ret: spec.ret.clone(),
        get: spec.get,
        put: spec.put,
        admin: false,
        exp: exp_from_days(spec.days),
        relay: String::new(),
        addrs: Vec::new(),
    }
}

/// Builds the body for an admin ticket.
pub fn admin_body(host: &PublicKey, days: u32, id: [u8; 16]) -> TicketBody {
    TicketBody {
        v: TICKET_VERSION,
        id,
        host: *host.as_bytes(),
        job: "admin".into(),
        root: String::new(),
        path: String::new(),
        ret: String::new(),
        get: false,
        put: false,
        admin: true,
        exp: exp_from_days(days),
        relay: String::new(),
        addrs: Vec::new(),
    }
}

/// `0` → never, else now + days.
pub fn exp_from_days(days: u32) -> u64 {
    if days == 0 {
        0
    } else {
        now_unix() + u64::from(days) * 86_400
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(secret: &SecretKey, exp: u64) -> Ticket {
        let spec = TicketSpec {
            job: "12A3 RAW".into(),
            root: "RAW".into(),
            path: "2026/12A3".into(),
            ret: "12A3".into(),
            get: true,
            put: true,
            days: 7,
        };
        let mut body = body_from_spec(&secret.public(), &spec, [7u8; 16]);
        body.exp = exp;
        Ticket::sign(body, secret)
    }

    #[test]
    fn roundtrip_and_forms() {
        let sk = SecretKey::generate();
        let t = sample(&sk, now_unix() + 100);
        let s = t.to_string();
        assert!(s.starts_with("toss1"));
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(s.len() < 400, "len {}", s.len());
        assert_eq!(s.parse::<Ticket>().unwrap(), t);
        assert_eq!(Ticket::from_str(&s.to_uppercase()).unwrap(), t);
        assert_eq!(Ticket::from_str(&format!("  {s}\n")).unwrap(), t);
        assert_eq!(Ticket::from_str(&t.deep_link()).unwrap(), t);
        assert_eq!(Ticket::from_str(&t.web_url("toss.example.com")).unwrap(), t);
        assert_eq!(
            Ticket::from_str(&format!("http://x.y/path/#{}", s.to_uppercase())).unwrap(),
            t
        );
        let (a, b) = s.split_at(100);
        assert_eq!(Ticket::from_str(&format!("{a}\n  {b}")).unwrap(), t);
        assert_eq!(t.id_hex(), "07".repeat(16));
        assert_eq!(
            t.web_url("https://toss.example.com/"),
            format!("https://toss.example.com/#{s}")
        );
        t.verify(&sk.public()).unwrap();
    }

    #[test]
    fn bad_strings() {
        assert_eq!(Ticket::from_str("hello"), Err(TicketError::BadPrefix));
        assert!(matches!(
            Ticket::from_str("toss1!!!"),
            Err(TicketError::BadEncoding(_))
        ));
        assert!(matches!(
            Ticket::from_str("toss1aaaa"),
            Err(TicketError::BadFormat(_))
        ));
        // A truncated v2 ticket is damage, not a version problem.
        let sk = SecretKey::generate();
        let bytes = postcard::to_allocvec(&sample(&sk, 0)).unwrap();
        let cut = format!(
            "{TICKET_PREFIX}{}",
            BASE32_NOPAD
                .encode(&bytes[..bytes.len() - 10])
                .to_ascii_lowercase()
        );
        assert!(matches!(
            Ticket::from_str(&cut),
            Err(TicketError::BadFormat(_))
        ));
    }

    #[test]
    fn tamper_detection() {
        let sk = SecretKey::generate();
        let t = sample(&sk, 0);
        t.verify(&sk.public()).unwrap();

        let mut t2 = t.clone();
        t2.body.put = !t2.body.put;
        assert_eq!(t2.verify(&sk.public()), Err(TicketError::BadSignature));

        let mut t3 = t.clone();
        t3.body.path = "../..".into();
        assert_eq!(t3.verify(&sk.public()), Err(TicketError::BadSignature));

        let mut t4 = t.clone();
        t4.sig[3] ^= 1;
        assert_eq!(t4.verify(&sk.public()), Err(TicketError::BadSignature));

        // Re-signed by another key but claiming the original host.
        let other = SecretKey::generate();
        let t5 = Ticket {
            body: t.body.clone(),
            sig: Ticket::sign(t.body.clone(), &other).sig,
        };
        assert_eq!(t5.verify(&sk.public()), Err(TicketError::BadSignature));

        // Valid ticket for another host.
        let t6 = sample(&other, 0);
        assert_eq!(t6.verify(&sk.public()), Err(TicketError::WrongHost));

        // Tampering inside the string form.
        let s = t.to_string();
        let mut chars: Vec<char> = s.chars().collect();
        let i = chars.len() / 2;
        chars[i] = if chars[i] == 'a' { 'b' } else { 'a' };
        let tampered: String = chars.into_iter().collect();
        if let Ok(tt) = Ticket::from_str(&tampered) {
            assert!(tt.verify(&sk.public()).is_err());
        }
    }

    #[test]
    fn expiry() {
        let sk = SecretKey::generate();
        let past = sample(&sk, now_unix() - 1);
        assert!(past.is_expired());
        assert_eq!(past.verify(&sk.public()), Err(TicketError::Expired));
        let never = sample(&sk, 0);
        assert!(!never.is_expired());
        let future = sample(&sk, now_unix() + 3600);
        assert!(!future.is_expired());
        assert!(exp_from_days(0) == 0);
        assert!(exp_from_days(7) > now_unix() + 6 * 86_400);
    }

    #[test]
    fn v2_roundtrip_with_addrs() {
        let sk = SecretKey::generate();
        let mut body = sample(&sk, 0).body;
        let hint = HostAddr {
            relay: "https://use1-1.relay.n0.iroh.link./".into(),
            addrs: vec![
                "203.0.113.7:61234".into(),
                "[2001:db8::1]:61234".into(),
                "192.168.1.20:61234".into(),
                "10.0.0.5:61234".into(),
                "[2001:db8:1234:5678:9abc:def0:1234:5678]:65535".into(),
                "198.51.100.200:1".into(),
                "172.16.200.1:40000".into(),
                "[fd00::1]:40000".into(),
            ],
        };
        body.set_host_addr(&hint);
        let t = Ticket::sign(body, &sk);
        let s = t.to_string();
        assert!(s.starts_with("toss1"));
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(s.len() < 900, "len {}", s.len());
        let back = Ticket::from_str(&s).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.body.v, 2);
        assert_eq!(back.body.host_addr(), hint);
        back.verify(&sk.public()).unwrap();

        let ea = back.endpoint_addr().unwrap();
        assert_eq!(ea.id, sk.public());
        assert_eq!(
            ea.relay_urls().next().map(|u| u.to_string()).as_deref(),
            Some("https://use1-1.relay.n0.iroh.link./")
        );
        assert_eq!(ea.ip_addrs().count(), 8);

        // The hint is signed: changing it breaks the signature.
        let mut t2 = t.clone();
        t2.body.addrs.push("1.2.3.4:5".into());
        assert_eq!(t2.verify(&sk.public()), Err(TicketError::BadSignature));
        let mut t3 = t.clone();
        t3.body.relay = "https://evil.example./".into();
        assert_eq!(t3.verify(&sk.public()), Err(TicketError::BadSignature));

        // Garbage entries are skipped when dialing, not fatal.
        let mut body = sample(&sk, 0).body;
        body.relay = "not a url".into();
        body.addrs = vec!["nope".into(), "10.1.2.3:4".into()];
        let t4 = Ticket::sign(body, &sk);
        let ea = Ticket::from_str(&t4.to_string())
            .unwrap()
            .endpoint_addr()
            .unwrap();
        assert_eq!(ea.relay_urls().count(), 0);
        assert_eq!(ea.ip_addrs().count(), 1);
    }

    #[test]
    fn v1_tickets_are_refused_clearly() {
        // A v1 ticket: same layout without relay/addrs, v = 1.
        #[derive(Serialize)]
        struct V1Body {
            v: u8,
            id: [u8; 16],
            host: [u8; 32],
            job: String,
            root: String,
            path: String,
            ret: String,
            get: bool,
            put: bool,
            admin: bool,
            exp: u64,
        }
        #[derive(Serialize)]
        struct V1Ticket {
            body: V1Body,
            #[serde(with = "sig64")]
            sig: [u8; 64],
        }
        let v1 = V1Ticket {
            body: V1Body {
                v: 1,
                id: [1; 16],
                host: [2; 32],
                job: "j".into(),
                root: "RAW".into(),
                path: String::new(),
                ret: String::new(),
                get: true,
                put: false,
                admin: false,
                exp: 0,
            },
            sig: [3; 64],
        };
        let bytes = postcard::to_allocvec(&v1).unwrap();
        let s = format!(
            "{TICKET_PREFIX}{}",
            BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
        );
        let err = Ticket::from_str(&s).unwrap_err();
        assert_eq!(err, TicketError::BadVersion(1));
        assert!(err.to_string().contains("new link"), "{err}");
    }

    #[test]
    fn host_addr_ranking() {
        let sk = SecretKey::generate();
        let ip = |s: &str| TransportAddr::Ip(s.parse().unwrap());
        let mut parts = vec![
            ip("127.0.0.1:5000"),
            ip("0.0.0.0:5000"),
            ip("169.254.1.1:5000"),
            ip("[fe80::1]:5000"),
            ip("[::1]:5000"),
            ip("192.168.1.2:5000"),
            ip("[fd12::1]:5000"),
            ip("[2001:db8::5]:5000"),
            ip("100.64.0.9:5000"),
            ip("203.0.113.9:5000"),
            TransportAddr::Relay("https://relay.example./".parse().unwrap()),
        ];
        for i in 0..10u8 {
            parts.push(ip(&format!("10.0.0.{i}:6000")));
        }
        let ha = HostAddr::from_endpoint_addr(&EndpointAddr::from_parts(sk.public(), parts));
        assert_eq!(ha.relay, "https://relay.example./");
        assert_eq!(ha.addrs.len(), MAX_TICKET_ADDRS);
        assert_eq!(ha.addrs[0], "203.0.113.9:5000");
        assert_eq!(ha.addrs[1], "[2001:db8::5]:5000");
        assert!(ha.addrs[2..].iter().all(|a| a.starts_with("10.")
            || a.starts_with("100.64.")
            || a.starts_with("192.168.")));
        assert!(!ha.addrs.iter().any(|a| a.contains("127.0.0.1")
            || a.contains("fe80")
            || a.contains("[::1]")
            || a.contains("169.254")));
        assert!(HostAddr::default().is_empty());
        assert_eq!(HostAddr::default().summary(), "relay=- direct=0");
    }

    #[test]
    fn admin_ticket() {
        let sk = SecretKey::generate();
        let t = Ticket::sign(admin_body(&sk.public(), 0, [1; 16]), &sk);
        assert!(t.body.admin && !t.body.get && !t.body.put);
        let back: Ticket = t.to_string().parse().unwrap();
        back.verify(&sk.public()).unwrap();
    }
}
