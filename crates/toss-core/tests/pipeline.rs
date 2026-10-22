//! v0.4 pipeline tests (contracts/speed-v0.4.md §4, §6): `toss/xfer/2`
//! segmented get/put, batching of small files, resume after a disconnect with
//! out-of-order segments, the exact xfer/2 wire format, and compatibility
//! with `toss/xfer/1` peers in both directions. Two nodes per test over
//! loopback; own test binary because it shortens the process-wide stall timeout.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use iroh::{EndpointAddr, TransportAddr};
use toss_core::{
    transfer::Status,
    wire::{
        decode_segment_header, read_msg, write_msg, ClientHello, Done, HostHello, Manifest, Op,
        Want, WantItem, SEG_HEADER_LEN,
    },
    HostConfig, Node, NodeConfig, Root, TicketSpec, TransferEvent, Tuning, ALPN_CTL, ALPN_XFER,
    ALPN_XFER2,
};

const STALL: Duration = Duration::from_secs(10);
const MIB: usize = 1024 * 1024;

fn init() {
    toss_core::testing::set_timeouts(STALL, Duration::from_secs(15));
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
        chunk_kb: 256,
        ..Tuning::default()
    }
}

async fn node(dir: &Path, name: &str, tuning: Tuning) -> Node {
    let mut cfg = NodeConfig {
        data_dir: dir.join(format!("{name}-data")),
        device_name: name.to_string(),
        app_version: "test".into(),
        net: Default::default(),
    };
    cfg.net.tuning = tuning;
    Node::start(cfg).await.expect("start node")
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

/// Pseudo-random bytes, distinct per seed and per MiB (fast to generate).
fn write_data(path: &Path, seed: u64, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut block = vec![0u8; MIB.min(len.max(1))];
    for b in block.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 24) as u8;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut left = len;
    let mut n = 0u64;
    while left > 0 {
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

struct Setup {
    _tmp: tempfile::TempDir,
    host: Node,
    client: Node,
    root: PathBuf,
    ret: PathBuf,
    dest: PathBuf,
    work: PathBuf,
}

async fn setup(host_tuning: Tuning, client_tuning: Tuning) -> Setup {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path().to_path_buf();
    let (root, ret, dest, work) = (t.join("root"), t.join("return"), t.join("dest"), t.join("work"));
    for d in [&root, &dest, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    let host = node(&t, "host", host_tuning).await;
    let client = node(&t, "client", client_tuning).await;
    host.set_host(Some(HostConfig::new(
        vec![Root {
            alias: "RAW".into(),
            path: root.clone(),
        }],
        ret.clone(),
    )))
    .await
    .unwrap();
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

/// A job folder: one file split into ≥ 3 segments, a medium file, 60 small
/// files (batched) in nested folders, an empty file.
fn make_tree(base: &Path) -> Vec<String> {
    let mut rels = Vec::new();
    let mut add = |rel: &str, seed: u64, len: usize| {
        write_data(&base.join(rel), seed, len);
        rels.push(rel.to_string());
    };
    add("video.mp4", 1, 5 * MIB + 12_345);
    add("medium.CR3", 2, 300 * 1024);
    add("empty.xmp", 3, 0);
    for i in 0..60u64 {
        let dir = if i % 3 == 0 { "sub/a" } else { "sub/b/c" };
        add(&format!("{dir}/IMG_{i:04}.xmp"), 100 + i, 1 + (i as usize * 997) % 20_000);
    }
    rels.sort();
    rels
}

fn assert_same(src: &Path, dst: &Path, rels: &[String]) {
    for rel in rels {
        let a = std::fs::read(src.join(rel)).unwrap();
        let b = std::fs::read(dst.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(a.len(), b.len(), "{rel} size");
        assert!(a == b, "{rel} differs");
        assert_eq!(blake3::hash(&a), blake3::hash(&b));
    }
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn rec(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                rec(base, &p, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    rec(dir, dir, &mut out);
    out.sort();
    out
}

async fn wait_done(t: &toss_core::Transfer) -> toss_core::TransferState {
    let st = tokio::time::timeout(Duration::from_secs(120), t.wait())
        .await
        .expect("transfer timed out");
    assert_eq!(st.status, Status::Done, "{st:?}");
    st
}

/// Get and put over xfer/2 with segmented, batched and empty files; a second
/// put of a changed file lands under a numbered name (host never overwrites).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_get_put_segments_and_batches() {
    let s = setup(small_tuning(), small_tuning()).await;
    let src = s.root.join("job");
    let rels = make_tree(&src);
    let t = s.host.create_ticket(spec("Job 1", "job", "back")).unwrap();

    let mut rx = s.client.subscribe();
    let tr = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    let st = wait_done(&tr).await;
    let got = s.dest.join("Job 1");
    assert_eq!(files_under(&got), rels);
    assert_same(&src, &got, &rels);
    assert_eq!(st.files_done, rels.len() as u32);
    assert_eq!(st.bytes_done, st.bytes_total);
    // Progress carries the v0.4 fields (streams drop back to 0 at the end).
    while let Ok(ev) = rx.try_recv() {
        if let TransferEvent::Progress { streams, .. } = ev {
            assert!(streams <= 64);
        }
    }

    // Put the same tree back.
    let tr = s.client.put(t.clone(), vec![got.clone()]).await.unwrap();
    wait_done(&tr).await;
    let back = s.ret.join("back/Job 1");
    assert_same(&src, &back, &rels);
    assert_eq!(files_under(&back), rels);

    // Changed big file with the same name: lands as "video (2).mp4".
    let changed = s.work.join("video.mp4");
    write_data(&changed, 77, 3 * MIB + 5);
    let tr = s.client.put(t.clone(), vec![changed.clone()]).await.unwrap();
    wait_done(&tr).await;
    assert_eq!(
        std::fs::read(s.ret.join("back/video.mp4")).unwrap(),
        std::fs::read(&changed).unwrap()
    );
    let tr = s.client.put(t, vec![src.join("video.mp4")]).await.unwrap();
    wait_done(&tr).await;
    assert_eq!(
        std::fs::read(s.ret.join("back/video (2).mp4")).unwrap(),
        std::fs::read(src.join("video.mp4")).unwrap()
    );
    assert_eq!(
        std::fs::read(s.ret.join("back/video.mp4")).unwrap(),
        std::fs::read(&changed).unwrap(),
        "host overwrote a complete file"
    );
}

/// The exact xfer/2 data-stream format, read by a raw client: segments
/// `u32 index, u64 offset, u64 len, bytes` back to back; the big file arrives
/// in ≥ 3 segments, small files share streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_wire_format() {
    let s = setup(small_tuning(), small_tuning()).await;
    let src = s.root.join("job");
    let rels = make_tree(&src);
    let t = s.host.create_ticket(spec("raw", "job", "")).unwrap();
    let conn = s
        .client
        .endpoint()
        .connect(loopback_addr(&s.host), ALPN_XFER2)
        .await
        .unwrap();
    assert_eq!(conn.alpn(), ALPN_XFER2);
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let hello = ClientHello::v1 {
        ticket: t,
        device: "raw".into(),
        op: Op::Get,
        app_version: "raw".into(),
    };
    write_msg(&mut send, &hello).await.unwrap();
    let h: HostHello = read_msg(&mut recv).await.unwrap();
    assert!(matches!(h, HostHello::Ok { .. }), "{h:?}");
    let m: Manifest = read_msg(&mut recv).await.unwrap();
    assert_eq!(m.files.len(), rels.len());
    // Resume the big file from 1000 bytes in.
    let big = m.files.iter().position(|f| f.rel == "video.mp4").unwrap() as u32;
    let items = (0..m.files.len() as u32)
        .map(|index| WantItem {
            index,
            offset: if index == big { 1000 } else { 0 },
        })
        .collect();
    write_msg(&mut send, &Want { items }).await.unwrap();

    // index -> (offset -> bytes); segments per stream.
    let mut got: BTreeMap<u32, BTreeMap<u64, Vec<u8>>> = BTreeMap::new();
    let mut per_stream = Vec::new();
    let total: u64 = m.files.iter().map(|f| f.size).sum::<u64>() - 1000;
    let mut received = 0u64;
    let mut seen_files = BTreeSet::new();
    while received < total || seen_files.len() < m.files.len() {
        let mut uni = tokio::time::timeout(Duration::from_secs(30), conn.accept_uni())
            .await
            .unwrap()
            .unwrap();
        let data = uni.read_to_end(64 * MIB).await.unwrap();
        let mut p = 0;
        let mut segs = 0;
        while p < data.len() {
            let h: [u8; SEG_HEADER_LEN] = data[p..p + SEG_HEADER_LEN].try_into().unwrap();
            let (index, offset, len) = decode_segment_header(&h);
            p += SEG_HEADER_LEN;
            let bytes = data[p..p + len as usize].to_vec();
            p += len as usize;
            received += len;
            segs += 1;
            seen_files.insert(index);
            assert!(got.entry(index).or_default().insert(offset, bytes).is_none());
        }
        per_stream.push(segs);
    }
    let big_segs = &got[&big];
    assert!(big_segs.len() >= 3, "big file in {} segments", big_segs.len());
    assert_eq!(*big_segs.keys().next().unwrap(), 1000);
    assert!(
        per_stream.iter().any(|n| *n >= 10),
        "small files were not batched: {per_stream:?}"
    );
    for (index, segs) in &got {
        let f = &m.files[*index as usize];
        let start = if *index == big { 1000 } else { 0 };
        let mut joined = Vec::new();
        let mut next = start;
        for (off, b) in segs {
            assert_eq!(*off, next, "{} has a gap", f.rel);
            next += b.len() as u64;
            joined.extend_from_slice(b);
        }
        assert_eq!(next, f.size, "{}", f.rel);
        let disk = std::fs::read(src.join(&f.rel)).unwrap();
        assert!(disk[start as usize..] == joined[..], "{} content", f.rel);
    }
    write_msg(
        &mut send,
        &Done {
            files: m.files.len() as u32,
            bytes: total,
            ok: true,
            msg: String::new(),
        },
    )
    .await
    .unwrap();
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
}

/// Polls until `part` exists with at least `min` bytes; panics if the
/// transfer finished first.
async fn wait_part(part: &Path, done: &Path, min: u64) {
    let t0 = Instant::now();
    loop {
        if std::fs::metadata(part).is_ok_and(|m| m.len() >= min) {
            return;
        }
        assert!(
            !done.exists(),
            "transfer finished before it could be interrupted"
        );
        assert!(t0.elapsed() < Duration::from_secs(60), "no progress on {}", part.display());
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// After an interrupted receive the `.part` must be a correct prefix of the
/// source (out-of-order segments beyond the contiguous watermark trimmed).
fn assert_prefix(part: &Path, src: &Path) -> u64 {
    let p = std::fs::read(part).unwrap_or_default();
    let s = std::fs::read(src).unwrap();
    assert!(p.len() <= s.len());
    assert!(
        s[..p.len()] == p[..],
        "{} is not a prefix of the source ({} bytes)",
        part.display(),
        p.len()
    );
    p.len() as u64
}

async fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Get and put of a file split into many 1 MiB segments on 8 streams,
/// interrupted mid-transfer (pause = connection closed) and resumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_resume_after_disconnect() {
    let tuning = Tuning {
        streams: 8,
        streams_high_rtt: 8,
        ..small_tuning()
    };
    let s = setup(tuning.clone(), tuning).await;
    let src = s.root.join("big/clip.mov");
    write_data(&src, 9, 160 * MIB + 333);
    let t = s.host.create_ticket(spec("Big", "big", "up")).unwrap();

    // ---- get ----
    let tr = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    let final_path = s.dest.join("Big/clip.mov");
    let part = s.dest.join("Big/clip.mov.part");
    wait_part(&part, &final_path, 24 * MIB as u64).await;
    s.client.pause(&tr.id).await.unwrap();
    assert_eq!(s.client.transfer(&tr.id).unwrap().status, Status::Paused);
    let kept = assert_prefix(&part, &src);
    eprintln!("get: .part kept {kept} bytes after the disconnect");
    assert!(kept > 0, "the contiguous prefix was lost");
    s.client.resume(&tr.id).await.unwrap();
    wait_done(&tr).await;
    assert_eq!(
        std::fs::read(&final_path).unwrap(),
        std::fs::read(&src).unwrap()
    );
    assert!(!part.exists());

    // ---- put (host side receives, trims on disconnect) ----
    let up = s.work.join("clip.mov");
    std::fs::copy(&src, &up).unwrap();
    let tr = s.client.put(t, vec![up.clone()]).await.unwrap();
    let hfinal = s.ret.join("up/clip.mov");
    let hpart = s.ret.join("up/clip.mov.part");
    wait_part(&hpart, &hfinal, 24 * MIB as u64).await;
    s.client.pause(&tr.id).await.unwrap();
    // The host notices the closed connection and trims its .part.
    wait_until("host session end", Duration::from_secs(30), || {
        s.host.host_status().unwrap().active == 0
    })
    .await;
    let kept = assert_prefix(&hpart, &up);
    eprintln!("put: host .part kept {kept} bytes after the disconnect");
    assert!(kept > 0, "the host lost the contiguous prefix");
    s.client.resume(&tr.id).await.unwrap();
    wait_done(&tr).await;
    assert_eq!(std::fs::read(&hfinal).unwrap(), std::fs::read(&up).unwrap());
    assert!(!hpart.exists());
    assert!(!s.ret.join("up/clip (2).mov").exists());
}

/// A v0.3 (xfer/1-only) client against a v0.4 host: get and put.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn xfer1_client_with_v2_host() {
    let s = setup(small_tuning(), small_tuning()).await;
    toss_core::testing::force_xfer1_client(s.client.endpoint_id(), true);
    let src = s.root.join("job");
    let rels = make_tree(&src);
    let t = s.host.create_ticket(spec("Old client", "job", "")).unwrap();
    let tr = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    wait_done(&tr).await;
    let got = s.dest.join("Old client");
    assert_same(&src, &got, &rels);
    let tr = s.client.put(t, vec![got]).await.unwrap();
    wait_done(&tr).await;
    assert_same(&src, &s.ret.join("Old client/Old client"), &rels);
    toss_core::testing::force_xfer1_client(s.client.endpoint_id(), false);
}

/// A v0.4 client against a v0.3 host (offers only xfer/1): falls back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_client_with_xfer1_only_host() {
    let s = setup(small_tuning(), small_tuning()).await;
    s.host
        .endpoint()
        .set_alpns(vec![ALPN_XFER.to_vec(), ALPN_CTL.to_vec()]);
    // A raw xfer/2 dial is refused by such a host.
    let refused = s
        .client
        .endpoint()
        .connect(loopback_addr(&s.host), ALPN_XFER2)
        .await;
    assert!(refused.is_err(), "xfer/1-only host accepted xfer/2");
    let src = s.root.join("job");
    let rels = make_tree(&src);
    let t = s.host.create_ticket(spec("Old host", "job", "")).unwrap();
    let tr = s.client.get(t.clone(), s.dest.clone()).await.unwrap();
    wait_done(&tr).await;
    let got = s.dest.join("Old host");
    assert_same(&src, &got, &rels);
    let tr = s.client.put(t, vec![got]).await.unwrap();
    wait_done(&tr).await;
    assert_same(&src, &s.ret.join("Old host/Old host"), &rels);
}

/// An xfer/2 client that never reads its data streams frees the host slot
/// after the stall timeout (watchdog instead of per-chunk timers).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_stalled_receiver_frees_the_slot() {
    let s = setup(small_tuning(), small_tuning()).await;
    let f = s.root.join("big/big.bin");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::File::create(&f).unwrap().set_len(256 << 20).unwrap();
    let t = s.host.create_ticket(spec("big", "big", "")).unwrap();
    let conn = s
        .client
        .endpoint()
        .connect(loopback_addr(&s.host), ALPN_XFER2)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let hello = ClientHello::v1 {
        ticket: t,
        device: "raw".into(),
        op: Op::Get,
        app_version: "raw".into(),
    };
    write_msg(&mut send, &hello).await.unwrap();
    let _: HostHello = read_msg(&mut recv).await.unwrap();
    let _: Manifest = read_msg(&mut recv).await.unwrap();
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
    wait_until("slot taken", Duration::from_secs(10), || {
        s.host.host_status().unwrap().active == 1
    })
    .await;
    wait_until("slot freed", STALL * 6, || {
        s.host.host_status().unwrap().active == 0
    })
    .await;
    drop(conn);
}
