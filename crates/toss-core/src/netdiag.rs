//! NAT traversal helpers + diagnostics (WS-A).
//! Contract: `contracts/speed-v0.4.md` §2.
//!
//! - [`relay_mode`]: own relays first, n0's default relays after.
//! - [`diagnostics`]: relay, advertised addresses, port mapping, NAT facts
//!   (iroh net report), likelihood of direct connections, Vietnamese reasons.
//! - [`test_connect`]: dial a host like a transfer would, watch the path for
//!   up to [`PROBE_WINDOW`], report path / RTT / timings / reason.
//! - [`spawn_repunch`]: a client connection still on the relay after
//!   [`REPUNCH_AFTER`] gets the ticket + mDNS addresses re-added once.

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use iroh::{
    endpoint::{default_relay_mode, ConnectOptions, Connection, RelayMode},
    unstable_net_report::NetReport,
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayUrl, TransportAddr, Watcher,
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use n0_future::StreamExt;
use parking_lot::Mutex;

use crate::{
    net::{ConnectReport, Likelihood, NatInfo, NetDiag, PortmapState, DEFAULT_HOST_PORT},
    node::Inner,
    ticket::{addr_class, AddrClass, HostAddr, Ticket},
    transfer::{conn_path, ConnPath},
    ALPN_XFER,
};

/// How long [`test_connect`] waits for a direct path after connecting.
pub(crate) const PROBE_WINDOW: Duration = Duration::from_secs(8);
/// A client connection still on the relay this long after connecting is re-punched once.
pub(crate) const REPUNCH_AFTER: Duration = Duration::from_secs(10);
/// Close code/reason of probe and re-punch connections (they never send a hello).
pub(crate) const PROBE_CLOSE: &[u8] = b"probe";

// ---------------------------------------------------------------- relays

/// `RelayMode` for `NetConfig::relay_urls`: the own relays plus n0's default
/// relays as fallback (a dead own relay never breaks Toss). iroh keeps relays
/// in a map and picks the home relay by measured latency, so an own relay in
/// Vietnam wins over n0's (Singapore at best) by being closer, not by order.
/// Unparsable URLs are returned for logging; a bare host name gets `https://`.
pub(crate) fn relay_mode(urls: &[String]) -> (RelayMode, Vec<String>) {
    let map = RelayMap::empty();
    let mut bad = Vec::new();
    for raw in urls {
        let u = raw.trim();
        if u.is_empty() {
            continue;
        }
        let full = if u.contains("://") {
            u.to_string()
        } else {
            format!("https://{u}")
        };
        match full.parse::<RelayUrl>() {
            Ok(url) => {
                map.insert(url.clone(), Arc::new(RelayConfig::from(url)));
            }
            Err(_) => bad.push(raw.clone()),
        }
    }
    let n0 = default_relay_mode();
    if map.is_empty() {
        return (n0, bad);
    }
    map.extend(&n0.relay_map());
    (RelayMode::Custom(map), bad)
}

// ----------------------------------------------------------- diagnostics

/// Waits for the first net report (see [`crate::Node::wait_net_report`]).
pub(crate) async fn wait_net_report(inner: &Inner, timeout: Duration) -> bool {
    let mut w = inner.endpoint.net_report();
    tokio::time::timeout(timeout, w.initialized()).await.is_ok()
}

/// `Node::diagnostics`.
pub(crate) async fn diagnostics(inner: &Inner) -> NetDiag {
    let ep = &inner.endpoint;
    let addr = ep.addr();
    let relays = ep.home_relay_status().get();
    let connected: Vec<&RelayUrl> = relays
        .iter()
        .filter(|r| r.is_connected())
        .map(|r| r.url())
        .collect();
    let relay_connected = !connected.is_empty();
    let relay_url = connected
        .first()
        .map(|u| u.to_string())
        .or_else(|| addr.relay_urls().next().map(|u| u.to_string()))
        .unwrap_or_default();
    let custom_relay = !inner.cfg.net.relay_urls.is_empty();
    let own_relay_home = custom_relay && {
        let (mode, _) = relay_mode(&inner.cfg.net.relay_urls);
        let n0 = default_relay_mode().relay_map();
        let own: Vec<RelayUrl> = mode
            .relay_map()
            .urls::<Vec<RelayUrl>>()
            .into_iter()
            .filter(|u| !n0.contains(u))
            .collect();
        connected.iter().any(|u| own.contains(u))
    };
    let report = ep.net_report().get();
    let nat = nat_info(report.as_ref());
    let host_addr = HostAddr::from_endpoint_addr(&addr);
    let portmap = portmap_state(inner, &addr, report.as_ref()).await;
    let global_v6 = nat.global_v6.is_some()
        || addr
            .ip_addrs()
            .any(|a| matches!(addr_class(a), Some((AddrClass::GlobalV6, _))));
    // A report from a node that reached no relay measures nothing (udp=false
    // only means "no answer"): treat it like a pending one.
    let report_ready = report.is_some() && (nat.udp || nat.relay_latency_ms.is_some());
    let facts = Facts {
        report_ready,
        nat: &nat,
        portmap: &portmap,
        global_v6,
        relay_connected,
        custom_relay,
        own_relay_home,
        mdns_cfg: inner.cfg.net.mdns,
        is_host: inner.host.read().is_some(),
        port: inner.bind_port,
        notes: &inner.net_notes,
        direct_addrs: host_addr.addrs.len(),
    };
    let direct_likelihood = likelihood(&facts);
    let mut reasons = reasons(&facts, direct_likelihood);
    let lan_blocked = inner.lan_blocked.load(std::sync::atomic::Ordering::SeqCst);
    if lan_blocked {
        reasons.insert(0, crate::lanprobe::REASON.to_string());
    }
    NetDiag {
        relay_url,
        relay_connected,
        custom_relay,
        direct_addrs: host_addr.addrs,
        bind_port: inner.bind_port,
        mdns: inner.mdns.is_some(),
        portmap,
        nat,
        direct_likelihood,
        reasons,
        // v0.5: macOS Local Network denial (lanprobe.rs; always false elsewhere).
        lan_blocked,
    }
}

/// NAT facts from iroh's net report (`None` = not measured yet).
pub(crate) fn nat_info(report: Option<&NetReport>) -> NatInfo {
    let Some(r) = report else {
        return NatInfo::default();
    };
    NatInfo {
        udp: r.has_udp(),
        global_v4: r.global_v4.map(|a| a.ip().to_string()),
        global_v6: r.global_v6.map(|a| a.ip().to_string()),
        mapping_varies_by_dest: r.mapping_varies_by_dest(),
        // iroh 1.4's net report does not probe hairpinning.
        hairpin: None,
        relay_latency_ms: r
            .relay_latency
            .iter()
            .map(|(_, _, d)| d)
            .min()
            .map(|d| d.as_millis().min(u32::MAX as u128) as u32),
    }
}

/// Inputs of the likelihood / reasons rules (kept pure for tests).
#[derive(Debug)]
pub(crate) struct Facts<'a> {
    pub report_ready: bool,
    pub nat: &'a NatInfo,
    pub portmap: &'a PortmapState,
    pub global_v6: bool,
    pub relay_connected: bool,
    pub custom_relay: bool,
    pub own_relay_home: bool,
    pub mdns_cfg: bool,
    pub is_host: bool,
    pub port: Option<u16>,
    pub notes: &'a [String],
    pub direct_addrs: usize,
}

