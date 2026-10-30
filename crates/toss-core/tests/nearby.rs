//! v0.8 "Gần đây" (contracts/nearby-v0.8.md §1): the nearby store, pushes and
//! text to devices that are not contacts (ask / "always" pairing both sides),
//! the receive policy matrix, resend, single-file tickets, Vietnamese errors
//! and compatibility with a Toss 0.6 peer.
//!
//! Two nodes in one process do not reliably see each other's mDNS multicast in
//! CI containers, so these tests run with `net.mdns = false` and fill the
//! nearby store with `toss_core::testing::add_nearby` (what the mDNS
//! subscription does on `Discovered`); dialing then uses the loopback
//! addresses of the injected entry, as it would use the LAN addresses mDNS saw.

use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};

use iroh::{EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};
use toss_core::{
    transfer::{Side, Status},
    HostConfig, NearbyPeer, Node, NodeConfig, ReceivePolicy, Root, TicketSpec, TransferEvent,
    Tuning,
};

const MIB: usize = 1024 * 1024;

fn init() {
    toss_core::testing::set_timeouts(Duration::from_secs(10), Duration::from_secs(15));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toss_core=info".into()),
        )
        .with_test_writer()
        .try_init();
}

async fn node(dir: &Path, name: &str) -> Node {
    let mut cfg = NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "0.8.0-test".into(),
        net: Default::default(),
    };
    cfg.net.mdns = false;
    cfg.net.tuning = Tuning {
        range_split_mb: 1,
        small_file_kb: 64,
        streams: 4,
        streams_high_rtt: 8,
        ..Tuning::default()
    };
    let n = Node::start(cfg).await.expect("start node");
    n.set_inbox_dir(dir.join(format!("{name}-inbox")));
    n
}

fn loopback(ep: &iroh::Endpoint) -> Vec<SocketAddr> {
    ep.bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| SocketAddr::from((Ipv4Addr::LOCALHOST, s.port())))
        .collect()
}

fn loopback_addr(node: &Node) -> EndpointAddr {
    EndpointAddr::from_parts(
        node.endpoint_id(),
        loopback(node.endpoint()).into_iter().map(TransportAddr::Ip),
    )
}

fn hex_id(n: &Node) -> String {
    hex::encode(n.endpoint_id().as_bytes())
}

/// The nearby entry mDNS would produce for `n` (its loopback addresses).
fn seen(n: &Node) -> NearbyPeer {
    NearbyPeer {
        id: hex_id(n),
        name: n.device_name().to_string(),
        platform: std::env::consts::OS.to_string(),
        version: "0.8.0-test".into(),
        addrs: loopback(n.endpoint())
            .into_iter()
            .map(|a| a.to_string())
            .collect(),
        last_seen: 0,
        contact: false,
    }
}

/// `a` and `b` see each other on the "LAN".
fn neighbours(a: &Node, b: &Node) {
    toss_core::testing::add_nearby(a, seen(b));
    toss_core::testing::add_nearby(b, seen(a));
}

fn write_data(path: &Path, seed: u64, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut left = len;
    let mut block = vec![0u8; 64 * 1024];
    while left > 0 {
        for b in block.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        let k = left.min(block.len());
        f.write_all(&block[..k]).unwrap();
        left -= k;
    }
    f.flush().unwrap();
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn rec(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                rec(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy();
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    rec(dir, dir, &mut out);
    out.sort();
    out
}

fn assert_same_tree(src: &Path, dst: &Path) {
    let a = files_under(src);
    assert!(!a.is_empty());
    assert_eq!(a, files_under(dst), "file lists differ");
    for rel in a {
        assert!(
            std::fs::read(src.join(&rel)).unwrap() == std::fs::read(dst.join(&rel)).unwrap(),
            "{rel} differs"
        );
    }
}

async fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_final(t: &toss_core::Transfer) -> toss_core::TransferState {
    tokio::time::timeout(Duration::from_secs(120), t.wait())
        .await
        .expect("transfer timed out")
}

/// Next event matching `f` within 30 s.
async fn next_event<T>(
    rx: &mut tokio::sync::broadcast::Receiver<TransferEvent>,
    mut f: impl FnMut(TransferEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(v) = f(ev) {
                        return v;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event channel: {e}"),
            }
        }
    })
    .await
    .expect("event not seen")
}

