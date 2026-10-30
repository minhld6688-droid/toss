//! v0.5 "Danh bạ máy" (contracts/contacts-v0.5.md §6): invites, pairing,
//! presence, pushes to contacts, transfer persistence across a restart, and
//! the Local Network probe. Nodes talk over loopback (`add_peer_addr`); no
//! internet needed. The restart test writes ~300 MB under `TMPDIR`: on a
//! nearly full disk it (and `write_data`) fail with "No space left on device".

use std::{
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    time::{Duration, Instant},
};

use iroh::{EndpointAddr, TransportAddr};
use toss_core::{
    transfer::{Side, Status},
    wire::{PeerReq, PeerResp},
    HostConfig, Node, NodeConfig, Root, TicketSpec, TransferEvent, Tuning,
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

/// Small split/batch thresholds so that test-sized files exercise them.
fn small_tuning() -> Tuning {
    Tuning {
        range_split_mb: 1,
        small_file_kb: 64,
        streams: 4,
        streams_high_rtt: 8,
        ..Tuning::default()
    }
}

async fn node(dir: &Path, name: &str) -> Node {
    let mut cfg = NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
        net: Default::default(),
    };
    cfg.net.tuning = small_tuning();
    let n = Node::start(cfg).await.expect("start node");
    n.set_inbox_dir(dir.join(format!("{name}-inbox")));
    n
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

/// Both nodes can dial each other over loopback.
fn link(a: &Node, b: &Node) {
    a.add_peer_addr(loopback_addr(b));
    b.add_peer_addr(loopback_addr(a));
}

fn hex_id(n: &Node) -> String {
    hex::encode(n.endpoint_id().as_bytes())
}

fn write_data(path: &Path, seed: u64, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut block = vec![0u8; MIB.min(len.max(1))];
    let mut left = len;
    let mut n = 0u64;
    while left > 0 {
        for b in block.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        let k = left.min(block.len());
        let tag = (seed ^ (n << 20)).to_le_bytes();
        let t = tag.len().min(k);
        block[..t].copy_from_slice(&tag[..t]);
        f.write_all(&block[..k]).unwrap();
        left -= k;
        n += 1;
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

/// Waits until client transfer `id` of `n` is `Done`; fails at once (with its
/// state) when it ends otherwise, e.g. `No space left on device` on a full
/// disk, instead of running into the timeout.
async fn wait_done(n: &Node, id: &str, what: &str, limit: Duration) {
    let t0 = Instant::now();
    loop {
        let st = n.transfer(&id.to_string()).expect("transfer listed");
        match st.status {
            Status::Done => return,
            Status::Error | Status::Cancelled => panic!("{what} ended with {st:?}"),
            _ => {}
        }
        assert!(t0.elapsed() < limit, "timed out waiting for {what}: {st:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_final(t: &toss_core::Transfer) -> toss_core::TransferState {
    tokio::time::timeout(Duration::from_secs(120), t.wait())
        .await
        .expect("transfer timed out")
}

/// Pairs `b` with an invite of `a`; returns (a's id, b's id).
async fn pair(a: &Node, b: &Node) -> (String, String) {
    link(a, b);
    let inv = a.create_invite(7, "").unwrap();
    let c = b.accept_invite(&inv.invite).await.expect("pair");
    assert_eq!(c.id, hex_id(a));
    (hex_id(a), hex_id(b))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invite_roundtrip_and_signature() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Đạt – Mac").await;
    let info = a.create_invite(7, "toss-link.vercel.app").unwrap();
    assert!(info.invite.starts_with("tossinv1"), "{}", info.invite);
    assert_eq!(info.deep_link, format!("toss://i/{}", info.invite));
    assert_eq!(
        info.web_url,
        format!("https://toss-link.vercel.app/#{}", info.invite)
    );
    assert_eq!(info.name, "Đạt – Mac");
    let week = 7 * 86_400;
    let now = toss_core::util::now_unix();
    assert!(info.exp > now + week - 60 && info.exp <= now + week + 60);
    for text in [
        info.invite.clone(),
        info.web_url.clone(),
        info.deep_link.clone(),
        format!("  {}\n", info.invite.to_uppercase()),
    ] {
        let p = Node::parse_invite(&text).unwrap();
        assert_eq!(p.invite, info.invite);
        assert_eq!(p.name, info.name);
        assert_eq!(p.exp, info.exp);
    }
    // Any flipped character breaks the decoding or the signature.
    let mut chars: Vec<char> = info.invite.chars().collect();
    for i in [12, chars.len() / 2, chars.len() - 30] {
        let orig = chars[i];
        chars[i] = if orig == 'a' { 'b' } else { 'a' };
        let s: String = chars.iter().collect();
        assert!(
            Node::parse_invite(&s).is_err(),
            "tampered invite accepted (pos {i})"
        );
        chars[i] = orig;
    }
    assert!(Node::parse_invite("toss1abcdef").is_err());
    assert!(tmp.path().join("Đạt – Mac-data/invites.json").exists());
    a.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_multi_use_and_revoke() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let c = node(tmp.path(), "Editor C").await;
    let d = node(tmp.path(), "Editor D").await;
    for x in [&b, &c, &d] {
        link(&a, x);
    }
    let mut a_events = a.subscribe();
    let inv = a.create_invite(7, "").unwrap();

    let cb = b.accept_invite(&inv.invite).await.expect("b pairs");
    assert_eq!(cb.id, hex_id(&a));
    assert_eq!(cb.name, "Studio");
    assert_eq!(cb.platform, std::env::consts::OS);
    assert!(cb.auto_accept);
    assert!(cb.online);
    assert_eq!(b.contacts(), vec![cb.clone()]);
    let got = a.contacts();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, hex_id(&b));
    assert_eq!(got[0].name, "Editor B");
    assert_eq!(got[0].their_name, "Editor B");
    let ev = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(TransferEvent::ContactsChanged {}) = a_events.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(ev.is_ok(), "inviter emitted no ContactsChanged");
    let json = serde_json::to_value(TransferEvent::ContactsChanged {}).unwrap();
    assert_eq!(json, serde_json::json!({"type": "contacts_changed"}));

    // The same invite pairs a second device (web link form).
    c.accept_invite(&inv.web_url).await.expect("c pairs");
    assert_eq!(a.contacts().len(), 2);
    // Persisted on both sides.
    let saved = std::fs::read_to_string(tmp.path().join("Studio-data/contacts.json")).unwrap();
    assert!(saved.contains(&hex_id(&c)));

    // Revoked: no more pairings, existing contacts stay.
    a.revoke_invites().unwrap();
    let err = d.accept_invite(&inv.invite).await.unwrap_err();
    assert!(format!("{err:#}").contains("huỷ"), "{err:#}");
    assert_eq!(a.contacts().len(), 2);
    assert!(d.contacts().is_empty());

    // Rename / auto-accept / remove.
    a.rename_contact(&hex_id(&b), "Máy Đạt").unwrap();
    a.set_contact_auto_accept(&hex_id(&b), false).unwrap();
    let cb_on_a = a
        .contacts()
        .into_iter()
        .find(|x| x.id == hex_id(&b))
        .unwrap();
    assert_eq!(cb_on_a.name, "Máy Đạt");
    assert!(!cb_on_a.auto_accept);
    a.remove_contact(&hex_id(&c)).unwrap();
    assert_eq!(a.contacts().len(), 1);
    for n in [a, b, c, d] {
        n.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_contact_requests_denied() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "A").await;
    let s = node(tmp.path(), "Stranger").await;
    link(&a, &s);
    let addr = loopback_addr(&a);
    for req in [
        PeerReq::Hello {
            name: "x".into(),
            version: "0".into(),
        },
        PeerReq::Push {
            push_id: [1; 16],
            job: "job".into(),
            files: 1,
            bytes: 1,
        },
        PeerReq::Pair {
            secret: [9; 16],
            name: "x".into(),
            platform: "linux".into(),
            version: "0".into(),
        },
    ] {
        let resp = toss_core::testing::peer_request(&s, addr.clone(), &req)
            .await
            .expect("answer");
        assert!(
            matches!(resp, PeerResp::Denied { .. }),
            "{req:?} -> {resp:?}"
        );
    }
    assert!(a.contacts().is_empty());
    assert!(a.incoming_pushes().is_empty());

    // A removed contact is a stranger again.
    let (aid, sid) = pair(&a, &s).await;
    a.remove_contact(&sid).unwrap();
    s.refresh_presence().await;
    let on_s = s.contacts().into_iter().find(|c| c.id == aid).unwrap();
    assert!(!on_s.online, "removed contact still answers presence");
    a.shutdown().await;
    s.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_auto_accept_lands_in_inbox() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let (aid, bid) = pair(&a, &b).await;

    // A folder with a file split into ≥ 3 segments, small files and an empty one.
    let job = tmp.path().join("work/12A3 RAW");
    write_data(&job.join("clip.mov"), 1, 3 * MIB + 777);
    for i in 0..12 {
        write_data(
            &job.join(format!("sub/IMG_{i:04}.CR3")),
            10 + i,
            20_000 + i as usize,
        );
    }
    std::fs::write(job.join("empty.txt"), b"").unwrap();

    let mut a_events = a.subscribe();
    let t = b.send_to_contact(&aid, vec![job.clone()]).await.unwrap();
    let st = b.transfer(&t.id).unwrap();
    assert_eq!(st.contact.as_deref(), Some(aid.as_str()));
    assert_eq!(st.side, Side::Client);
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    let landed = a.inbox_dir().join("Editor B/12A3 RAW");
    assert_same_tree(&job, &landed);

    // The receiving side shows the push with the contact id.
    let host_side = a
        .transfers()
        .into_iter()
        .find(|s| s.side == Side::Host)
        .expect("receiver transfer");
    assert_eq!(host_side.contact.as_deref(), Some(bid.as_str()));
    assert_eq!(host_side.status, Status::Done);
    let mut started_contact = None;
    while let Ok(ev) = a_events.try_recv() {
        if let TransferEvent::Started { contact, .. } = ev {
            started_contact = contact;
        }
    }
    assert_eq!(started_contact.as_deref(), Some(bid.as_str()));

    // Loose files → "N file"; a changed file never overwrites (numbered name).
    let f1 = tmp.path().join("work/a.jpg");
    let f2 = tmp.path().join("work/b.jpg");
    write_data(&f1, 50, 10_000);
    write_data(&f2, 51, 10_000);
    let t = b
        .send_to_contact(&aid, vec![f1.clone(), f2.clone()])
        .await
        .unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    write_data(&f1, 52, 12_000);
    let t = b.send_to_contact(&aid, vec![f1.clone(), f2]).await.unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    let loose = a.inbox_dir().join("Editor B/2 file");
    assert_eq!(files_under(&loose), vec!["a (2).jpg", "a.jpg", "b.jpg"]);
    assert_eq!(
        std::fs::read(loose.join("a (2).jpg")).unwrap(),
        std::fs::read(&f1).unwrap()
    );
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_waits_for_user_answer() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let (aid, bid) = pair(&a, &b).await;
    a.set_contact_auto_accept(&bid, false).unwrap();
    let src = tmp.path().join("work/Job");
    write_data(&src.join("x.bin"), 3, 2 * MIB);

    // Declined.
    let mut a_events = a.subscribe();
    let t = b.send_to_contact(&aid, vec![src.clone()]).await.unwrap();
    let incoming = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(TransferEvent::PushIncoming(p)) = a_events.recv().await {
                return p;
            }
        }
    })
    .await
    .expect("no PushIncoming");
    assert_eq!(incoming.from_id, bid);
    assert_eq!(incoming.from_name, "Editor B");
    assert_eq!(incoming.job, "Job");
    assert_eq!(incoming.files, 1);
    assert_eq!(incoming.bytes, 2 * MIB as u64);
    assert_eq!(a.incoming_pushes(), vec![incoming.clone()]);
    let json = serde_json::to_value(TransferEvent::PushIncoming(incoming.clone())).unwrap();
    assert_eq!(json["type"], "push_incoming");
    assert_eq!(json["push_id"], incoming.push_id.as_str());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(b.transfer(&t.id).unwrap().status, Status::Connecting);
    a.answer_push(&incoming.push_id, false, false).unwrap();
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Error);
    assert_eq!(st.error.as_deref(), Some("người nhận đã từ chối"));
    assert!(a.incoming_pushes().is_empty());
    assert!(a.answer_push(&incoming.push_id, true, false).is_err());
    assert!(!a.inbox_dir().join("Editor B/Job/x.bin").exists());

    // Accepted.
    let t = b.send_to_contact(&aid, vec![src.clone()]).await.unwrap();
    wait_until("queued push", Duration::from_secs(30), || {
        !a.incoming_pushes().is_empty()
    })
    .await;
    let p = a.incoming_pushes().remove(0);
    a.answer_push(&p.push_id, true, false).unwrap();
    assert_eq!(wait_final(&t).await.status, Status::Done);
    assert_same_tree(&src, &a.inbox_dir().join("Editor B/Job"));
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_to_offline_contact_starts_when_it_comes_online() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "Studio").await;
    let b = node(tmp.path(), "Editor B").await;
    let (aid, _bid) = pair(&a, &b).await;
    let a_inbox = a.inbox_dir();
    a.shutdown().await;
    b.refresh_presence().await;
    let on_b = b.contacts().into_iter().find(|c| c.id == aid).unwrap();
    assert!(!on_b.online, "A is down");

    let src = tmp.path().join("work/Offline job");
    write_data(&src.join("clip.mov"), 7, 2 * MIB + 5);
    let t = b.send_to_contact(&aid, vec![src.clone()]).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let st = b.transfer(&t.id).unwrap();
    assert_eq!(st.status, Status::Connecting, "{st:?}");
    assert_eq!(st.error.as_deref(), Some("Chờ Studio online"));
    // Persisted as a waiting push.
    let saved = std::fs::read_to_string(tmp.path().join("Editor B-data/transfers.json")).unwrap();
    assert!(
        saved.contains("\"push\"") && saved.contains(&aid),
        "{saved}"
    );

    // A comes back (same identity); its presence ping tells B.
    let a2 = node(tmp.path(), "Studio").await;
    a2.set_inbox_dir(a_inbox.clone());
    link(&a2, &b);
    a2.refresh_presence().await;
    let st = wait_final(&t).await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_same_tree(&src, &a_inbox.join("Editor B/Offline job"));
    a2.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_mid_get_resumes_from_transfers_json() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let root = t.join("root");
    let dest = t.join("dest");
    let src = root.join("big/clip.mov");
    write_data(&src, 9, 96 * MIB + 333);
    let host = node(&t, "host").await;
    host.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "RAW".into(),
            path: root.clone(),
        }],
        t.join("return"),
    )))
    .await
    .unwrap();
    let ticket = host
        .create_ticket(TicketSpec {
            job: "Big".into(),
            root: "RAW".into(),
            path: "big".into(),
            ret: String::new(),
            get: true,
            put: false,
            days: 1,
        })
        .unwrap();

    let client = node(&t, "client").await;
    client.add_peer_addr(loopback_addr(&host));
    // A paused one stays paused across the restart.
    let dest2 = t.join("dest2");
    let paused = client.get(ticket.clone(), dest2.clone()).await.unwrap();
    client.pause(&paused.id).await.unwrap();
    let tr = client.get(ticket, dest.clone()).await.unwrap();
    let part = dest.join("Big/clip.mov.part");
    let fin = dest.join("Big/clip.mov");
    let t0 = Instant::now();
    while !std::fs::metadata(&part).is_ok_and(|m| m.len() >= 4 * MIB as u64) {
        assert!(!fin.exists(), "finished before the restart");
        assert!(t0.elapsed() < Duration::from_secs(60), "no progress");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    client.shutdown().await;
    let saved = std::fs::read_to_string(t.join("client-data/transfers.json")).unwrap();
    let recs: Vec<toss_core::resume::TransferRecord> = serde_json::from_str(&saved).unwrap();
    assert_eq!(recs.len(), 2);
    let rec = recs.iter().find(|r| r.id == tr.id).unwrap();
    assert_eq!(rec.kind, toss_core::resume::Kind::Get);
    assert!(
        matches!(rec.status, Status::Running | Status::Connecting),
        "{:?}",
        rec.status
    );
    let rec2 = recs.iter().find(|r| r.id == paused.id).unwrap();
    assert_eq!(rec2.status, Status::Paused);
    let kept = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    eprintln!(".part kept {kept} bytes across the restart");

    let client = node(&t, "client").await;
    client.add_peer_addr(loopback_addr(&host));
    wait_until("restored transfer", Duration::from_secs(10), || {
        client.transfer(&tr.id).is_some()
    })
    .await;
    wait_done(
        &client,
        &tr.id,
        "resumed transfer",
        Duration::from_secs(150),
    )
    .await;
    assert!(std::fs::read(&fin).unwrap() == std::fs::read(&src).unwrap());
    assert!(!part.exists());
    // transfers.json is written right after the final event, not before it: wait for it.
    let tj = t.join("client-data/transfers.json");
    wait_until(
        "transfers.json records done",
        Duration::from_secs(10),
        || std::fs::read_to_string(&tj).is_ok_and(|s| s.contains("\"done\"")),
    )
    .await;
    assert_eq!(client.transfer(&paused.id).unwrap().status, Status::Paused);
    client.resume(&paused.id).await.unwrap();
    wait_done(
        &client,
        &paused.id,
        "resumed paused transfer",
        Duration::from_secs(150),
    )
    .await;
    assert!(std::fs::read(dest2.join("Big/clip.mov")).unwrap() == std::fs::read(&src).unwrap());
    client.shutdown().await;
    host.shutdown().await;
}

#[cfg(not(target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_blocked_is_false_off_macos() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let a = node(tmp.path(), "A").await;
    let d = a.diagnostics().await;
    assert!(!d.lan_blocked);
    assert!(!d.reasons.iter().any(|r| r.contains("Mạng cục bộ")));
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["lan_blocked"], false);
    let ev = serde_json::to_value(TransferEvent::LanBlocked { blocked: true }).unwrap();
    assert_eq!(
        ev,
        serde_json::json!({"type": "lan_blocked", "blocked": true})
    );
    a.shutdown().await;
}
