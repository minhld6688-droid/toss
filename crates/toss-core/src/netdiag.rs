//! NAT traversal + diagnostics (WS-A). Scaffold: real implementation pending.
//! Contract: `contracts/speed-v0.4.md` §2–§3.

use crate::{
    net::{ConnectReport, Likelihood, NatInfo, NetDiag, PortmapState},
    node::Inner,
    ticket::Ticket,
    transfer::ConnPath,
};

/// `Node::diagnostics`.
pub(crate) async fn diagnostics(inner: &Inner) -> NetDiag {
    let addr = inner.endpoint.addr();
    let relay_url = addr
        .relay_urls()
        .next()
        .map(|u| u.to_string())
        .unwrap_or_default();
    let direct_addrs = addr.ip_addrs().map(|a| a.to_string()).collect();
    NetDiag {
        relay_url,
        relay_connected: false,
        custom_relay: !inner.cfg.net.relay_urls.is_empty(),
        direct_addrs,
        bind_port: inner.bind_port,
        mdns: false,
        portmap: PortmapState {
            status: "off".into(),
            external_addr: None,
            protocol: None,
        },
        nat: NatInfo::default(),
        direct_likelihood: Likelihood::Unknown,
        reasons: vec![],
    }
}

/// `Node::test_connect`.
pub(crate) async fn test_connect(inner: &Inner, ticket: &Ticket) -> ConnectReport {
    let started = std::time::Instant::now();
    match inner.dial_host(ticket, crate::ALPN_XFER).await {
        Ok(conn) => {
            let path = crate::transfer::conn_path(&conn);
            conn.close(0u32.into(), b"probe");
            ConnectReport {
                ok: true,
                path,
                rtt_ms: None,
                connect_ms: started.elapsed().as_millis() as u64,
                direct_after_ms: None,
                known_paths: vec![],
                reason: String::new(),
            }
        }
        Err(e) => ConnectReport {
            ok: false,
            path: ConnPath::Unknown,
            rtt_ms: None,
            connect_ms: started.elapsed().as_millis() as u64,
            direct_after_ms: None,
            known_paths: vec![],
            reason: format!("{e:#}"),
        },
    }
}