fn incoming(ev: TransferEvent) -> Option<toss_core::IncomingPush> {
    match ev {
        TransferEvent::PushIncoming(p) => Some(p),
        _ => None,
    }
}

async fn pair(a: &Node, b: &Node) {
    a.add_peer_addr(loopback_addr(b));
    b.add_peer_addr(loopback_addr(a));
    let inv = a.create_invite(7, "").unwrap();
    b.accept_invite(&inv.invite).await.expect("pair");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nearby_store_events_contacts_and_name() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let c = node(tmp.path(), "Editor C").await;
    assert!(a.nearby().is_empty(), "mDNS off: empty store");

    let mut ev = a.subscribe();
    toss_core::testing::add_nearby(&a, seen(&b));
    next_event(&mut ev, |e| {
        matches!(e, TransferEvent::NearbyChanged {}).then_some(())
    })
    .await;
    let json = serde_json::to_value(TransferEvent::NearbyChanged {}).unwrap();
    assert_eq!(json, serde_json::json!({"type": "nearby_changed"}));
    let list = a.nearby();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, hex_id(&b));
    assert_eq!(list[0].name, "Editor B");
    assert_eq!(list[0].version, "0.8.0-test");
    assert!(!list[0].contact);
    assert!(list[0].last_seen > 0);
    assert!(list[0].addrs[0].starts_with("127.0.0.1:"));
    let json = serde_json::to_value(&list[0]).unwrap();
    for k in [
        "id",
        "name",
        "platform",
        "version",
        "addrs",
        "last_seen",
        "contact",
    ] {
        assert!(json.get(k).is_some(), "NearbyPeer JSON lacks {k}");
    }

    // Self is never listed; an entry without user data (Toss < 0.8) gets a name.
    toss_core::testing::add_nearby(&a, seen(&a));
    let mut old = seen(&c);
    old.name.clear();
    old.version.clear();
    toss_core::testing::add_nearby(&a, old);
    let list = a.nearby();
    assert_eq!(list.len(), 2, "{list:?}");
    let oc = list.iter().find(|p| p.id == hex_id(&c)).unwrap();
    assert_eq!(oc.name, format!("Máy {}", &hex_id(&c)[..6]));
    assert_eq!(oc.version, "");

    // Expired / stale entries go.
    toss_core::testing::expire_nearby(&a, &hex_id(&c));
    assert_eq!(a.nearby().len(), 1);
    toss_core::testing::age_nearby(&a, &hex_id(&b), 120);
    assert_eq!(a.nearby().len(), 1, "2 min old stays");
    toss_core::testing::age_nearby(&a, &hex_id(&b), 181);
    assert!(a.nearby().is_empty(), "not seen for 3 min: dropped");

    // A contact seen nearby is online at once (no 60 s presence lag).
    pair(&a, &b).await;
    let bid = hex_id(&b);
    let b_data = tmp.path().join("Editor B-data");
    b.shutdown().await;
    a.refresh_presence().await;
    let on_a = |a: &Node| a.contacts().into_iter().find(|x| x.id == bid).unwrap();
    assert!(!on_a(&a).online);
    let mut cfg = NodeConfig {
        data_dir: b_data,
        device_name: "Editor B".into(),
        app_version: "0.8.0-test".into(),
        net: Default::default(),
    };
    cfg.net.mdns = false;
    let b2 = Node::start(cfg).await.unwrap();
    let mut ev = a.subscribe();
    toss_core::testing::add_nearby(&a, seen(&b2));
    assert!(
        on_a(&a).online,
        "contact seen nearby must be online immediately"
    );
    next_event(&mut ev, |e| match e {
        TransferEvent::ContactPresence { id, online, .. } if id == bid => Some(online),
        _ => None,
    })
    .await;
    let list = a.nearby();
    assert!(list[0].contact);

    // Rename: device_name, advertised user data and what contacts see.
    b2.set_device_name("  Máy Đạt;mới  ");
    assert_eq!(b2.device_name(), "Máy Đạt;mới");
    assert_eq!(
        toss_core::testing::advertised_user_data(&b2),
        format!("n=Máy Đạt,mới;p={};v=0.8.0-test", std::env::consts::OS)
    );
    b2.set_device_name("   ");
    assert_eq!(b2.device_name(), "Máy Đạt;mới", "empty names are ignored");
    b2.add_peer_addr(loopback_addr(&a));
    a.add_peer_addr(loopback_addr(&b2));
    b2.refresh_presence().await;
    wait_until("new name on the contact", Duration::from_secs(10), || {
        on_a(&a).their_name == "Máy Đạt;mới"
    })
    .await;
    for n in [a, b2, c] {
        n.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stranger_push_ask_decline_and_always_pairs_both_sides() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let (aid, bid) = (hex_id(&a), hex_id(&b));
    let job = tmp.path().join("work/12A3");
    write_data(&job.join("clip.mov"), 1, 2 * MIB + 77);
    write_data(&job.join("sub/a.jpg"), 2, 30_000);

    // Not a contact and not nearby (on either side): refused.
    let err = b.send_to_peer(&aid, vec![job.clone()]).await.unwrap_err();
    assert!(format!("{err:#}").contains("mạng LAN"), "{err:#}");
    toss_core::testing::add_nearby(&b, seen(&a));
    let t = b.send_to_peer(&aid, vec![job.clone()]).await.unwrap();
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Error, "{st:?}");
    assert!(
        st.error
            .as_deref()
            .unwrap_or("")
            .contains("không ở cùng mạng LAN"),
        "{st:?}"
    );

    // Nearby on both sides: the receiver is asked (stranger).
    toss_core::testing::add_nearby(&a, seen(&b));
    let mut a_ev = a.subscribe();
    let t = b.send_to_peer(&aid, vec![job.clone()]).await.unwrap();
    assert_eq!(
        b.transfer(&t.id).unwrap().contact.as_deref(),
        Some(aid.as_str())
    );
    let p = next_event(&mut a_ev, incoming).await;
    assert!(p.stranger);
    assert_eq!(p.from_id, bid);
    assert_eq!(p.from_name, "Editor B");
    assert_eq!(p.job, "12A3");
    assert_eq!(p.files, 2);
    let json = serde_json::to_value(TransferEvent::PushIncoming(p.clone())).unwrap();
    assert_eq!(json["stranger"], true);
    a.answer_push(&p.push_id, false, false).unwrap();
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Error);
    assert_eq!(st.error.as_deref(), Some("người nhận đã từ chối"));
    assert!(a.contacts().is_empty() && b.contacts().is_empty());

    // Accept once (no "always"): lands in <inbox>/<sender name>/<job>/, still strangers.
    let t = b.send_to_peer(&aid, vec![job.clone()]).await.unwrap();
    let p = next_event(&mut a_ev, incoming).await;
    a.answer_push(&p.push_id, true, false).unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    let landed = a.inbox_dir().join("Editor B/12A3");
    assert_same_tree(&job, &landed);
    let dest = next_event(&mut a_ev, |e| match e {
        TransferEvent::Done { dest, .. } => Some(dest),
        _ => None,
    })
    .await;
    let dest = std::path::PathBuf::from(dest.expect("receiver Done carries dest"));
    assert_eq!(
        toss_core::paths::canonicalize(&dest).unwrap(),
        toss_core::paths::canonicalize(&landed).unwrap()
    );
    assert!(a.contacts().is_empty() && b.contacts().is_empty());
    let host_side = a
        .transfers()
        .into_iter()
        .find(|s| s.side == Side::Host)
        .unwrap();
    assert_eq!(host_side.peer, "Editor B");
    assert_eq!(host_side.contact.as_deref(), Some(bid.as_str()));

    // "Luôn nhận từ máy này": both sides become contacts (auto-accept).
    let f = tmp.path().join("work/note.txt");
    std::fs::write(&f, b"hello").unwrap();
    let mut b_ev = b.subscribe();
    let t = b.send_to_peer(&aid, vec![f.clone()]).await.unwrap();
    let p = next_event(&mut a_ev, incoming).await;
    a.answer_push(&p.push_id, true, true).unwrap();
    let on_a = a.contacts();
    assert_eq!(on_a.len(), 1, "added at once by answer_push");
    assert_eq!(on_a[0].id, bid);
    assert_eq!(on_a[0].name, "Editor B");
    assert!(on_a[0].auto_accept);
    assert_eq!(wait_final(&t).await.status, Status::Done);
    assert_eq!(
        std::fs::read(a.inbox_dir().join("Editor B/1 file/note.txt")).unwrap(),
        b"hello"
    );
    next_event(&mut b_ev, |e| {
        matches!(e, TransferEvent::ContactsChanged {}).then_some(())
    })
    .await;
    let on_b = b.contacts();
    assert_eq!(on_b.len(), 1, "sender paired back");
    assert_eq!(on_b[0].id, aid);
    assert_eq!(on_b[0].name, "Studio");
    assert!(on_b[0].auto_accept);
    assert!(b.nearby()[0].contact);

    // Now a contact: plain Push, accepted without asking.
    let t = b.send_to_peer(&aid, vec![job.clone()]).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    assert!(a.incoming_pushes().is_empty());
    // A contact that is not nearby still works (presence path).
    toss_core::testing::expire_nearby(&b, &aid);
    toss_core::testing::expire_nearby(&a, &bid);
    a.add_peer_addr(loopback_addr(&b));
    b.add_peer_addr(loopback_addr(&a));
    let t = b.send_to_contact(&aid, vec![f.clone()]).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    a.shutdown().await;
    b.shutdown().await;
}