/// High: a router-mapped public address or a global IPv6 exists. Unknown:
/// the net report is pending. Low: no UDP or symmetric NAT. Medium otherwise
/// (UDP works, mapping does not vary by destination → hole punching works
/// for most editors).
pub(crate) fn likelihood(f: &Facts) -> Likelihood {
    if f.portmap.status == "mapped" || f.global_v6 {
        Likelihood::High
    } else if !f.report_ready {
        Likelihood::Unknown
    } else if !f.nat.udp || f.nat.mapping_varies_by_dest == Some(true) {
        Likelihood::Low
    } else {
        Likelihood::Medium
    }
}

/// Short Vietnamese sentences explaining [`likelihood`] and anything that
/// will push transfers onto the relay.
pub(crate) fn reasons(f: &Facts, l: Likelihood) -> Vec<String> {
    let port = f.port.unwrap_or(DEFAULT_HOST_PORT);
    let mut v: Vec<String> = f.notes.to_vec();
    if !f.relay_connected {
        v.push(
            "Chưa nối được relay — kiểm tra Internet/proxy; trong lúc này chỉ máy cùng mạng LAN \
             nối được"
                .into(),
        );
    } else if f.custom_relay && !f.own_relay_home {
        v.push("Relay riêng chưa nối được hoặc xa hơn — đang dùng relay n0 dự phòng".into());
    }
    if f.direct_addrs == 0 {
        v.push("Máy chưa có địa chỉ mạng nào để nối trực tiếp (chưa có mạng?)".into());
    }
    if !f.report_ready {
        v.push(if f.relay_connected {
            "Đang đo mạng (UDP, NAT, IP công cộng)…".into()
        } else {
            "Chưa đo được NAT/UDP vì chưa tới được relay nào".into()
        });
    } else if !f.nat.udp {
        v.push(
            "Mạng chặn UDP ra Internet — editor ở xa sẽ đi qua relay (chậm); cho phép UDP trên \
             firewall/router"
                .into(),
        );
    } else if f.nat.mapping_varies_by_dest == Some(true) && l != Likelihood::High {
        v.push(if f.is_host {
            format!(
                "NAT đối xứng: editor ở xa sẽ đi qua relay — mở cổng UDP {port} trên router \
                 (UPnP hoặc port forward) để nối trực tiếp"
            )
        } else {
            "NAT đối xứng: editor ở xa sẽ đi qua relay".into()
        });
    }
    match f.portmap.status.as_str() {
        "mapped" => v.push(format!(
            "Router đã mở cổng {}{} — editor ở xa nối trực tiếp được",
            f.portmap.external_addr.as_deref().unwrap_or("UDP"),
            f.portmap
                .protocol
                .as_deref()
                .map(|p| format!(" ({})", p.to_uppercase()))
                .unwrap_or_default()
        )),
        "unavailable" if f.is_host && l != Likelihood::High => v.push(format!(
            "Router không hỗ trợ UPnP — mở cổng UDP {port} thủ công để editor ở xa nối trực tiếp"
        )),
        "off" if f.is_host => v.push("Đã tắt mở cổng tự động (UPnP / NAT-PMP / PCP)".into()),
        _ => {}
    }
    if f.global_v6 {
        v.push("Có IPv6 công cộng — editor có IPv6 nối thẳng được".into());
    }
    if !f.mdns_cfg {
        v.push("Đã tắt mDNS — máy cùng mạng LAN chỉ tìm nhau qua địa chỉ trong link".into());
    }
    if f.is_host && f.port.is_none() {
        v.push(format!(
            "Máy tổng đang chạy cổng UDP ngẫu nhiên — đặt cổng cố định {DEFAULT_HOST_PORT} để mở \
             cổng/firewall ổn định"
        ));
    }
    if l == Likelihood::Medium && f.is_host {
        v.push("NAT dễ: đa số editor ở xa sẽ đục lỗ được để nối trực tiếp".into());
    }
    v
}

