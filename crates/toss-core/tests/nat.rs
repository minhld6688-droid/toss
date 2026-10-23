//! v0.4 NAT traversal + diagnostics (contracts/speed-v0.4.md §2). Everything
//! here works offline: the sandbox reaches no relay, so peers are introduced
//! with loopback addresses (`Node::add_peer_addr`). Tests that need real
//! multicast are `#[ignore]`d.

use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};

use iroh::{EndpointAddr, TransportAddr};
use toss_core::{
    net::DEFAULT_HOST_PORT, ConnPath, HostAddr, HostConfig, NetConfig, Node, NodeConfig, Root,
    TicketSpec,
};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toss_core=info".into()),
        )
        .with_test_writer()
        .try_init();
}

fn cfg(dir: &Path, name: &str, net: NetConfig) -> NodeConfig {
    NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
        net,
    }
}

async fn node(dir: &Path, name: &str) -> Node {
    Node::start(cfg(dir, name, NetConfig::default()))
        .await
        .expect("start node")
}

fn loopback_addr(node: &Node) -> EndpointAddr {
    let addrs = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| TransportAddr::Ip(SocketAddr::from((Ipv4Addr::LOCALHOST, s.port()))));
    EndpointAddr::from_parts(node.endpoint_id(), addrs)
}

async fn host_with_ticket(host: &Node, dir: &Path) -> toss_core::Ticket {
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"hello").unwrap();
    host.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "RAW".into(),
            path: root,
        }],
        dir.join("ret"),
    )))
    .await
    .unwrap();
    host.create_ticket(TicketSpec {
        job: "probe".into(),
        root: "RAW".into(),
        path: String::new(),
        ret: String::new(),
        get: true,
        put: false,
        days: 1,
    })
    .unwrap()
}

fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
}

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `NetDiag` / `ConnectReport` serialize with exactly the keys of
/// `contracts/ipc.ts` (`NetDiag`, `PortmapState`, `NatInfo`, `ConnectReport`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_json_shape() {
    init_tracing();
    let dir = tempfile::tempdir().unwrap();
    let a = node(dir.path(), "a").await;
    let d = a.diagnostics().await;
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(
        keys(&v),
        set(&[
            "relay_url",
            "relay_connected",
            "custom_relay",
            "direct_addrs",
            "bind_port",
            "mdns",
            "portmap",
            "nat",
            "direct_likelihood",
            "reasons",
            "lan_blocked",
        ])
    );
    assert_eq!(
        keys(&v["portmap"]),
        set(&["status", "external_addr", "protocol"])
    );
    assert_eq!(
        keys(&v["nat"]),
        set(&[
            "udp",
            "global_v4",
            "global_v6",
            "mapping_varies_by_dest",
            "hairpin",
            "relay_latency_ms",
        ])
    );
    assert!(v["relay_url"].is_string());
    assert!(v["relay_connected"].is_boolean());
    assert!(v["direct_addrs"].is_array());
    assert!(v["bind_port"].is_null(), "client node binds a random port");
    assert!(v["reasons"].is_array());
    assert!(
        ["high", "medium", "low", "unknown"].contains(&v["direct_likelihood"].as_str().unwrap())
    );
    assert!(["off", "probing", "mapped", "unavailable"]
        .contains(&v["portmap"]["status"].as_str().unwrap()));
    // Round-trips (the shell adds firewall/log_dir on top of the same JSON).
    let back: toss_core::NetDiag = serde_json::from_value(v).unwrap();
    assert_eq!(back, d);
    // Offline sandbox: no relay → the reasons say so, in Vietnamese.
    if !d.relay_connected {
        assert!(
            d.reasons
                .iter()
                .any(|r| r.starts_with("Chưa nối được relay")),
            "{:?}",
            d.reasons
        );
    }

    // ConnectReport keys (failure path: unknown host, nothing reachable).
    let b = node(dir.path(), "b").await;
    let t = host_with_ticket(&b, dir.path()).await;
    b.shutdown().await;
    let rep = tokio::time::timeout(Duration::from_secs(60), a.test_connect(&t))
        .await
        .expect("test_connect is bounded");
    let v = serde_json::to_value(&rep).unwrap();
    assert_eq!(
        keys(&v),
        set(&[
            "ok",
            "path",
            "rtt_ms",
            "connect_ms",
            "direct_after_ms",
            "known_paths",
            "reason",
        ])
    );
    assert!(!rep.ok);
    assert!(
        rep.reason.starts_with("Không nối được máy tổng"),
        "{}",
        rep.reason
    );
    a.shutdown().await;
}

