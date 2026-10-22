//! End-to-end tests: two nodes in one process talking over loopback.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use iroh::{EndpointAddr, TransportAddr};
use tokio::sync::broadcast;
use toss_core::{
    transfer::{ErrorCode, Status},
    HostConfig, Node, NodeConfig, Root, Ticket, TicketSpec, TransferEvent,
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

async fn node(dir: &Path, name: &str) -> Node {
    Node::start(NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
    })
    .await
    .expect("start node")
}

/// Loopback address of `node` (avoids depending on discovery in the sandbox).
fn loopback_addr(node: &Node) -> EndpointAddr {
    let addrs = node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .filter(|s| s.is_ipv4())
        .map(|s| TransportAddr::Ip(SocketAddr::from((Ipv4Addr::LOCALHOST, s.port()))));
    EndpointAddr::from_parts(node.endpoint_id(), addrs)
}

/// Deterministic pseudo-random bytes.
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

const BIG: usize = 20 * 1024 * 1024;

/// Creates ~20 files incl. nested dirs and one 20 MB file. Returns the rel paths
/// that must be transferred.
fn make_tree(base: &Path) -> Vec<String> {
    let mut rels = Vec::new();
    let mut add = |rel: &str, bytes: Vec<u8>| {
        let p = base.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        rels.push(rel.to_string());
    };
    add("big.bin", data(1, BIG));
    add("empty.txt", Vec::new());
    for i in 0..8 {
        add(
            &format!("IMG_{i:04}.CR3"),
            data(10 + i, 50_000 + i as usize * 1000),
        );
    }
    for i in 0..5 {
        add(
            &format!("sub/a/photo {i}.jpg"),
            data(100 + i, 3_000 + i as usize),
        );
    }
    for i in 0..4 {
        add(&format!("sub/b/c/deep_{i}.tif"), data(200 + i, 70_000));
    }
    add("Ảnh lớp/12A3.jpg", data(300, 12_345));
    // Not transferred:
    std::fs::write(base.join(".DS_Store"), b"junk").unwrap();
    std::fs::write(base.join("sub/half.bin.part"), b"partial").unwrap();
    std::fs::create_dir_all(base.join(".git")).unwrap();
    std::fs::write(base.join(".git/config"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", base.join("passwd-link")).unwrap();
    rels.sort();
    rels
}

fn assert_same(src: &Path, dst: &Path, rels: &[String]) {
    for rel in rels {
        let a = std::fs::read(src.join(rel)).unwrap();
        let b = std::fs::read(dst.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(a == b, "{rel} differs");
        assert_eq!(blake3::hash(&a), blake3::hash(&b));
    }
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn rec(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                rec(base, &p, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    rec(dir, dir, &mut out);
    out.sort();
    out
}

/// Collects events of transfer `id` until a final one.
async fn events_for(rx: &mut broadcast::Receiver<TransferEvent>, id: &str) -> Vec<TransferEvent> {
    let mut out = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("event timeout")
            .expect("event channel");
        let (eid, last) = match &ev {
            TransferEvent::Started { id, .. }
            | TransferEvent::Manifest { id, .. }
            | TransferEvent::Progress { id, .. }
            | TransferEvent::Paused { id } => (id.clone(), false),
            TransferEvent::Done { id, .. } => (id.clone(), true),
            TransferEvent::Error { id, retry_in_s, .. } => (id.clone(), retry_in_s.is_none()),
            TransferEvent::HostStatus(_) => continue,
        };
        if eid == id {
            out.push(ev);
            if last {
                return out;
            }
        }
    }
}

fn manifest_of(evs: &[TransferEvent]) -> (u32, u64, u32, u64) {
    evs.iter()
        .rev()
        .find_map(|e| match e {
            TransferEvent::Manifest {
                files,
                bytes,
                to_do_files,
                to_do_bytes,
                ..
            } => Some((*files, *bytes, *to_do_files, *to_do_bytes)),
            _ => None,
        })
        .expect("manifest event")
}

struct Setup {
    _tmp: tempfile::TempDir,
    host: Node,
    client: Node,
    root: PathBuf,
    ret: PathBuf,
    dest: PathBuf,
    work: PathBuf,
}

async fn setup(max_jobs: u32) -> Setup {
    init_tracing();
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let root = t.join("root");
    let ret = t.join("return");
    let dest = t.join("dest");
    let work = t.join("work");
    for d in [&root, &dest, &work] {
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
        _tmp: tmp,
        host,
        client,
        root,
        ret,
        dest,
        work,
    }
}

fn spec(job: &str, path: &str, ret: &str) -> TicketSpec {
    TicketSpec {
        job: job.into(),
        root: "RAW".into(),
        path: path.into(),
        ret: ret.into(),
        get: true,
        put: true,
        days: 1,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_put_resume() {
    let s = setup(2).await;
    let src = s.root.join("2026/12A3");
    let rels = make_tree(&src);
    assert_eq!(rels.len(), 20);

    let ticket = s
        .host
        .create_ticket(spec("12A3 RAW", "2026/12A3", "12A3"))
        .unwrap();
    // Round-trip through the string form, as a real client would.
    let ticket: Ticket = ticket.to_string().parse().unwrap();

    // ---- get ----
    let mut rx = s.client.subscribe();
    let mut host_rx = s.host.subscribe();
    let t = s.client.get(ticket.clone(), s.dest.clone()).await.unwrap();
    let job_dir = s.dest.join("12A3 RAW");
    assert!(job_dir.is_dir(), "dest/<job> must exist immediately");
    let evs = events_for(&mut rx, &t.id).await;
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert!(matches!(evs.first(), Some(TransferEvent::Started { .. })));
    assert!(matches!(evs.last(), Some(TransferEvent::Done { .. })));
    assert!(
        matches!(
            evs[evs.len() - 2],
            TransferEvent::Progress {
                path: toss_core::ConnPath::Direct,
                ..
            }
        ),
        "Direct Progress right before Done: {:?}",
        evs[evs.len() - 2]
    );
    // Event JSON shape matches contracts/ipc.ts.
    let json = serde_json::to_value(&evs[0]).unwrap();
    assert_eq!(json["type"], "started");
    assert_eq!(json["op"], "get");
    assert_eq!(json["side"], "client");
    let json = serde_json::to_value(&evs[evs.len() - 2]).unwrap();
    assert_eq!(json["type"], "progress");
    assert_eq!(json["path"], "direct");
    let total: u64 = rels
        .iter()
        .map(|r| std::fs::metadata(src.join(r)).unwrap().len())
        .sum();
    assert_eq!(manifest_of(&evs), (20, total, 20, total));
    assert_eq!(files_under(&job_dir), rels);
    assert_same(&src, &job_dir, &rels);
    assert_eq!(st.bytes_done, total);
    assert_eq!(st.files_done, 20);
    assert_eq!(
        st.dest.as_deref(),
        Some(job_dir.display().to_string().as_str())
    );
    // Host side saw it too.
    let hevs = loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), host_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let TransferEvent::Started { id, side, peer, .. } = &ev {
            assert_eq!(*side, toss_core::transfer::Side::Host);
            assert_eq!(peer, "client");
            break events_for(&mut host_rx, id).await;
        }
    };
    assert!(
        matches!(hevs.last(), Some(TransferEvent::Done { .. })),
        "{hevs:?}"
    );
    let jobs = s.host.list_jobs();
    let last = jobs[0].last.as_ref().expect("job event recorded");
    assert!(last.ok && last.files == 20 && last.bytes == total);

    // ---- get again: nothing to do ----
    let t = s.client.get(ticket.clone(), s.dest.clone()).await.unwrap();
    let evs = events_for(&mut rx, &t.id).await;
    assert_eq!(t.wait().await.status, Status::Done);
    assert_eq!(manifest_of(&evs), (20, total, 0, 0));

    // ---- resume: big.bin half done as .part, one small file missing ----
    let big = job_dir.join("big.bin");
    let half = (BIG / 2) as u64;
    let bytes = std::fs::read(&big).unwrap();
    std::fs::write(job_dir.join("big.bin.part"), &bytes[..half as usize]).unwrap();
    std::fs::remove_file(&big).unwrap();
    std::fs::remove_file(job_dir.join("sub/a/photo 2.jpg")).unwrap();
    let small = std::fs::metadata(src.join("sub/a/photo 2.jpg"))
        .unwrap()
        .len();
    let t = s.client.get(ticket.clone(), s.dest.clone()).await.unwrap();
    let evs = events_for(&mut rx, &t.id).await;
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_eq!(manifest_of(&evs), (20, total, 2, BIG as u64 - half + small));
    assert_same(&src, &job_dir, &rels);
    assert!(!job_dir.join("big.bin.part").exists());

    // ---- corrupted .part: wrong prefix gets detected and re-fetched once ----
    let mut bad = bytes[..half as usize].to_vec();
    bad[10] ^= 0xff;
    std::fs::write(job_dir.join("big.bin.part"), &bad).unwrap();
    std::fs::remove_file(&big).unwrap();
    let t = s.client.get(ticket.clone(), s.dest.clone()).await.unwrap();
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_same(&src, &job_dir, &rels);

    // ---- put a folder back ----
    let edits = s.work.join("edits");
    std::fs::create_dir_all(edits.join("raw")).unwrap();
    std::fs::write(edits.join("a.jpg"), data(500, 40_000)).unwrap();
    std::fs::write(edits.join("raw/b.tif"), data(501, 90_000)).unwrap();
    std::fs::write(edits.join(".DS_Store"), b"x").unwrap();
    std::fs::write(s.work.join("single.txt"), b"hello").unwrap();
    let paths = vec![edits.clone(), s.work.join("single.txt")];
    let t = s.client.put(ticket.clone(), paths.clone()).await.unwrap();
    let evs = events_for(&mut rx, &t.id).await;
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_eq!(manifest_of(&evs).2, 3);
    let landed = s.ret.join("12A3");
    assert_eq!(
        files_under(&landed),
        vec!["edits/a.jpg", "edits/raw/b.tif", "single.txt"]
    );
    assert_eq!(
        std::fs::read(landed.join("edits/a.jpg")).unwrap(),
        data(500, 40_000)
    );

    // Same content again: idempotent, nothing renamed.
    let t = s.client.put(ticket.clone(), paths.clone()).await.unwrap();
    let evs = events_for(&mut rx, &t.id).await;
    assert_eq!(t.wait().await.status, Status::Done);
    assert_eq!(manifest_of(&evs).2, 0);
    assert_eq!(files_under(&landed).len(), 3);

    // Changed content: never overwrite, write "<stem> (2).<ext>".
    std::fs::write(edits.join("a.jpg"), data(502, 41_000)).unwrap();
    let t = s.client.put(ticket.clone(), paths.clone()).await.unwrap();
    assert_eq!(t.wait().await.status, Status::Done);
    std::fs::write(edits.join("a.jpg"), data(503, 42_000)).unwrap();
    let t = s.client.put(ticket.clone(), paths.clone()).await.unwrap();
    assert_eq!(t.wait().await.status, Status::Done);
    assert_eq!(
        files_under(&landed),
        vec![
            "edits/a (2).jpg",
            "edits/a (3).jpg",
            "edits/a.jpg",
            "edits/raw/b.tif",
            "single.txt"
        ]
    );
    assert_eq!(
        std::fs::read(landed.join("edits/a.jpg")).unwrap(),
        data(500, 40_000)
    );
    assert_eq!(
        std::fs::read(landed.join("edits/a (2).jpg")).unwrap(),
        data(502, 41_000)
    );
    assert_eq!(
        std::fs::read(landed.join("edits/a (3).jpg")).unwrap(),
        data(503, 42_000)
    );

    // ---- get-only ticket refuses put; revoked ticket is refused for good ----
    let mut ro = spec("ro", "2026/12A3/sub", "");
    ro.put = false;
    let ro = s.host.create_ticket(ro).unwrap();
    assert!(s.client.put(ro.clone(), paths.clone()).await.is_err());
    s.host.revoke(ro.body.id).unwrap();
    let t = s.client.get(ro.clone(), s.dest.clone()).await.unwrap();
    let evs = events_for(&mut rx, &t.id).await;
    let st = t.wait().await;
    assert_eq!(st.status, Status::Error);
    assert!(
        matches!(
            evs.last(),
            Some(TransferEvent::Error {
                code: ErrorCode::Revoked,
                retry_in_s: None,
                ..
            })
        ),
        "{evs:?}"
    );

    // ---- pause right away, then resume ----
    std::fs::remove_file(&big).unwrap();
    let t = s.client.get(ticket.clone(), s.dest.clone()).await.unwrap();
    s.client.pause(&t.id).await.unwrap();
    assert_eq!(s.client.transfer(&t.id).unwrap().status, Status::Paused);
    s.client.resume(&t.id).await.unwrap();
    let st = t.wait().await;
    assert_eq!(st.status, Status::Done, "{st:?}");
    assert_same(&src, &job_dir, &rels);

    s.client.shutdown().await;
    s.host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_and_cancel() {
    let s = setup(0).await;
    std::fs::write(s.root.join("f.txt"), b"x").unwrap();
    let ticket = s.host.create_ticket(spec("busy", "", "")).unwrap();
    let mut rx = s.client.subscribe();
    let t = s.client.get(ticket, s.dest.clone()).await.unwrap();
    let ev = loop {
        let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(ev, TransferEvent::Error { .. }) {
            break ev;
        }
    };
    assert!(
        matches!(
            ev,
            TransferEvent::Error {
                code: ErrorCode::Busy,
                retry_in_s: Some(30),
                ..
            }
        ),
        "{ev:?}"
    );
    assert_eq!(s.client.transfer(&t.id).unwrap().status, Status::Connecting);
    assert_eq!(s.host.host_status().unwrap().queued, 1);
    s.client.cancel(&t.id).await.unwrap();
    assert_eq!(t.wait().await.status, Status::Cancelled);
    s.client.shutdown().await;
    s.host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_control() {
    let s = setup(2).await;
    std::fs::create_dir_all(s.root.join("2026/12A3")).unwrap();
    std::fs::write(s.root.join("2026/x.jpg"), b"xyz").unwrap();
    let admin = s.host.create_admin_ticket(0).unwrap();
    let admin: Ticket = admin.to_string().parse().unwrap();
    let ctl = s.client.ctl(admin.clone()).await.unwrap();
    let st = ctl.status().await.unwrap();
    assert_eq!(st.device, "host");
    assert_eq!(st.roots, vec!["RAW".to_string()]);
    let dir = ctl.list_dir("RAW", "2026").await.unwrap();
    assert_eq!(dir.len(), 2);
    assert!(dir[0].is_dir && dir[0].name == "12A3");
    assert_eq!((dir[1].name.as_str(), dir[1].size), ("x.jpg", 3));
    assert!(ctl.list_dir("RAW", "../..").await.is_err());
    assert!(ctl.list_dir("NOPE", "").await.is_err());
    let t = ctl
        .create_ticket(spec("remote job", "2026/12A3", ""))
        .await
        .unwrap();
    t.verify(&s.host.endpoint_id()).unwrap();
    assert!(ctl.create_ticket(spec("bad", "../etc", "")).await.is_err());
    let jobs = ctl.list_jobs().await.unwrap();
    assert!(jobs
        .iter()
        .any(|j| j.id == t.body.id && j.job == "remote job"));
    ctl.revoke(t.body.id).await.unwrap();
    assert!(s
        .host
        .list_jobs()
        .iter()
        .any(|j| j.id == t.body.id && j.revoked));
    // Re-signed ticket from the store is identical.
    assert_eq!(s.host.job_ticket(t.body.id).unwrap(), t);
    // A job ticket cannot be used as admin ticket.
    let job = s.host.create_ticket(spec("j", "", "")).unwrap();
    assert!(s.client.ctl(job).await.is_err());
    // Revoking the admin ticket cuts access.
    s.host.revoke(admin.body.id).unwrap();
    assert!(ctl.status().await.is_err());
    s.client.shutdown().await;
    s.host.shutdown().await;
}