// ------------------------------------------------------------ port mapping

/// Last port-mapping protocol probe, shared by all nodes of the process
/// (same network), refreshed at most every [`PROBE_TTL`].
static PROBE_CACHE: Mutex<Option<(Instant, ProbeResult)>> = Mutex::new(None);
const PROBE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
enum ProbeResult {
    /// (upnp, pcp, nat_pmp)
    Ok(bool, bool, bool),
    Err(String),
}

impl ProbeResult {
    fn protocol(&self) -> Option<&'static str> {
        match self {
            ProbeResult::Ok(true, _, _) => Some("upnp"),
            ProbeResult::Ok(_, true, _) => Some("pcp"),
            ProbeResult::Ok(_, _, true) => Some("nat-pmp"),
            _ => None,
        }
    }
}

/// Which port-mapping protocols the gateway speaks. iroh does not expose its
/// own portmapper's probe, so this runs a separate probe-only client (it never
/// requests a mapping): UPnP SSDP search + PCP / NAT-PMP announce, ≤ ~1 s.
async fn probe_portmap() -> ProbeResult {
    if let Some((at, r)) = PROBE_CACHE.lock().clone() {
        if at.elapsed() < PROBE_TTL {
            return r;
        }
    }
    let client = portmapper::Client::default();
    let r = match tokio::time::timeout(Duration::from_secs(3), client.probe()).await {
        Ok(Ok(Ok(out))) => ProbeResult::Ok(out.upnp, out.pcp, out.nat_pmp),
        Ok(Ok(Err(e))) => ProbeResult::Err(format!("{e:#}")),
        Ok(Err(_)) => ProbeResult::Err("probe dropped".into()),
        Err(_) => ProbeResult::Err("probe timed out".into()),
    };
    tracing::debug!(?r, "port mapping probe");
    *PROBE_CACHE.lock() = Some((Instant::now(), r.clone()));
    r
}

