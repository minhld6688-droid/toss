mod common;

use common::*;
use std::time::Duration;
use toss_ls::{LsEvent, LsOptions};

/// The known-address path (what multicast-less networks and "add by IP" use).
#[tokio::test]
async fn bridges_find_each_other_by_address() {
    let mut a = node("Máy tổng").await;
    let b = node("Editor").await;
    assert!(a.b.devices().is_empty());
    let d =
        a.b.discover_addr("127.0.0.1", b.b.port())
            .await
            .expect("found");
    assert_eq!(d.fingerprint, b.b.fingerprint());
    assert_eq!(d.alias, "Editor");
    assert!(d.is_toss);
    assert_eq!(d.model.as_deref(), Some("Toss"));
    assert_eq!(d.kind, "desktop");
    assert!(d.https);
    assert_eq!(d.port, b.b.port());
    assert!(d.last_seen > 0);
    a.wait(5, |e| matches!(e, LsEvent::DevicesChanged)).await;
    // b learned a from the register request, self never listed.
    for _ in 0..50 {
        if !b.b.devices().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let bd = b.b.devices();
    assert_eq!(bd.len(), 1);
    assert_eq!(bd[0].fingerprint, a.b.fingerprint());
    assert!(a
        .b
        .devices()
        .iter()
        .all(|d| d.fingerprint != a.b.fingerprint()));
    // Probing yourself finds nothing.
    assert!(a.b.discover_addr("127.0.0.1", a.b.port()).await.is_none());
    // Nobody there.
    assert!(a.b.discover_addr("127.0.0.1", 1).await.is_none());
}

#[tokio::test]
async fn fingerprint_and_cert_persist() {
    let a = node("A").await;
    let fp = a.b.fingerprint();
    assert_eq!(fp.len(), 64);
    assert!(a.data.path().join("ls/cert.pem").exists());
    assert!(a.data.path().join("ls/key.pem").exists());
    a.b.stop().await;
    let (b, _rx) = toss_ls::LsBridge::start_with(
        toss_ls::LsConfig {
            alias: "A".into(),
            data_dir: a.data.path().into(),
            dest_dir: a.inbox.path().into(),
            port: 0,
        },
        quiet(),
    )
    .await
    .unwrap();
    assert_eq!(b.fingerprint(), fp);
    b.stop().await;
}

#[tokio::test]
async fn set_alias_is_seen_by_peers() {
    let a = node("Tên cũ").await;
    let b = node("B").await;
    link(&a, &b).await;
    a.b.set_alias("Tên mới").await;
    let d =
        b.b.discover_addr("127.0.0.1", a.b.port())
            .await
            .expect("found");
    assert_eq!(d.alias, "Tên mới");
}

/// Multicast announcements between two bridges on this host. Loopback multicast
/// is often unavailable in containers/CI, so this only runs on demand.
#[tokio::test]
#[ignore = "needs a network interface that loops back multicast (not available in CI containers)"]
async fn bridges_find_each_other_by_multicast() {
    let opts = LsOptions {
        multicast: true,
        multicast_port: 55327,
        subnet_scan: false,
        ..LsOptions::default()
    };
    let a = node_with("A", 0, opts.clone()).await;
    let b = node_with("B", 0, opts).await;
    b.b.rescan().await; // announce burst; a answers with a register request
    let fp = b.b.fingerprint();
    for _ in 0..100 {
        if a.b.devices().iter().any(|d| d.fingerprint == fp) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("not discovered via multicast");
}
