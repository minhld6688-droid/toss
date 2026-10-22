//! Adversarial and robustness tests: raw-protocol peers that misbehave, races
//! between transfers, cancellation. Own test binary because it shortens the
//! process-wide stall / pre-auth timeouts.

use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use iroh::{endpoint::Connection, EndpointAddr, SecretKey, TransportAddr};
use tokio::sync::broadcast;
use toss_core::{
    ticket::body_from_spec,
    transfer::{ErrorCode, Side, Status},
    wire::{
        read_msg, write_msg, ClientHello, CtlRequest, CtlResponse, DenyReason, Done, FileEntry,
        HostHello, Manifest, Op, Want, WantItem,
    },
    HostConfig, Node, NodeConfig, Root, Ticket, TicketSpec, TransferEvent, ALPN_CTL, ALPN_XFER,
};

const STALL: Duration = Duration::from_secs(2);
const PRE_AUTH: Duration = Duration::from_secs(3);

fn init() {
    toss_core::testing::set_timeouts(STALL, PRE_AUTH);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toss_core=info".into()),
        )
        .with_test_writer()
        .try_init();
}

async fn node(dir: &Path, name: &str) -> Node {
    Node::start(NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
    })
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

fn data(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// A sparse file of `len` zero bytes (cheap to create, slow enough to send).
fn sparse(path: &Path, len: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::File::create(path).unwrap().set_len(len).unwrap();
}

struct Setup {
    tmp: tempfile::TempDir,
    host: Node,
    client: Node,
    root: PathBuf,
    ret: PathBuf,
    dest: PathBuf,
}

async fn setup(max_jobs: u32) -> Setup {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let (root, ret, dest) = (t.join("root"), t.join("return"), t.join("dest"));
    for d in [&root, &dest] {
        std::fs::create_dir_all(d).unwrap();
    }
    let host = node(&t, "host").await;
    let client = node(&t, "client").await;
    let mut cfg = HostConfig::new(
        vec![Root {
            alias: "RAW".into(),
            path: root.clone(),
        }],
        ret.clone(),
    );
    cfg.max_parallel_jobs = max_jobs;
    host.set_host(Some(cfg)).await.unwrap();
    client.add_peer_addr(loopback_addr(&host));
    Setup {
        tmp,
        host,
        client,
        root,
        ret,
        dest,
    }
}

fn spec(job: &str, path: &str) -> TicketSpec {
    TicketSpec {
        job: job.into(),
        root: "RAW".into(),
        path: path.into(),
        ret: String::new(),
        get: true,
        put: true,
        days: 1,
    }
}

async fn raw(s: &Setup, alpn: &[u8]) -> Connection {
    s.client
        .endpoint()
        .connect(loopback_addr(&s.host), alpn)
        .await
        .expect("raw connect")
}

/// Raw xfer hello; returns the host's answer and the open streams.
async fn raw_hello(
    s: &Setup,
    ticket: &Ticket,
    op: Op,
) -> (
    Connection,
    iroh::endpoint::SendStream,
    iroh::endpoint::RecvStream,
    HostHello,
) {
    let conn = raw(s, ALPN_XFER).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let hello = ClientHello::v1 {
        ticket: ticket.clone(),
        device: "raw\nclient".into(),
        op,
        app_version: "raw".into(),
    };
    write_msg(&mut send, &hello).await.unwrap();
    let answer: HostHello = read_msg(&mut recv).await.unwrap();
    (conn, send, recv, answer)
}

fn denied(h: &HostHello) -> Option<DenyReason> {
    match h {
        HostHello::Denied { reason } => Some(reason.clone()),
        HostHello::Ok { .. } => None,
    }
}

async fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn final_events(evs: &[TransferEvent], id: &str) -> usize {
    evs.iter()
        .filter(|e| match e {
            TransferEvent::Done { id: i, .. } => i == id,
            TransferEvent::Error {
                id: i, retry_in_s, ..
            } => i == id && retry_in_s.is_none(),
            _ => false,
        })
        .count()
}

fn drain(rx: &mut broadcast::Receiver<TransferEvent>) -> Vec<TransferEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

// ---- unauthenticated peers ------------------------------------------------------------

/// A peer announcing a huge hello must be cut off at once (not buffered while
/// it trickles, not after the pre-auth timeout).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_hello_is_rejected_immediately() {
    let s = setup(2).await;
    let conn = raw(&s, ALPN_XFER).await;
    let (mut send, _recv) = conn.open_bi().await.unwrap();
    send.write_all(&(1u32 << 20).to_le_bytes()).await.unwrap();
    send.write_all(&[0u8; 100]).await.unwrap();
    let t0 = Instant::now();
    tokio::time::timeout(Duration::from_secs(10), conn.closed())
        .await
        .expect("host must close the connection");
    assert!(
        t0.elapsed() < PRE_AUTH / 2,
        "closed only after {:?} (pre-auth timeout, not the size check)",
        t0.elapsed()
    );
}