/// The router-mapped public address among the advertised ones. iroh adds the
/// portmapper's external address to `Endpoint::addr()` but does not label it;
/// the net-report counter tells whether a mapping was ever obtained, and the
/// mapped address is the public IPv4 that differs from the QAD-observed one
/// (or the QAD one when the router preserved the port).
fn mapped_addr(
    ever_mapped: bool,
    addr: &EndpointAddr,
    report: Option<&NetReport>,
) -> Option<SocketAddr> {
    if !ever_mapped {
        return None;
    }
    let publics: Vec<SocketAddr> = addr
        .ip_addrs()
        .filter(|a| a.is_ipv4() && matches!(addr_class(a), Some((AddrClass::PublicV4, _))))
        .copied()
        .collect();
    let qad = report.and_then(|r| r.global_v4).map(SocketAddr::V4);
    publics
        .iter()
        .find(|a| Some(**a) != qad)
        .or(publics.first())
        .copied()
}

async fn portmap_state(
    inner: &Inner,
    addr: &EndpointAddr,
    report: Option<&NetReport>,
) -> PortmapState {
    if !inner.cfg.net.portmap {
        return PortmapState {
            status: "off".into(),
            external_addr: None,
            protocol: None,
        };
    }
    let m = &inner.endpoint.metrics().net_report;
    let ever_mapped = m.portmap_external_address_updated.get() > 0;
    let attempts = m.portmap_attempts.get();
    let probe = probe_portmap().await;
    if let ProbeResult::Err(e) = &probe {
        tracing::debug!("no port mapping gateway: {e}");
    }
    let protocol = probe.protocol().map(str::to_string);
    if let Some(ext) = mapped_addr(ever_mapped, addr, report) {
        return PortmapState {
            status: "mapped".into(),
            external_addr: Some(ext.to_string()),
            protocol,
        };
    }
    // A gateway that speaks a protocol usually maps within seconds of the
    // first net report; give it 30 s before calling it unavailable.
    let pending = attempts == 0 || inner.started.elapsed() < Duration::from_secs(30);
    let status = match (&probe, &protocol) {
        (ProbeResult::Ok(..), Some(_)) if pending => "probing",
        _ => "unavailable",
    };
    PortmapState {
        status: status.into(),
        external_addr: None,
        protocol,
    }
}

// ------------------------------------------------------------ test_connect