/// Sends a small file from `from` to `to` and reports whether `to` asked its
/// user (then accepts) or accepted at once.
async fn asked(from: &Node, to: &Node, f: &Path) -> bool {
    let mut ev = to.subscribe();
    let t = from
        .send_to_peer(&hex_id(to), vec![f.to_path_buf()])
        .await
        .unwrap();
    let res = tokio::select! {
        st = t.wait() => {
            assert_eq!(st.status, Status::Done, "{st:?}");
            false
        }
        p = next_event(&mut ev, incoming) => {
            to.answer_push(&p.push_id, true, false).unwrap();
            assert_eq!(wait_final(&t).await.status, Status::Done);
            true
        }
    };
    assert!(to.incoming_pushes().is_empty());
    res
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receive_policy_matrix() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Receiver").await;
    let c = node(tmp.path(), "Contact auto").await;
    let m = node(tmp.path(), "Contact manual").await;
    let s = node(tmp.path(), "Stranger").await;
    pair(&a, &c).await;
    pair(&a, &m).await;
    a.set_contact_auto_accept(&hex_id(&m), false).unwrap();
    neighbours(&a, &s);
    let f = tmp.path().join("x.bin");
    write_data(&f, 5, 10_000);
    assert_eq!(a.receive_policy(), ReceivePolicy::Contacts, "default");
    // (policy, [contact auto, contact manual, stranger]) -> asked?
    for (policy, want) in [
        (ReceivePolicy::Contacts, [false, true, true]),
        (ReceivePolicy::Off, [true, true, true]),
        (ReceivePolicy::Everyone, [false, true, false]),
    ] {
        a.set_receive_policy(policy);
        assert_eq!(a.receive_policy(), policy);
        let got = [
            asked(&c, &a, &f).await,
            asked(&m, &a, &f).await,
            asked(&s, &a, &f).await,
        ];
        assert_eq!(got, want, "{policy:?}");
    }
    // "Everyone" does not add strangers to contacts.
    assert_eq!(a.contacts().len(), 2);
    // `always` on a contact turns its auto-accept on.
    a.set_receive_policy(ReceivePolicy::Contacts);
    let mut ev = a.subscribe();
    let t = m.send_to_peer(&hex_id(&a), vec![f.clone()]).await.unwrap();
    let p = next_event(&mut ev, incoming).await;
    assert!(!p.stranger);
    a.answer_push(&p.push_id, true, true).unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    assert!(a.contacts().iter().all(|c| c.auto_accept));
    assert!(!asked(&m, &a, &f).await);
    for n in [a, c, m, s] {
        n.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn text_messages() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let s = node(tmp.path(), "Stranger").await;
    let c = node(tmp.path(), "Contact").await;
    let aid = hex_id(&a);

    // Not nearby: refused on the sender, and on the receiver side too.
    let err = s.send_text(&aid, "chào").await.unwrap_err();
    assert!(format!("{err:#}").contains("mạng LAN"), "{err:#}");
    toss_core::testing::add_nearby(&s, seen(&a));
    let err = s.send_text(&aid, "chào").await.unwrap_err();
    assert!(
        format!("{err:#}").contains("không ở cùng mạng LAN"),
        "{err:#}"
    );

    // Nearby stranger: shown (policy does not matter for text).
    toss_core::testing::add_nearby(&a, seen(&s));
    for policy in [ReceivePolicy::Off, ReceivePolicy::Contacts] {
        a.set_receive_policy(policy);
        let mut ev = a.subscribe();
        s.send_text(&aid, "Ảnh lớp 12A3:\nhttps://x.example/a\u{1b}[31m")
            .await
            .unwrap();
        let (from_id, from_name, text, at) = next_event(&mut ev, |e| match e {
            TransferEvent::TextIncoming {
                from_id,
                from_name,
                text,
                at,
            } => Some((from_id, from_name, text, at)),
            _ => None,
        })
        .await;
        assert_eq!(from_id, hex_id(&s));
        assert_eq!(from_name, "Stranger");
        assert_eq!(
            text, "Ảnh lớp 12A3:\nhttps://x.example/a[31m",
            "newline kept, ESC dropped"
        );
        assert!(at > 0);
    }
    let json = serde_json::to_value(TransferEvent::TextIncoming {
        from_id: "x".into(),
        from_name: "y".into(),
        text: "z".into(),
        at: 1,
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type": "text_incoming", "from_id": "x", "from_name": "y", "text": "z", "at": 1})
    );

    // Contact, not nearby: shown with the contact name.
    pair(&a, &c).await;
    a.rename_contact(&hex_id(&c), "Máy Đạt").unwrap();
    let mut ev = a.subscribe();
    c.send_text(&aid, "xong rồi").await.unwrap();
    let name = next_event(&mut ev, |e| match e {
        TransferEvent::TextIncoming { from_name, .. } => Some(from_name),
        _ => None,
    })
    .await;
    assert_eq!(name, "Máy Đạt");

    // Limits.
    let max = "a".repeat(60 * 1024);
    c.send_text(&aid, &max).await.unwrap();
    let err = c.send_text(&aid, &format!("{max}b")).await.unwrap_err();
    assert!(format!("{err:#}").contains("quá dài"), "{err:#}");
    assert!(c.send_text(&aid, "  ").await.is_err());
    assert!(c.send_text("zz", "x").await.is_err());
    for n in [a, s, c] {
        n.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resend_push_and_ticket_put() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    neighbours(&a, &b);
    a.set_receive_policy(ReceivePolicy::Everyone);
    let job = tmp.path().join("work/Job");
    write_data(&job.join("a.bin"), 1, MIB + 3);

    // Push to a nearby device, then resend after a new file appeared.
    let t = b
        .send_to_peer(&hex_id(&a), vec![job.clone()])
        .await
        .unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    write_data(&job.join("b.bin"), 2, 5000);
    let t2 = b.resend(&t.id).await.unwrap();
    assert_ne!(t2.id, t.id);
    let st = wait_final(&t2).await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_eq!(st.job, "Job");
    assert_eq!(st.contact.as_deref(), Some(hex_id(&a).as_str()));
    assert_same_tree(&job, &a.inbox_dir().join("Editor B/Job"));

    // Receive side / unknown ids cannot be resent (Vietnamese errors).
    let host_side = a
        .transfers()
        .into_iter()
        .find(|s| s.side == Side::Host)
        .unwrap();
    let err = a.resend(&host_side.id).await.unwrap_err();
    assert!(format!("{err:#}").contains("gửi đi"), "{err:#}");
    let err = a.resend(&"nope".to_string()).await.unwrap_err();
    assert_eq!(format!("{err:#}"), "Không tìm thấy lượt chuyển này");

    // Ticket put: same paths, same link.
    a.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "RAW".into(),
            path: tmp.path().join("work"),
        }],
        tmp.path().join("return"),
    )))
    .await
    .unwrap();
    let ticket = a
        .create_ticket(TicketSpec {
            job: "Upload".into(),
            root: "RAW".into(),
            path: String::new(),
            ret: String::new(),
            get: false,
            put: true,
            days: 1,
        })
        .unwrap();
    b.add_peer_addr(loopback_addr(&a));
    let f = tmp.path().join("up.txt");
    std::fs::write(&f, b"v1").unwrap();
    let t = b.put(ticket.clone(), vec![f.clone()]).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    std::fs::write(&f, b"v2").unwrap();
    let t2 = b.resend(&t.id).await.unwrap();
    assert_eq!(wait_final(&t2).await.status, Status::Done);
    let ret = tmp.path().join("return/Upload");
    assert_eq!(files_under(&ret), vec!["up (2).txt", "up.txt"]);
    assert_eq!(std::fs::read(ret.join("up (2).txt")).unwrap(), b"v2");
    // The source is gone: Vietnamese error, nothing started.
    std::fs::remove_file(&f).unwrap();
    let err = b.resend(&t.id).await.unwrap_err();
    assert!(format!("{err:#}").contains("Không đọc được"), "{err:#}");
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_file_ticket_get() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let host = node(tmp.path(), "host").await;
    let client = node(tmp.path(), "client").await;
    let photos = tmp.path().join("photos");
    write_data(&photos.join("IMG_0001.CR3"), 3, 3 * MIB + 5);
    write_data(&photos.join("other.jpg"), 4, 1000);

    // Quick share of a file: parent folder as root, the file as path.
    let st = toss_core::paths::share_target(&photos.join("IMG_0001.CR3")).unwrap();
    assert!(st.is_file);
    assert_eq!(st.path, "IMG_0001.CR3");
    assert_eq!(st.job, "IMG_0001");
    assert_eq!(st.root, toss_core::paths::canonicalize(&photos).unwrap());
    let sf = toss_core::paths::share_target(&photos).unwrap();
    assert!(!sf.is_file && sf.path.is_empty() && sf.job == "photos");
    let err = toss_core::paths::share_target(&photos.join("nope")).unwrap_err();
    assert!(format!("{err:#}").contains("Không tìm thấy"), "{err:#}");

    host.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "qs-1".into(),
            path: st.root.clone(),
        }],
        tmp.path().join("return"),
    )))
    .await
    .unwrap();
    let ticket = host
        .create_ticket(TicketSpec {
            job: st.job.clone(),
            root: "qs-1".into(),
            path: st.path.clone(),
            ret: String::new(),
            get: true,
            put: false,
            days: 7,
        })
        .unwrap();
    let ticket: toss_core::Ticket = ticket.to_string().parse().unwrap();
    client.add_peer_addr(loopback_addr(&host));
    let dest = tmp.path().join("dest");
    let t = client.get(ticket.clone(), dest.clone()).await.unwrap();
    let s = wait_final(&t).await;
    assert_eq!(s.status, Status::Done, "{s:?}");
    assert_eq!((s.files_total, s.bytes_total), (1, 3 * MIB as u64 + 5));
    assert_eq!(files_under(&dest), vec!["IMG_0001/IMG_0001.CR3"]);
    assert!(
        std::fs::read(dest.join("IMG_0001/IMG_0001.CR3")).unwrap()
            == std::fs::read(photos.join("IMG_0001.CR3")).unwrap()
    );
    // Same link through the v0.3 protocol (toss/xfer/1, what pre-0.4 apps
    // speak; 0.6 clients use xfer/2 exactly like the get above).
    toss_core::testing::force_xfer1_client(client.endpoint_id(), true);
    let dest1 = tmp.path().join("dest-xfer1");
    let t = client.get(ticket, dest1.clone()).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    toss_core::testing::force_xfer1_client(client.endpoint_id(), false);
    assert_eq!(files_under(&dest1), vec!["IMG_0001/IMG_0001.CR3"]);
    host.shutdown().await;
    client.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vietnamese_api_errors() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "A").await;
    let err = a.create_ticket(TicketSpec {
        job: "x".into(),
        root: "RAW".into(),
        path: String::new(),
        ret: String::new(),
        get: true,
        put: false,
        days: 1,
    });
    assert_eq!(format!("{:#}", err.unwrap_err()), "Chế độ máy chủ đang tắt");
    let err = a
        .set_host(Some(HostConfig::new(
            vec![Root {
                alias: "bad alias!".into(),
                path: tmp.path().into(),
            }],
            tmp.path().join("ret"),
        )))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").starts_with("Tên thư mục gốc"), "{err:#}");
    let err = a
        .send_to_contact(&"ab".repeat(32), vec![])
        .await
        .unwrap_err();
    assert_eq!(format!("{err:#}"), "Không có máy này trong danh bạ");
    let err = a.pause(&"x".to_string()).await.unwrap_err();
    assert_eq!(format!("{err:#}"), "Không tìm thấy lượt chuyển này");
    let err = a.answer_push(&"00".repeat(16), true, false).unwrap_err();
    assert!(
        format!("{err:#}").starts_with("Không còn yêu cầu"),
        "{err:#}"
    );
    let err = "toss1abc".parse::<toss_core::Ticket>().unwrap_err();
    assert!(err.to_string().starts_with("Link bị hỏng"), "{err}");
    a.shutdown().await;
}