/// Two loopback nodes: the probe connects, reports Direct with an RTT and an
/// empty reason, and does not create a transfer on either side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_connect_loopback_is_direct() {
    init_tracing();
    let dir = tempfile::tempdir().unwrap();
    let host = node(dir.path(), "host").await;
    let client = node(dir.path(), "client").await;
    let ticket = host_with_ticket(&host, dir.path()).await;
    client.add_peer_addr(loopback_addr(&host));

    let rep = client.test_connect(&ticket).await;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(rep.path, ConnPath::Direct, "{rep:?}");
    assert!(rep.direct_after_ms.is_some(), "{rep:?}");
    assert!(rep.rtt_ms.is_some(), "{rep:?}");
    assert!(rep.reason.is_empty(), "{rep:?}");
    assert!(!rep.known_paths.is_empty(), "{rep:?}");
    assert!(
        rep.known_paths.iter().any(|p| p.contains("selected")),
        "{rep:?}"
    );

    // No job/transfer was started by the probe.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.transfers().is_empty());
    assert!(host.transfers().is_empty());
    host.shutdown().await;
    client.shutdown().await;
}

/// Own relay URLs (unreachable here) + mDNS/portmap off: the node starts,
/// diagnostics reflect the config, and loopback transfers still go direct.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_relay_config_starts_and_connects() {
    init_tracing();
    let dir = tempfile::tempdir().unwrap();
    let net = NetConfig {
        relay_urls: vec![
            "https://relay.toss.invalid".into(),
            "relay2.toss.invalid".into(),
            "this is not a url".into(),
        ],
        mdns: false,
        portmap: false,
        ..Default::default()
    };
    let host = Node::start(cfg(dir.path(), "host", net.clone()))
        .await
        .expect("start with own relays");
    let client = Node::start(cfg(dir.path(), "client", net))
        .await
        .expect("start client with own relays");
    let ticket = host_with_ticket(&host, dir.path()).await;

    let d = host.diagnostics().await;
    assert!(d.custom_relay);
    assert!(!d.mdns);
    assert_eq!(d.portmap.status, "off");
    assert!(
        d.reasons.iter().any(|r| r.contains("this is not a url")),
        "bad URL is reported: {:?}",
        d.reasons
    );
    assert!(
        d.reasons.iter().any(|r| r.contains("mDNS")),
        "{:?}",
        d.reasons
    );

    client.add_peer_addr(loopback_addr(&host));
    let rep = client.test_connect(&ticket).await;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(rep.path, ConnPath::Direct, "{rep:?}");
    host.shutdown().await;
    client.shutdown().await;
}