/// `Node::test_connect`: dials like a transfer (ALPN xfer/1, ticket
/// addresses), keeps the connection up to [`PROBE_WINDOW`] or until a direct
/// path is selected, then closes it without sending anything (the host sees a
/// connection that ends before its hello; no job is touched).
pub(crate) async fn test_connect(inner: &Inner, ticket: &Ticket) -> ConnectReport {
    let started = Instant::now();
    let host = ticket.host().ok();
    let conn = match inner.dial_host(ticket, ALPN_XFER).await {
        Ok(c) => c,
        Err(e) => {
            let known = match host {
                Some(id) => known_paths(&inner.endpoint, None, id).await,
                None => Vec::new(),
            };
            return ConnectReport {
                ok: false,
                path: ConnPath::Unknown,
                rtt_ms: None,
                connect_ms: ms(started.elapsed()),
                direct_after_ms: None,
                known_paths: known,
                reason: format!("Không nối được máy tổng: {e:#}"),
            };
        }
    };
    let connect_ms = ms(started.elapsed());
    let mut direct_after_ms = None;
    if conn_path(&conn) == ConnPath::Direct {
        direct_after_ms = Some(connect_ms);
    } else {
        let mut paths = conn.paths_stream();
        let deadline = tokio::time::sleep(PROBE_WINDOW);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                next = paths.next() => match next {
                    Some(list) if list.iter().any(|p| p.is_selected() && p.is_ip()) => {
                        direct_after_ms = Some(ms(started.elapsed()));
                        break;
                    }
                    Some(_) => {}
                    None => break,
                },
            }
        }
    }
    let path = conn_path(&conn);
    let rtt_ms = conn
        .paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| ms(p.rtt()) as u32);
    let known = known_paths(&inner.endpoint, Some(&conn), conn.remote_id()).await;
    let reason = if path == ConnPath::Direct {
        String::new()
    } else {
        relay_reason(inner, ticket)
    };
    conn.close(0u32.into(), PROBE_CLOSE);
    tracing::info!(
        host = %conn.remote_id().fmt_short(),
        ?path,
        ?rtt_ms,
        connect_ms,
        ?direct_after_ms,
        known = %known.join(", "),
        "test connect"
    );
    ConnectReport {
        ok: true,
        path,
        rtt_ms,
        connect_ms,
        direct_after_ms,
        known_paths: known,
        reason,
    }
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

/// Why a probe stayed on the relay, from the ticket and this side's net report.
fn relay_reason(inner: &Inner, ticket: &Ticket) -> String {
    let report = inner.endpoint.net_report().get();
    let addrs: Vec<SocketAddr> = ticket
        .body
        .addrs
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    let host_public = addrs.iter().any(|a| {
        matches!(
            addr_class(a),
            Some((AddrClass::PublicV4 | AddrClass::GlobalV6, _))
        )
    });
    if addrs.is_empty() {
        "Link không có địa chỉ trực tiếp của máy tổng (máy tổng chưa có mạng lúc tạo link) — \
         tạo link mới"
            .into()
    } else if report.as_ref().is_some_and(|r| !r.has_udp()) {
        "Mạng máy này chặn UDP ra Internet — chỉ đi được qua relay".into()
    } else if report
        .as_ref()
        .is_some_and(|r| r.mapping_varies_by_dest() == Some(true))
    {
        "Máy này sau NAT đối xứng — chỉ đi qua relay, trừ khi máy tổng mở cổng UDP (UPnP / \
         port forward)"
            .into()
    } else if !host_public {
        format!(
            "Máy tổng không có địa chỉ công cộng trong link và hai máy không cùng mạng LAN — mở \
             cổng UDP {DEFAULT_HOST_PORT} trên router của máy tổng"
        )
    } else {
        format!(
            "Chưa đục được NAT trong {} giây — kiểm tra firewall máy tổng (cho phép Toss qua \
             UDP) và mở cổng UDP {DEFAULT_HOST_PORT} trên router",
            PROBE_WINDOW.as_secs()
        )
    }
}

fn fmt_taddr(a: &TransportAddr) -> String {
    match a {
        TransportAddr::Ip(sa) => sa.to_string(),
        TransportAddr::Relay(u) => format!("relay {u}"),
        other => format!("{other:?}"),
    }
}