/// A control peer that never finishes its Auth message is dropped after the
/// pre-auth timeout (QUIC keep-alives would otherwise keep it forever).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn silent_ctl_peer_is_dropped() {
    let s = setup(2).await;
    let conn = raw(&s, ALPN_CTL).await;
    let (mut send, _recv) = conn.open_bi().await.unwrap();
    send.write_all(&[7, 0]).await.unwrap(); // half a length prefix, then nothing
    tokio::time::timeout(PRE_AUTH * 3, conn.closed())
        .await
        .expect("host must drop a peer that never authenticates");
}

/// Ticket checks on the raw protocol: admin tickets never grant get/put, job
/// tickets never grant ctl, flags are enforced, forged/revoked tickets fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_ticket_checks() {
    let s = setup(2).await;
    std::fs::write(s.root.join("f.txt"), b"x").unwrap();
    let admin = s.host.create_admin_ticket(0).unwrap();
    let (_c, _s, _r, h) = raw_hello(&s, &admin, Op::Get).await;
    assert_eq!(denied(&h), Some(DenyReason::NotAllowed));

    let mut ro = spec("ro", "");
    ro.put = false;
    let ro = s.host.create_ticket(ro).unwrap();
    let (_c, _s, _r, h) = raw_hello(&s, &ro, Op::Put).await;
    assert_eq!(denied(&h), Some(DenyReason::NotAllowed));

    // Forged: claims the host but signed by someone else.
    let other = SecretKey::generate();
    let mut body = body_from_spec(&other.public(), &spec("forged", ""), [5; 16]);
    body.host = *s.host.endpoint_id().as_bytes();
    let forged = Ticket::sign(body, &other);
    let (_c, _s, _r, h) = raw_hello(&s, &forged, Op::Get).await;
    assert_eq!(denied(&h), Some(DenyReason::BadSignature));

    // Tampered flags on a genuine ticket.
    let mut t = s.host.create_ticket(spec("t", "")).unwrap();
    t.body.path = "..".into();
    let (_c, _s, _r, h) = raw_hello(&s, &t, Op::Get).await;
    assert_eq!(denied(&h), Some(DenyReason::BadSignature));

    // Revoked.
    let rv = s.host.create_ticket(spec("rv", "")).unwrap();
    s.host.revoke(rv.body.id).unwrap();
    let (_c, _s, _r, h) = raw_hello(&s, &rv, Op::Get).await;
    assert_eq!(denied(&h), Some(DenyReason::Revoked));

    // Job ticket on the control channel.
    let job = s.host.create_ticket(spec("j", "")).unwrap();
    let conn = raw(&s, ALPN_CTL).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_msg(
        &mut send,
        &CtlRequest::Auth {
            ticket: job,
            device: "x".into(),
        },
    )
    .await
    .unwrap();
    let r: CtlResponse = read_msg(&mut recv).await.unwrap();
    assert!(
        matches!(
            r,
            CtlResponse::Denied {
                reason: DenyReason::NotAllowed
            }
        ),
        "{r:?}"
    );
}

