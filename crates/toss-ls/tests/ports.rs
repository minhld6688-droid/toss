//! Tests that use the fixed LocalSend ports; serialized.
mod common;

use common::*;
use std::time::Duration;
use toss_ls::{LsEvent, LsOptions, DEFAULT_PORT, FALLBACK_PORTS};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn port_fallback_when_53317_is_busy() {
    let _g = SERIAL.lock().await;
    // Hold 53317 (if something else already holds it, that is just as busy).
    let _hold = std::net::TcpListener::bind(("0.0.0.0", DEFAULT_PORT));
    let mut a = node_with("A", DEFAULT_PORT, quiet()).await;
    let port = a.b.port();
    assert_ne!(port, DEFAULT_PORT);
    assert!(FALLBACK_PORTS.contains(&port), "{port}");
    match a.wait(5, |e| matches!(e, LsEvent::Port { .. })).await {
        LsEvent::Port { port: p, fallback } => {
            assert_eq!(p, port);
            assert!(fallback);
        }
        _ => unreachable!(),
    }
    // A second bridge takes the next free one; stop() frees the port again.
    let b = node_with("B", DEFAULT_PORT, quiet()).await;
    assert!(FALLBACK_PORTS.contains(&b.b.port()) && b.b.port() != port);
    a.b.stop().await;
    let c = node_with("C", port, quiet()).await;
    assert_eq!(c.b.port(), port, "stop() released the port");
    let mut c = c;
    match c.wait(5, |e| matches!(e, LsEvent::Port { .. })).await {
        LsEvent::Port { fallback, .. } => assert!(!fallback),
        _ => unreachable!(),
    }
}

/// Subnet scan (multicast-less networks). Both bridges share this host, and a scan
/// skips its own interface address, so the /24 scanned here is loopback's.
#[tokio::test]
async fn rescan_finds_a_peer_by_subnet_scan() {
    let _g = SERIAL.lock().await;
    let a = node_with("Scanned", FALLBACK_PORTS.end() - 1, quiet()).await;
    let opts = LsOptions {
        multicast: false,
        subnet_scan: true,
        scan_ports: vec![a.b.port()],
        scan_subnets: Some(vec![std::net::Ipv4Addr::new(127, 0, 0, 2)]),
        ..LsOptions::default()
    };
    let mut b = node_with("Scanner", 0, opts).await;
    tokio::time::timeout(Duration::from_secs(60), b.b.rescan())
        .await
        .expect("rescan finishes");
    let found =
        b.b.devices()
            .into_iter()
            .find(|d| d.fingerprint == a.b.fingerprint())
            .expect("found by scan");
    assert!(found.is_toss);
    assert_eq!(found.port, a.b.port());
    assert!(
        b.b.devices()
            .iter()
            .all(|d| d.fingerprint != b.b.fingerprint()),
        "self excluded"
    );
    b.wait(5, |e| matches!(e, LsEvent::DevicesChanged)).await;
}