/// Paths of `conn` (selected first, with RTT) plus other addresses iroh
/// knows for `id`, for logs and support.
pub(crate) async fn known_paths(
    ep: &Endpoint,
    conn: Option<&Connection>,
    id: EndpointId,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(conn) = conn {
        let paths = conn.paths();
        let mut list: Vec<_> = paths.iter().collect();
        list.sort_by_key(|p| !p.is_selected());
        for p in list {
            let a = p.remote_addr().clone();
            out.push(format!(
                "{} ({}, {} ms)",
                fmt_taddr(&a),
                if p.is_selected() { "selected" } else { "open" },
                p.rtt().as_millis()
            ));
            seen.insert(a);
        }
    }
    if let Some(info) = ep.remote_info(id).await {
        for a in info.addrs() {
            if seen.insert(a.addr().clone()) {
                out.push(format!("{} ({:?})", fmt_taddr(a.addr()), a.usage()).to_lowercase());
            }
        }
    }
    out
}

// ----------------------------------------------------------------- repunch

/// Watches a client connection: if it is still not direct
/// [`REPUNCH_AFTER`] after connecting, logs `still on relay after 10s: …`
/// and re-adds the ticket addresses + what mDNS knows about the host, once.
/// Never touches the running transfer; ends when the connection closes.
///
/// iroh only consults address lookup (incl. [`iroh::address_lookup::MemoryLookup`])
/// while no path is selected — a relay path counts — so addresses that the
/// connection does not know yet are handed to it by a short extra dial
/// ("nudge", closed right after the handshake like [`test_connect`]): the
/// dial's addresses go into the per-host path state shared by all
/// connections, which triggers a new hole-punch round on the running one.
pub(crate) fn spawn_repunch(
    inner: &Inner,
    ticket: &Ticket,
    conn: &Connection,
) -> tokio::task::JoinHandle<()> {
    let ep = inner.endpoint.clone();
    let lookup = inner.lookup.clone();
    let mdns = inner.mdns.clone();
    let ticket_addr = ticket.endpoint_addr().ok();
    let weak = conn.weak_handle();
    let alpn = conn.alpn().to_vec();
    let id = conn.remote_id();
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(REPUNCH_AFTER) => {}
            _ = weak.closed() => return,
        }
        let Some(conn) = weak.upgrade() else { return };
        if conn_path(&conn) == ConnPath::Direct {
            return;
        }
        let known = known_paths(&ep, Some(&conn), id).await;
        drop(conn);
        repunch(&ep, &lookup, mdns.as_ref(), ticket_addr, id, &alpn, &known).await;
    })
}