// ───────────────── compatibility with a Toss 0.6 peer ─────────────────

/// `PeerReq` as Toss 0.6 declares it (variants 0..=2).
#[derive(Serialize, Deserialize, Debug, PartialEq)]
enum PeerReq06 {
    Pair {
        secret: [u8; 16],
        name: String,
        platform: String,
        version: String,
    },
    Hello {
        name: String,
        version: String,
    },
    Push {
        push_id: [u8; 16],
        job: String,
        files: u32,
        bytes: u64,
    },
}

#[test]
fn old_variants_decode_as_0_6() {
    let new = toss_core::wire::PeerReq::Push {
        push_id: [7; 16],
        job: "Job".into(),
        files: 3,
        bytes: 9,
    };
    let bytes = postcard::to_allocvec(&new).unwrap();
    let old: PeerReq06 = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(
        old,
        PeerReq06::Push {
            push_id: [7; 16],
            job: "Job".into(),
            files: 3,
            bytes: 9
        }
    );
    let text = toss_core::wire::PeerReq::Text {
        name: "a".into(),
        text: "b".into(),
    };
    let bytes = postcard::to_allocvec(&text).unwrap();
    assert!(postcard::from_bytes::<PeerReq06>(&bytes).is_err());
}

/// A stand-in for a Toss 0.6 node on `toss/peer/1`: decodes the request as
/// 0.6 does and, like 0.6, closes with "bad request" when it cannot.
async fn old_peer() -> iroh::Endpoint {
    let ep = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![b"toss/peer/1".to_vec()])
        .bind()
        .await
        .unwrap();
    let e2 = ep.clone();
    tokio::spawn(async move {
        while let Some(inc) = e2.accept().await {
            tokio::spawn(async move {
                let Ok(conn) = inc.accept().unwrap().await else {
                    return;
                };
                let Ok((_send, mut recv)) = conn.accept_bi().await else {
                    return;
                };
                let res: anyhow::Result<PeerReq06> =
                    toss_core::wire::read_msg_max(&mut recv, toss_core::wire::MAX_HELLO).await;
                if res.is_err() {
                    conn.close(1u32.into(), b"bad request");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            });
        }
    });
    ep
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_peer_gets_a_clear_error() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let b = node(tmp.path(), "Editor B").await;
    let old = old_peer().await;
    let old_id = hex::encode(old.id().as_bytes());
    toss_core::testing::add_nearby(
        &b,
        NearbyPeer {
            id: old_id.clone(),
            name: String::new(),
            platform: String::new(),
            version: String::new(),
            addrs: loopback(&old).into_iter().map(|a| a.to_string()).collect(),
            last_seen: 0,
            contact: false,
        },
    );
    let err = b.send_text(&old_id, "chào").await.unwrap_err();
    assert!(format!("{err:#}").contains("bản cũ"), "{err:#}");
    let f = tmp.path().join("a.txt");
    std::fs::write(&f, b"x").unwrap();
    let t = b.send_to_peer(&old_id, vec![f]).await.unwrap();
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Error);
    assert!(
        st.error.as_deref().unwrap_or("").contains("bản cũ"),
        "{st:?}"
    );
    b.shutdown().await;
    old.close().await;
}

