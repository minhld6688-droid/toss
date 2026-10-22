//! v0.4 network types (contract: `contracts/speed-v0.4.md`). Types only —
//! behaviour lives in `netdiag.rs` (WS-A: NAT traversal, diagnostics) and
//! `tuning.rs` (WS-D: QUIC transport tuning, transfer pipeline).
//! Owned by the coordinator: feature agents do not change these shapes.

use serde::{Deserialize, Serialize};

use crate::transfer::ConnPath;

/// Default fixed UDP port for host mode (port-mapping / firewall / forwarding).
pub const DEFAULT_HOST_PORT: u16 = 41820;

/// Network settings of a node. Every field has a default so old
/// `settings.json` / `NodeConfig` JSON keeps working.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NetConfig {
    /// Own relay URLs, tried first (e.g. `https://relay.example.vn`). Empty =
    /// n0 default relays only. When non-empty the n0 relays stay as fallback.
    pub relay_urls: Vec<String>,
    /// mDNS local-network discovery (same LAN without relay or ticket addrs).
    pub mdns: bool,
    /// UPnP-IGD / NAT-PMP / PCP port mapping (iroh's portmapper).
    pub portmap: bool,
    /// QUIC + pipeline tuning.
    pub tuning: Tuning,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            relay_urls: Vec::new(),
            mdns: true,
            portmap: true,
            tuning: Tuning::default(),
        }
    }
}

/// Congestion controller.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Congestion {
    /// BBR (BBRv3 in noq). Opt-in until it beats Cubic on a real LAN/WAN A/B.
    Bbr,
    /// Cubic (default since v0.4: faster than BBR on every v0.4 bench).
    #[default]
    Cubic,
}

/// Transfer tuning. Defaults per `contracts/speed-v0.4.md` §4.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Tuning {
    /// Per-stream receive window, MiB (default 2; capped at 2 by `tuning.rs`, see CHANGES.md).
    pub stream_window_mb: u32,
    /// Connection send window, MiB (default 64).
    pub send_window_mb: u32,
    /// Congestion controller (default Cubic).
    pub congestion: Congestion,
    /// Data streams in flight per transfer (default 8).
    pub streams: u32,
    /// Streams in flight when RTT > 30 ms or the path is relay (default 16).
    pub streams_high_rtt: u32,
    /// Read/write chunk, KiB (default 1024).
    pub chunk_kb: u32,
    /// Files larger than this (MiB) are split into ranges on several streams (default 512).
    pub range_split_mb: u32,
    /// Files smaller than this (KiB) are batched several per stream (default 1024).
    pub small_file_kb: u32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            stream_window_mb: 2,
            send_window_mb: 64,
            congestion: Congestion::Cubic,
            streams: 8,
            streams_high_rtt: 16,
            chunk_kb: 1024,
            range_split_mb: 512,
            small_file_kb: 1024,
        }
    }
}

/// How likely editors will reach this host directly (not via relay).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Likelihood {
    /// A public mapped/forwarded port or global IPv6 is reachable.
    High,
    /// Hole punching should work for most editors (easy NAT).
    Medium,
    /// Symmetric NAT / no UDP / no relay: expect relay.
    Low,
    /// Not measured yet (net report pending).
    Unknown,
}

/// Port-mapping state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortmapState {
    /// "off" | "probing" | "mapped" | "unavailable"
    pub status: String,
    /// Public `ip:port` the router maps to us, when mapped.
    pub external_addr: Option<String>,
    /// Which protocol got the mapping ("upnp" | "pcp" | "nat-pmp"), when known.
    pub protocol: Option<String>,
}

/// NAT facts from iroh's net report.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NatInfo {
    /// UDP works at all.
    pub udp: bool,
    /// Public IPv4 as seen by relays/QAD.
    pub global_v4: Option<String>,
    /// Public IPv6, if any.
    pub global_v6: Option<String>,
    /// `Some(true)` = symmetric NAT (mapping varies by destination) → hole punching unlikely.
    pub mapping_varies_by_dest: Option<bool>,
    /// Router supports hairpinning.
    pub hairpin: Option<bool>,
    /// Lowest relay latency, ms.
    pub relay_latency_ms: Option<u32>,
}

/// Snapshot of this node's network situation (`Node::diagnostics`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetDiag {
    /// Home relay URL ("" = none yet).
    pub relay_url: String,
    /// Connected to the home relay.
    pub relay_connected: bool,
    /// Own relay configured (`NetConfig::relay_urls` non-empty).
    pub custom_relay: bool,
    /// Direct addresses as advertised (best first: mapped public, v6, v4, LAN).
    pub direct_addrs: Vec<String>,
    /// UDP port bound (None = random).
    pub bind_port: Option<u16>,
    /// mDNS running.
    pub mdns: bool,
    /// Port mapping.
    pub portmap: PortmapState,
    /// NAT facts.
    pub nat: NatInfo,
    /// Summary for hosts.
    pub direct_likelihood: Likelihood,
    /// Human-readable reasons, Vietnamese, one short sentence each.
    pub reasons: Vec<String>,
}

/// Result of `Node::test_connect`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConnectReport {
    /// Connected at all.
    pub ok: bool,
    /// Path after the probe window (direct preferred).
    pub path: ConnPath,
    /// Round-trip time on the selected path, ms.
    pub rtt_ms: Option<u32>,
    /// Time to connect, ms.
    pub connect_ms: u64,
    /// Time until a direct path was selected, ms (None = never during the probe).
    pub direct_after_ms: Option<u64>,
    /// Remote addresses iroh knew (for logs/support).
    pub known_paths: Vec<String>,
    /// Why it stayed on relay / failed, Vietnamese; empty when direct.
    pub reason: String,
}