/// Put manifests with hostile names: only the clean one is wanted, nothing is
/// written outside the job's return folder, and the job record keeps a
/// sanitized device name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_put_with_hostile_names() {
    let s = setup(2).await;
    let t = s.host.create_ticket(spec("evil", "")).unwrap();
    let (conn, mut send, mut recv, h) = raw_hello(&s, &t, Op::Put).await;
    assert!(denied(&h).is_none());
    let body = b"fine".to_vec();
    let rels = [
        "../escape.txt",
        "/abs.txt",
        "ok/../../x.txt",
        "C:\\x.txt",
        "a\\..\\..\\y.txt",
        "sub/ok.txt",
    ];
    let files: Vec<FileEntry> = rels
        .iter()
        .map(|r| FileEntry {
            rel: r.to_string(),
            size: body.len() as u64,
            hash: *blake3::hash(&body).as_bytes(),
        })
        .collect();
    let total = files.iter().map(|f| f.size).sum();
    write_msg(&mut send, &Manifest { files, total })
        .await
        .unwrap();
    let want: Want = read_msg(&mut recv).await.unwrap();
    assert_eq!(
        want.items,
        vec![WantItem {
            index: 5,
            offset: 0
        }]
    );
    let mut uni = conn.open_uni().await.unwrap();
    uni.write_all(&toss_core::wire::encode_stream_header(5, 0))
        .await
        .unwrap();
    uni.write_all(&body).await.unwrap();
    uni.finish().unwrap();
    let done: Done = read_msg(&mut recv).await.unwrap();
    assert!(!done.ok && done.msg.contains("skipped"), "{done:?}");
    conn.close(0u32.into(), b"bye");

    let landed = s.ret.join("evil");
    assert_eq!(std::fs::read(landed.join("sub/ok.txt")).unwrap(), body);
    let mut all = BTreeSet::new();
    fn rec(dir: &Path, out: &mut BTreeSet<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                rec(&e.path(), out);
            } else {
                out.insert(e.path());
            }
        }
    }
    rec(s.tmp.path(), &mut all);
    for p in &all {
        let n = p.file_name().unwrap().to_string_lossy();
        assert!(
            !["escape.txt", "abs.txt", "x.txt", "y.txt"].contains(&n.as_ref()),
            "hostile name landed at {}",
            p.display()
        );
    }
    wait_until("job event", Duration::from_secs(10), || {
        s.host.list_jobs().iter().any(|j| j.last.is_some())
    })
    .await;
    let last = s.host.list_jobs().remove(0).last.unwrap();
    assert_eq!(last.device, "rawclient");
}

// ---- stalled peers ----------------------------------------------------------------------

/// A client that asks for a file and then never reads must not hold a host
/// slot forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_receiver_frees_the_slot() {
    let s = setup(1).await;
    sparse(&s.root.join("big/big.bin"), 256 << 20);
    std::fs::write(s.root.join("small.txt"), b"small").unwrap();
    let t = s.host.create_ticket(spec("big", "big")).unwrap();
    let (_conn, mut send, mut recv, h) = raw_hello(&s, &t, Op::Get).await;
    assert!(denied(&h).is_none());
    let m: Manifest = read_msg(&mut recv).await.unwrap();
    assert_eq!(m.files.len(), 1);
    write_msg(
        &mut send,
        &Want {
            items: vec![WantItem {
                index: 0,
                offset: 0,
            }],
        },
    )
    .await
    .unwrap();
    // Never accept the uni stream.
    wait_until("slot taken", Duration::from_secs(10), || {
        s.host.host_status().unwrap().active == 1
    })
    .await;
    wait_until("slot freed", STALL * 6, || {
        s.host.host_status().unwrap().active == 0
    })
    .await;
    // The host is usable again.
    let t2 = s.host.create_ticket(spec("small", "small.txt")).unwrap();
    let tr = s.client.get(t2, s.dest.clone()).await.unwrap();
    let st = tokio::time::timeout(Duration::from_secs(30), tr.wait())
        .await
        .unwrap();
    assert_eq!(st.status, Status::Done, "{st:?}");
}

// ---- races and cancellation ----------------------------------------------------------------

/// Two editors upload different files with the same name at the same time:
/// both versions must survive intact (`a.bin` and `a (2).bin`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_puts_never_share_a_target() {
    let s = setup(2).await;
    let client2 = node(s.tmp.path(), "client2").await;
    client2.add_peer_addr(loopback_addr(&s.host));
    let t = s.host.create_ticket(spec("same", "")).unwrap();
    let (a, b) = (data(1, 24 << 20), data(2, 24 << 20));
    let (wa, wb) = (s.tmp.path().join("wa"), s.tmp.path().join("wb"));
    std::fs::create_dir_all(&wa).unwrap();
    std::fs::create_dir_all(&wb).unwrap();
    std::fs::write(wa.join("a.bin"), &a).unwrap();
    std::fs::write(wb.join("a.bin"), &b).unwrap();
    let (ta, tb) = tokio::join!(
        s.client.put(t.clone(), vec![wa.join("a.bin")]),
        client2.put(t.clone(), vec![wb.join("a.bin")]),
    );
    let (ta, tb) = (ta.unwrap(), tb.unwrap());
    let (sa, sb) = tokio::time::timeout(Duration::from_secs(120), async {
        tokio::join!(ta.wait(), tb.wait())
    })
    .await
    .expect("puts finish");
    assert_eq!(sa.status, Status::Done, "{sa:?}");
    assert_eq!(sb.status, Status::Done, "{sb:?}");
    let landed = s.ret.join("same");
    let names: BTreeSet<String> = std::fs::read_dir(&landed)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["a (2).bin", "a.bin"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    let contents: BTreeSet<[u8; 32]> = names
        .iter()
        .map(|n| *blake3::hash(&std::fs::read(landed.join(n)).unwrap()).as_bytes())
        .collect();
    let expected: BTreeSet<[u8; 32]> = [&a, &b]
        .iter()
        .map(|d| *blake3::hash(d).as_bytes())
        .collect();
    assert_eq!(contents, expected, "an upload was corrupted or overwritten");
    client2.shutdown().await;
}