/// The real mDNS path: two nodes with `net.mdns` on advertise
/// `n=<name>;p=…;v=…` and find each other in `nearby()` without any hook,
/// then a stranger push needs nothing else. Ignored: needs working multicast
/// (sandboxes and CI containers usually drop it). Run with
/// `cargo test -p toss-core --test nearby -- --ignored`.
#[ignore = "needs multicast (mDNS) on the local network"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_mdns_nearby() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let start = |name: &str| {
        let mut cfg = NodeConfig {
            data_dir: tmp.path().join(format!("{name}-data")),
            device_name: name.to_string(),
            app_version: "0.8.0-test".into(),
            net: Default::default(),
        };
        cfg.net.mdns = true;
        Node::start(cfg)
    };
    let a = start("Mdns A").await.unwrap();
    let b = start("Mdns B").await.unwrap();
    a.set_inbox_dir(tmp.path().join("a-inbox"));
    a.set_receive_policy(ReceivePolicy::Everyone);
    let (aid, bid) = (hex_id(&a), hex_id(&b));
    wait_until("each other via mDNS", Duration::from_secs(30), || {
        a.nearby().iter().any(|p| p.id == bid && p.name == "Mdns B")
            && b.nearby().iter().any(|p| p.id == aid && p.name == "Mdns A")
    })
    .await;
    let p = b.nearby().into_iter().find(|p| p.id == aid).unwrap();
    assert_eq!(p.version, "0.8.0-test");
    assert!(!p.addrs.is_empty());
    // Renaming is re-advertised.
    a.set_device_name("Mdns A2");
    wait_until("new name via mDNS", Duration::from_secs(30), || {
        b.nearby()
            .iter()
            .any(|p| p.id == aid && p.name == "Mdns A2")
    })
    .await;
    let f = tmp.path().join("m.txt");
    std::fs::write(&f, b"mdns").unwrap();
    let t = b.send_to_peer(&aid, vec![f]).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    a.shutdown().await;
    b.shutdown().await;
}