async fn repunch(
    ep: &Endpoint,
    lookup: &iroh::address_lookup::MemoryLookup,
    mdns: Option<&MdnsAddressLookup>,
    ticket_addr: Option<EndpointAddr>,
    id: EndpointId,
    alpn: &[u8],
    known: &[String],
) {
    let host = id.fmt_short().to_string();
    let mut addrs: BTreeSet<TransportAddr> = ticket_addr.map(|a| a.addrs).unwrap_or_default();
    let mut from_mdns = 0;
    if let Some(m) = mdns {
        if let Some(a) = crate::mdns::resolve(m, id, Duration::from_secs(2)).await {
            from_mdns = a.addrs.len();
            addrs.extend(a.addrs);
        }
    }
    let current: BTreeSet<TransportAddr> = match ep.remote_info(id).await {
        Some(info) => info.into_addrs().map(|a| a.into_addr()).collect(),
        None => BTreeSet::new(),
    };
    let new: Vec<&TransportAddr> = addrs
        .iter()
        .filter(|a| !current.contains(*a) && usable(a))
        .collect();
    tracing::info!(
        %host,
        readd = addrs.len(),
        from_mdns,
        new = %new.iter().map(|a| fmt_taddr(a)).collect::<Vec<_>>().join(" "),
        "still on relay after 10s: {}",
        if known.is_empty() { "none".to_string() } else { known.join(", ") }
    );
    if addrs.is_empty() {
        return;
    }
    lookup.add_endpoint_info(EndpointAddr::from_parts(id, addrs.iter().cloned()));
    if new.is_empty() {
        // iroh already knows them all and keeps hole punching on its own.
        return;
    }
    let nudge = async {
        let connecting = ep
            .connect_with_opts(
                EndpointAddr::from_parts(id, addrs.iter().cloned()),
                alpn,
                ConnectOptions::new(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let c = connecting.await.map_err(|e| anyhow::anyhow!(e))?;
        c.close(0u32.into(), PROBE_CLOSE);
        anyhow::Ok(())
    };
    match tokio::time::timeout(Duration::from_secs(15), nudge).await {
        Ok(Ok(())) => tracing::debug!(%host, "re-punch: new addresses handed to iroh"),
        Ok(Err(e)) => tracing::debug!(%host, "re-punch dial failed: {e:#}"),
        Err(_) => tracing::debug!(%host, "re-punch dial timed out"),
    }
}

/// Only direct addresses are worth handing to iroh (it has the relay already).
fn usable(a: &TransportAddr) -> bool {
    matches!(a, TransportAddr::Ip(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(status: &str) -> PortmapState {
        PortmapState {
            status: status.into(),
            external_addr: None,
            protocol: None,
        }
    }

    fn facts<'a>(nat: &'a NatInfo, portmap: &'a PortmapState) -> Facts<'a> {
        Facts {
            report_ready: true,
            nat,
            portmap,
            global_v6: false,
            relay_connected: true,
            custom_relay: false,
            own_relay_home: false,
            mdns_cfg: true,
            is_host: true,
            port: Some(DEFAULT_HOST_PORT),
            notes: &[],
            direct_addrs: 2,
        }
    }

    #[test]
    fn likelihood_rules() {
        let easy = NatInfo {
            udp: true,
            mapping_varies_by_dest: Some(false),
            ..Default::default()
        };
        let sym = NatInfo {
            udp: true,
            mapping_varies_by_dest: Some(true),
            ..Default::default()
        };
        let no_udp = NatInfo::default();
        let unavailable = pm("unavailable");
        let mapped = pm("mapped");

        assert_eq!(likelihood(&facts(&easy, &unavailable)), Likelihood::Medium);
        assert_eq!(likelihood(&facts(&sym, &unavailable)), Likelihood::Low);
        assert_eq!(likelihood(&facts(&no_udp, &unavailable)), Likelihood::Low);
        assert_eq!(likelihood(&facts(&sym, &mapped)), Likelihood::High);
        let mut f = facts(&sym, &unavailable);
        f.global_v6 = true;
        assert_eq!(likelihood(&f), Likelihood::High);
        let mut f = facts(&easy, &unavailable);
        f.report_ready = false;
        assert_eq!(likelihood(&f), Likelihood::Unknown);

        // Reasons are Vietnamese and name the fix.
        let f = facts(&sym, &unavailable);
        let r = reasons(&f, likelihood(&f));
        assert!(r.iter().any(|s| s.starts_with("NAT đối xứng")), "{r:?}");
        assert!(
            r.iter().any(|s| s.contains("UPnP") && s.contains("41820")),
            "{r:?}"
        );
        let mut f = facts(&easy, &unavailable);
        f.relay_connected = false;
        let r = reasons(&f, likelihood(&f));
        assert!(
            r.iter().any(|s| s.starts_with("Chưa nối được relay")),
            "{r:?}"
        );
    }

    #[test]
    fn relay_mode_own_first_then_n0() {
        let (mode, bad) = relay_mode(&[
            "https://relay.example.vn".into(),
            "relay2.example.vn".into(),
            "not a url".into(),
        ]);
        assert_eq!(bad, vec!["not a url".to_string()]);
        let map = mode.relay_map();
        let urls: Vec<String> = map
            .urls::<Vec<RelayUrl>>()
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://relay.example.vn")));
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://relay2.example.vn")));
        let n0 = default_relay_mode().relay_map();
        assert_eq!(map.len(), 2 + n0.len());
        assert!(matches!(mode, RelayMode::Custom(_)));
        // Only bad URLs → n0 defaults.
        let (mode, bad) = relay_mode(&["::".into()]);
        assert_eq!(bad.len(), 1);
        assert!(!matches!(mode, RelayMode::Custom(_)));
    }
}