/// `Node::start_host`: binds the requested (default 41820) port, falls back to
/// a random one when it is taken and says why in the diagnostics.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_port_default_and_fallback() {
    init_tracing();
    let hc = HostConfig::new(vec![], "ret".into());
    assert_eq!(hc.effective_bind_port(), DEFAULT_HOST_PORT);
    assert_eq!(DEFAULT_HOST_PORT, 41820);

    let dir = tempfile::tempdir().unwrap();
    let net = NetConfig {
        mdns: false,
        portmap: false,
        ..Default::default()
    };
    // Free port: bound as asked.
    let probe = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let free = probe.local_addr().unwrap().port();
    drop(probe);
    let a = Node::start_host(cfg(dir.path(), "a", net.clone()), Some(free))
        .await
        .unwrap();
    assert_eq!(a.bind_port(), Some(free));
    assert_eq!(a.diagnostics().await.bind_port, Some(free));

    // Taken port: random port + reason.
    let b = Node::start_host(cfg(dir.path(), "b", net), Some(free))
        .await
        .expect("falls back to a random port");
    assert_eq!(b.bind_port(), None);
    let d = b.diagnostics().await;
    assert_eq!(d.bind_port, None);
    assert!(
        d.reasons
            .iter()
            .any(|r| r.contains(&format!("Cổng UDP {free}"))),
        "{:?}",
        d.reasons
    );
    a.shutdown().await;
    b.shutdown().await;
}

/// Tickets: public v4, global v6, then LAN v4 — and LAN addresses survive
/// even when there are more public addresses than slots.
#[test]
fn ticket_addr_order_keeps_lan() {
    let id = iroh::SecretKey::generate().public();
    let ip = |s: &str| TransportAddr::Ip(s.parse().unwrap());
    let mut parts = vec![
        ip("192.168.1.10:41820"),
        ip("10.1.2.3:41820"),
        ip("127.0.0.1:41820"),
        ip("[fe80::1]:41820"),
    ];
    for i in 0..5u8 {
        parts.push(ip(&format!("[2001:db8::{i}]:41820")));
    }
    for i in 0..5u8 {
        parts.push(ip(&format!("203.0.113.{i}:41820")));
    }
    let ha = HostAddr::from_endpoint_addr(&EndpointAddr::from_parts(id, parts));
    assert_eq!(ha.addrs.len(), toss_core::ticket::MAX_TICKET_ADDRS);
    let n = ha.addrs.len();
    assert!(ha.addrs[0].starts_with("203.0.113."), "{:?}", ha.addrs);
    assert!(ha.addrs[..n - 2]
        .iter()
        .all(|a| a.starts_with("203.") || a.starts_with("[2001:db8::")));
    assert_eq!(
        &ha.addrs[n - 2..],
        &["192.168.1.10:41820", "10.1.2.3:41820"]
    );
    let v4_before_v6 = ha.addrs.iter().position(|a| a.starts_with('[')).unwrap()
        > ha.addrs
            .iter()
            .rposition(|a| a.starts_with("203."))
            .unwrap();
    assert!(v4_before_v6, "{:?}", ha.addrs);
}

/// Same-LAN discovery: a client finds the host through mDNS alone (no ticket
/// addresses, no relay, no `add_peer_addr`).
/// Ignored: needs working multicast on the test machine (sandboxes and CI
/// containers usually drop it). Run with `cargo test -- --ignored mdns`.
#[ignore = "needs multicast (mDNS) on the local network"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mdns_finds_host_on_lan() {
    use n0_future::StreamExt;
    init_tracing();
    let dir = tempfile::tempdir().unwrap();
    let host = node(dir.path(), "host").await;
    let client = node(dir.path(), "client").await;
    assert!(host.diagnostics().await.mdns);
    let mut stream = client
        .endpoint()
        .address_lookup()
        .unwrap()
        .resolve(host.endpoint_id());
    let found = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(item) = stream.next().await {
            if let Ok(Ok(item)) = item {
                if item.provenance() == "mdns" {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(found, "host not found via mDNS");

    // A ticket without relay/addresses still connects directly (mDNS only).
    // test_connect never sends the ticket, so the edited body needs no re-signing.
    let mut ticket = host_with_ticket(&host, dir.path()).await;
    ticket.body.relay.clear();
    ticket.body.addrs.clear();
    let rep = client.test_connect(&ticket).await;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(rep.path, ConnPath::Direct, "{rep:?}");
    host.shutdown().await;
    client.shutdown().await;
}