/// Cancelling a host-side transfer leaves it Cancelled with a single final
/// event, even though the session then fails on its closed connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_side_cancel_stays_cancelled() {
    let s = setup(2).await;
    sparse(&s.root.join("c/big.bin"), 256 << 20);
    let t = s.host.create_ticket(spec("cancel", "c")).unwrap();
    let mut hrx = s.host.subscribe();
    let _tr = s.client.get(t, s.dest.clone()).await.unwrap();
    let hid = loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), hrx.recv())
            .await
            .unwrap()
            .unwrap();
        if let TransferEvent::Started { id, side, .. } = ev {
            assert_eq!(side, Side::Host);
            break id;
        }
    };
    s.host.cancel(&hid).await.unwrap();
    wait_until("host slot released", Duration::from_secs(20), || {
        s.host.host_status().unwrap().active == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let st = s.host.transfer(&hid).unwrap();
    assert_eq!(st.status, Status::Cancelled, "{st:?}");
    let evs = drain(&mut hrx);
    let errors: Vec<_> = evs
        .iter()
        .filter(|e| matches!(e, TransferEvent::Error { id, .. } if *id == hid))
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(matches!(
        errors[0],
        TransferEvent::Error {
            code: ErrorCode::Cancelled,
            ..
        }
    ));
}

/// Starting the same get twice while the first is still running returns the
/// running transfer instead of a second one writing the same `.part` files.
/// Cancelling twice emits one final event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_get_and_double_cancel() {
    let s = setup(0).await; // host always busy: the get stays Connecting
    std::fs::write(s.root.join("f.txt"), b"x").unwrap();
    let t = s.host.create_ticket(spec("dup", "")).unwrap();
    let mut rx = s.client.subscribe();
    let a = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    let b = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(s.client.transfers().len(), 1);
    // Another destination is a different transfer.
    let other = s.tmp.path().join("other");
    let c = s.client.get(t.clone(), other).await.unwrap();
    assert_ne!(c.id, a.id);

    s.client.cancel(&a.id).await.unwrap();
    s.client.cancel(&a.id).await.unwrap();
    assert_eq!(b.wait().await.status, Status::Cancelled);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let evs = drain(&mut rx);
    assert_eq!(final_events(&evs, &a.id), 1, "{evs:?}");
    // A finished get can be started again (new transfer).
    let d = s.client.get(t, s.dest.clone()).await.unwrap();
    assert_ne!(d.id, a.id);
    s.client.cancel(&c.id).await.unwrap();
    s.client.cancel(&d.id).await.unwrap();
}

/// Two resumes at once (double click) must not start two workers; the loser
/// would otherwise report `Paused` while the other keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resume_starts_one_worker() {
    let s = setup(0).await;
    std::fs::write(s.root.join("f.txt"), b"x").unwrap();
    let t = s.host.create_ticket(spec("resume", "")).unwrap();
    let mut rx = s.client.subscribe();
    let tr = s.client.get(t, s.dest.clone()).await.unwrap();
    for _ in 0..20 {
        s.client.pause(&tr.id).await.unwrap();
        let (c1, c2) = (s.client.clone(), s.client.clone());
        let (i1, i2) = (tr.id.clone(), tr.id.clone());
        let h1 = tokio::spawn(async move { c1.resume(&i1).await });
        let h2 = tokio::spawn(async move { c2.resume(&i2).await });
        h1.await.unwrap().unwrap();
        h2.await.unwrap().unwrap();
        drain(&mut rx);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let late_pause = drain(&mut rx)
            .into_iter()
            .any(|e| matches!(e, TransferEvent::Paused { ref id } if *id == tr.id));
        assert!(
            !late_pause,
            "a second worker was started and the first one reported Paused"
        );
        assert_ne!(s.client.transfer(&tr.id).unwrap().status, Status::Paused);
    }
    s.client.cancel(&tr.id).await.unwrap();
}
