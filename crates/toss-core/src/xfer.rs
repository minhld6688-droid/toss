//! Shared data plane of `toss/xfer/1` and `toss/xfer/2`: sending files on uni
//! streams and receiving them into verified `.part` files. Used by both
//! directions (host get / client put send; client get / host put receive).
//!
//! xfer/1: one uni stream per file (`u32 index, u64 offset`, bytes to EOF).
//! xfer/2 (contracts/speed-v0.4.md §4.3): each uni stream carries segments
//! `u32 index, u64 offset, u64 len, <len bytes>` back to back; big files are
//! split into segments on parallel streams, small files batched per stream.

use std::{
    collections::HashMap,
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use iroh::endpoint::{Connection, RecvStream};
use parking_lot::{Condvar, Mutex};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, Notify},
    task::JoinSet,
};

use crate::{
    fsutil::{hash_prefix, HashCache},
    net::Tuning,
    paths::check_inside,
    transfer::Tracker,
    tuning::{self, MAX_BATCH_BYTES, MAX_BATCH_FILES, MAX_SEGMENT},
    wire::{
        decode_segment_header, encode_segment_header, encode_stream_header, read_stream_header,
        FileEntry, Manifest, SEG_HEADER_LEN,
    },
};

const CHUNK: usize = 256 * 1024;

/// A peer that makes no progress for this long (no chunk read/written, no
/// stream opened while none is running) is treated as gone. QUIC keep-alives
/// keep an idle connection open forever, so without this a stalled or
/// malicious peer would hold a host slot indefinitely.
static STALL_MS: AtomicU64 = AtomicU64::new(120_000);
/// Unauthenticated phase: from connection to the first message (hello / auth).
static PRE_AUTH_MS: AtomicU64 = AtomicU64::new(15_000);
/// Host waiting for the client's `Want` (the client may be hashing files it
/// already has, which can take a while on a big resumed job).
pub(crate) const WANT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Current stall timeout.
pub(crate) fn stall_timeout() -> Duration {
    Duration::from_millis(STALL_MS.load(Ordering::Relaxed))
}

/// Current pre-auth timeout.
pub(crate) fn pre_auth_timeout() -> Duration {
    Duration::from_millis(PRE_AUTH_MS.load(Ordering::Relaxed))
}

/// Overrides the stall and pre-auth timeouts (process-wide; tests only).
pub fn set_timeouts(stall: Duration, pre_auth: Duration) {
    STALL_MS.store(stall.as_millis() as u64, Ordering::Relaxed);
    PRE_AUTH_MS.store(pre_auth.as_millis() as u64, Ordering::Relaxed);
}

/// Runs a network step with the stall timeout; errors and timeouts are
/// connection problems (`Offline`, so the client retries).
pub(crate) async fn timed<T, E: Into<anyhow::Error>>(
    what: &str,
    limit: Duration,
    fut: impl Future<Output = Result<T, E>>,
) -> Result<T, SessErr> {
    match tokio::time::timeout(limit, fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(net(true)(e.into())),
        Err(_) => Err(net(true)(anyhow!(
            "{what}: no progress for {}s",
            limit.as_secs()
        ))),
    }
}

/// Checks a manifest received from a peer: sizes must add up to `total`
/// without overflow. Returns the total.
pub(crate) fn check_manifest(m: &Manifest) -> Result<u64> {
    let mut sum = 0u64;
    for f in &m.files {
        sum = sum
            .checked_add(f.size)
            .ok_or_else(|| anyhow!("manifest sizes overflow"))?;
    }
    if sum != m.total {
        bail!(
            "manifest total {} does not match the sum of its files ({sum})",
            m.total
        );
    }
    u32::try_from(m.files.len()).context("too many files in manifest")?;
    Ok(sum)
}

/// Session-level failure, classified for the retry policy.
#[derive(Debug)]
pub(crate) enum SessErr {
    /// Host refused the ticket.
    Denied(crate::wire::DenyReason),
    /// Network problem (dial failed or connection lost). `connected` = got past the hello.
    Offline { connected: bool, err: anyhow::Error },
    /// Local filesystem problem.
    Io(anyhow::Error),
    /// Protocol violation or anything else.
    Other(anyhow::Error),
}

impl std::fmt::Display for SessErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessErr::Denied(r) => write!(f, "denied: {r}"),
            SessErr::Offline { err, .. } => write!(f, "connection: {err:#}"),
            SessErr::Io(e) => write!(f, "I/O: {e:#}"),
            SessErr::Other(e) => write!(f, "{e:#}"),
        }
    }
}

/// Maps a network error.
pub(crate) fn net(connected: bool) -> impl Fn(anyhow::Error) -> SessErr {
    move |err| SessErr::Offline { connected, err }
}

/// One file to send.
#[derive(Clone, Debug)]
pub(crate) struct SendItem {
    pub index: u32,
    pub offset: u64,
    pub abs: PathBuf,
    pub size: u64,
}

/// Sends every item on its own uni stream, at most `max_parallel` at a time.
pub(crate) async fn send_files(
    conn: &Connection,
    items: Vec<SendItem>,
    max_parallel: usize,
    tracker: &Arc<Tracker>,
) -> Result<(), SessErr> {
    let max_parallel = max_parallel.max(1);
    let mut set: JoinSet<Result<(), SessErr>> = JoinSet::new();
    let mut iter = items.into_iter();
    loop {
        while set.len() < max_parallel {
            let Some(item) = iter.next() else { break };
            let conn = conn.clone();
            let tracker = tracker.clone();
            set.spawn(async move { send_one(&conn, item, &tracker).await });
        }
        match set.join_next().await {
            None => break,
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(e))) => return Err(e),
            Some(Err(join)) => return Err(SessErr::Other(anyhow!("send task failed: {join}"))),
        }
    }
    Ok(())
}

async fn send_one(conn: &Connection, item: SendItem, tracker: &Tracker) -> Result<(), SessErr> {
    let mut file = File::open(&item.abs)
        .await
        .with_context(|| format!("open {}", item.abs.display()))
        .map_err(SessErr::Io)?;
    if item.offset > 0 {
        file.seek(SeekFrom::Start(item.offset))
            .await
            .with_context(|| format!("seek {}", item.abs.display()))
            .map_err(SessErr::Io)?;
    }
    let stall = stall_timeout();
    let mut stream = timed("open stream", stall, conn.open_uni()).await?;
    timed(
        "write",
        stall,
        stream.write_all(&encode_stream_header(item.index, item.offset)),
    )
    .await?;
    let mut left = item.size.saturating_sub(item.offset);
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = usize::try_from(left.min(CHUNK as u64)).unwrap_or(CHUNK);
        let n = file
            .read(&mut buf[..want])
            .await
            .with_context(|| format!("read {}", item.abs.display()))
            .map_err(SessErr::Io)?;
        if n == 0 {
            // File shrank since the manifest; the receiver will notice the hash mismatch.
            tracing::warn!("{} shorter than announced", item.abs.display());
            break;
        }
        timed("write", stall, stream.write_all(&buf[..n])).await?;
        tracker.add_bytes(n as u64);
        left -= n as u64;
    }
    stream.finish().map_err(|e| net(true)(e.into()))?;
    tracker.file_done();
    Ok(())
}

/// One file to receive.
#[derive(Clone, Debug)]
pub(crate) struct RecvItem {
    pub entry: FileEntry,
    pub final_path: PathBuf,
    pub part: PathBuf,
    pub offset: u64,
    /// Display string for renames etc.
    pub label: String,
    /// Host side of a put: never replace a file that appeared at `final_path`
    /// while this one was being received (it is reported as a mismatch so the
    /// client retries and the host picks a free name).
    pub no_clobber: bool,
}

/// Outcome of [`receive_files`].
#[derive(Debug, Default)]
pub(crate) struct RecvSummary {
    pub files: u32,
    pub bytes: u64,
    /// Labels of files whose hash did not match (their `.part` was deleted).
    pub mismatched: Vec<String>,
}

/// Accepts exactly `items.len()` uni streams and writes each into its `.part`
/// file under `base_c` (canonical), verifies blake3 and renames into place.
pub(crate) async fn receive_files(
    conn: &Connection,
    base_c: &Path,
    items: HashMap<u32, RecvItem>,
    tracker: &Arc<Tracker>,
    hashes: &Arc<HashCache>,
) -> Result<RecvSummary, SessErr> {
    let expected = items.len();
    let items = Arc::new(Mutex::new(items));
    let base_c: Arc<PathBuf> = Arc::new(base_c.to_path_buf());
    let mut set: JoinSet<Result<RecvOne, SessErr>> = JoinSet::new();
    let mut accepted = 0usize;
    let mut summary = RecvSummary::default();
    loop {
        // While files are being received their own reads time out; with none
        // running, the sender must open the next stream within the stall timeout.
        let idle = set.is_empty();
        tokio::select! {
            r = async {
                if idle {
                    timed("waiting for next file", stall_timeout(), conn.accept_uni()).await
                } else {
                    conn.accept_uni().await.map_err(|e| net(true)(e.into()))
                }
            }, if accepted < expected => {
                let stream = r?;
                accepted += 1;
                let items = items.clone();
                let base_c = base_c.clone();
                let tracker = tracker.clone();
                let hashes = hashes.clone();
                set.spawn(async move { recv_one(stream, &items, &base_c, &tracker, &hashes).await });
            }
            Some(res) = set.join_next() => {
                match res {
                    Ok(Ok(RecvOne::Ok { bytes })) => { summary.files += 1; summary.bytes += bytes; }
                    Ok(Ok(RecvOne::Mismatch { label, bytes })) => {
                        summary.bytes += bytes;
                        summary.mismatched.push(label);
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(join) => return Err(SessErr::Other(anyhow!("receive task failed: {join}"))),
                }
            }
            else => break,
        }
    }
    Ok(summary)
}

enum RecvOne {
    Ok { bytes: u64 },
    Mismatch { label: String, bytes: u64 },
}

async fn recv_one(
    mut stream: RecvStream,
    items: &Mutex<HashMap<u32, RecvItem>>,
    base_c: &Path,
    tracker: &Tracker,
    hashes: &HashCache,
) -> Result<RecvOne, SessErr> {
    let stall = stall_timeout();
    let (index, offset) = timed("read", stall, read_stream_header(&mut stream)).await?;
    let item = items
        .lock()
        .remove(&index)
        .ok_or_else(|| SessErr::Other(anyhow!("unexpected stream for file #{index}")))?;
    if offset != item.offset {
        return Err(SessErr::Other(anyhow!(
            "stream for {} starts at {offset}, expected {}",
            item.label,
            item.offset
        )));
    }
    let size = item.entry.size;

    // Prepare the .part file (parents created and re-checked against symlink escapes).
    let part = item.part.clone();
    let base = base_c.to_path_buf();
    let hasher = tokio::task::spawn_blocking(move || -> Result<blake3::Hasher> {
        if let Some(parent) = part.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        check_inside(&base, &part)?;
        if offset > 0 {
            hash_prefix(&part, offset)
        } else {
            Ok(blake3::Hasher::new())
        }
    })
    .await
    .map_err(|e| SessErr::Other(e.into()))?
    .map_err(SessErr::Io)?;
    let mut hasher = hasher;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&item.part)
        .await
        .with_context(|| format!("open {}", item.part.display()))
        .map_err(SessErr::Io)?;
    let io =
        |e: std::io::Error, what: &str| SessErr::Io(anyhow!("{what} {}: {e}", item.part.display()));
    file.set_len(offset).await.map_err(|e| io(e, "truncate"))?;
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(|e| io(e, "seek"))?;

    let mut received = offset;
    let mut moved = 0u64;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = timed("read", stall, stream.read(&mut buf)).await?;
        let Some(n) = n else { break };
        if n == 0 {
            continue;
        }
        received += n as u64;
        if received > size {
            return Err(SessErr::Other(anyhow!(
                "{}: more data than announced",
                item.label
            )));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .await
            .map_err(|e| io(e, "write"))?;
        tracker.add_bytes(n as u64);
        moved += n as u64;
    }
    file.flush().await.map_err(|e| io(e, "flush"))?;
    file.sync_data().await.map_err(|e| io(e, "sync"))?;
    drop(file);

    let ok = received == size && *hasher.finalize().as_bytes() == item.entry.hash;
    if !ok {
        tracing::warn!("hash mismatch for {} ({received}/{size} bytes)", item.label);
        let _ = tokio::fs::remove_file(&item.part).await;
        tracker.sub_bytes(received);
        return Ok(RecvOne::Mismatch {
            label: item.label,
            bytes: moved,
        });
    }
    if item.no_clobber && tokio::fs::symlink_metadata(&item.final_path).await.is_ok() {
        tracing::warn!(
            "{} appeared while receiving {}; not replacing it",
            item.final_path.display(),
            item.label
        );
        let _ = tokio::fs::remove_file(&item.part).await;
        tracker.sub_bytes(received);
        return Ok(RecvOne::Mismatch {
            label: format!("{} (target appeared during transfer)", item.label),
            bytes: moved,
        });
    }
    tokio::fs::rename(&item.part, &item.final_path)
        .await
        .with_context(|| {
            format!(
                "rename {} -> {}",
                item.part.display(),
                item.final_path.display()
            )
        })
        .map_err(SessErr::Io)?;
    hashes.insert(&item.final_path, item.entry.hash);
    tracker.file_done();
    Ok(RecvOne::Ok { bytes: moved })
}

/// Decides how a receiver handles an existing `<final>.part`.
/// Returns `Some(offset)` to request from `offset`, or `None` if the part was
/// complete and has been renamed into place.
pub(crate) fn resume_offset(
    final_path: &Path,
    part: &Path,
    entry: &FileEntry,
    hashes: &HashCache,
) -> Result<Option<u64>> {
    let Ok(meta) = std::fs::symlink_metadata(part) else {
        return Ok(Some(0));
    };
    if !meta.is_file() {
        bail!("{} exists and is not a file", part.display());
    }
    let psize = meta.len();
    if psize < entry.size {
        return Ok(Some(psize));
    }
    if psize == entry.size && hashes.hash(part)? == entry.hash {
        std::fs::rename(part, final_path)
            .with_context(|| format!("rename {} -> {}", part.display(), final_path.display()))?;
        hashes.insert(final_path, entry.hash);
        return Ok(None);
    }
    std::fs::remove_file(part).with_context(|| format!("remove {}", part.display()))?;
    Ok(Some(0))
}

// ───────────────────────────── toss/xfer/2 ─────────────────────────────

const MIB: u64 = 1024 * 1024;
/// How often the sender re-checks path/RTT to pick its stream count.
const RETARGET: Duration = Duration::from_secs(2);
/// A multi-segment file's rolling hash catches up once this much new
/// contiguous data is on disk (read back from the page cache).
const CATCH_UP: u64 = 16 * MIB;

/// Pipeline settings of one xfer/2 transfer.
#[derive(Clone, Debug)]
pub(crate) struct Pipe {
    /// This node's tuning, clamped ([`tuning::sane`]).
    pub tuning: Tuning,
    /// Host `max_parallel_streams` cap (sender side of a get).
    pub cap: Option<u32>,
}

impl Pipe {
    pub fn new(t: &Tuning, cap: Option<u32>) -> Pipe {
        Pipe {
            tuning: tuning::sane(t),
            cap,
        }
    }

    fn chunk(&self) -> usize {
        self.tuning.chunk_kb as usize * 1024
    }
}

/// Last-progress clock shared by all streams of one transfer: one atomic
/// store per chunk instead of a timer around every read/write. A watchdog
/// fails the transfer after [`stall_timeout`] without progress.
#[derive(Debug)]
struct Watch {
    base: Instant,
    last_ms: AtomicU64,
}

impl Watch {
    fn new() -> Watch {
        Watch {
            base: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        self.last_ms
            .store(self.base.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle(&self) -> Duration {
        self.base
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

fn watchdog(stall: Duration) -> tokio::time::Interval {
    let period = (stall / 4).clamp(Duration::from_millis(50), Duration::from_secs(5));
    let mut iv = tokio::time::interval(period);
    iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    iv
}

/// One segment to send.
#[derive(Clone, Debug)]
struct Seg {
    index: u32,
    offset: u64,
    len: u64,
    abs: Arc<PathBuf>,
    /// Segments of this file not yet sent (the last one counts the file done).
    left: Arc<AtomicU32>,
}

/// Groups the wanted items into streams (each a list of segments), in want
/// order: files larger than `range_split_mb` become segments of at most
/// `min(range_split_mb, 64 MiB)`, each on its own stream; files smaller than
/// `small_file_kb` are batched (≤ 16 MiB, spread so that a batch-only job
/// still uses about `streams` streams); everything else is one segment on one
/// stream. Every item yields at least one segment (`len` 0 for nothing left).
fn plan_streams(items: Vec<SendItem>, t: &Tuning) -> Vec<Vec<Seg>> {
    let split = u64::from(t.range_split_mb) * MIB;
    let seg_len = split.clamp(1, MAX_SEGMENT);
    let small = u64::from(t.small_file_kb) * 1024;
    let small_total: u64 = items
        .iter()
        .filter(|i| i.size < small)
        .map(|i| i.size - i.offset.min(i.size))
        .sum();
    let batch_cap = (small_total / u64::from(t.streams.max(1)).max(1)).clamp(256 * 1024, MAX_BATCH_BYTES);
    let mut jobs = Vec::new();
    let mut batch: Vec<Seg> = Vec::new();
    let mut batch_bytes = 0u64;
    for it in items {
        let abs = Arc::new(it.abs);
        let offset = it.offset.min(it.size);
        let rem = it.size - offset;
        if it.size > split && rem > seg_len {
            let n = rem.div_ceil(seg_len);
            let left = Arc::new(AtomicU32::new(u32::try_from(n).unwrap_or(u32::MAX)));
            let mut off = offset;
            while off < it.size {
                let len = seg_len.min(it.size - off);
                jobs.push(vec![Seg {
                    index: it.index,
                    offset: off,
                    len,
                    abs: abs.clone(),
                    left: left.clone(),
                }]);
                off += len;
            }
            continue;
        }
        let seg = Seg {
            index: it.index,
            offset,
            len: rem,
            abs,
            left: Arc::new(AtomicU32::new(1)),
        };
        if it.size < small {
            if !batch.is_empty()
                && (batch_bytes + rem > batch_cap || batch.len() >= MAX_BATCH_FILES)
            {
                jobs.push(std::mem::take(&mut batch));
                batch_bytes = 0;
            }
            batch_bytes += rem;
            batch.push(seg);
        } else {
            jobs.push(vec![seg]);
        }
    }
    if !batch.is_empty() {
        jobs.push(batch);
    }
    jobs
}

/// xfer/2 sender: plans streams ([`plan_streams`]) and keeps up to
/// [`tuning::stream_target`] of them in flight (re-checked every 2 s).
pub(crate) async fn send_files_v2(
    conn: &Connection,
    items: Vec<SendItem>,
    pipe: &Pipe,
    tracker: &Arc<Tracker>,
) -> Result<(), SessErr> {
    let files = items.len();
    let jobs = plan_streams(items, &pipe.tuning);
    let mut target = tuning::stream_target(conn, &pipe.tuning, pipe.cap);
    tracing::debug!(
        id = %tracker.id,
        files,
        streams = jobs.len(),
        segments = jobs.iter().map(Vec::len).sum::<usize>(),
        target,
        "xfer/2 send plan"
    );
    let watch = Arc::new(Watch::new());
    let stall = stall_timeout();
    let chunk = pipe.chunk();
    let mut set: JoinSet<Result<(), SessErr>> = JoinSet::new();
    let mut jobs = jobs.into_iter().enumerate();
    let mut retarget = tokio::time::interval(RETARGET);
    retarget.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dog = watchdog(stall);
    loop {
        while set.len() < target {
            let Some((n, job)) = jobs.next() else { break };
            let conn = conn.clone();
            let tracker = tracker.clone();
            let watch = watch.clone();
            // Earlier streams first: the receiver's contiguous prefix (what a
            // resume keeps) then grows nearly in order, while later streams
            // still fill whatever the earlier ones leave unused.
            let prio = -i32::try_from(n).unwrap_or(i32::MAX);
            set.spawn(async move { send_stream(&conn, job, prio, chunk, &tracker, &watch).await });
        }
        if set.is_empty() {
            break;
        }
        tokio::select! {
            r = set.join_next() => match r {
                Some(Ok(Ok(()))) | None => {}
                Some(Ok(Err(e))) => return Err(e),
                Some(Err(join)) => return Err(SessErr::Other(anyhow!("send task failed: {join}"))),
            },
            _ = retarget.tick() => {
                let t = tuning::stream_target(conn, &pipe.tuning, pipe.cap);
                if t != target {
                    tracing::debug!(id = %tracker.id, from = target, to = t, "xfer/2 stream target changed");
                    target = t;
                }
            }
            _ = dog.tick() => {
                if watch.idle() >= stall {
                    return Err(net(true)(anyhow!("send: no progress for {}s", stall.as_secs())));
                }
            }
        }
    }
    Ok(())
}

/// What the blocking reader hands to the stream writer.
enum Piece {
    Header([u8; SEG_HEADER_LEN]),
    Data(Bytes),
    SegDone(Arc<AtomicU32>),
}

/// Sends one stream: a blocking reader thread reads the segments in
/// `chunk`-sized `Bytes` (prefetching a few) while this task writes them with
/// `write_chunk` (no copy). Ends once the peer acknowledged all of it, so
/// "streams in flight" really bounds the data in flight (and the receiver's
/// out-of-order window), not just what sits in the send buffer.
async fn send_stream(
    conn: &Connection,
    job: Vec<Seg>,
    priority: i32,
    chunk: usize,
    tracker: &Arc<Tracker>,
    watch: &Watch,
) -> Result<(), SessErr> {
    let mut stream = conn.open_uni().await.map_err(|e| net(true)(e.into()))?;
    let _ = stream.set_priority(priority);
    let _open = tracker.stream_open();
    watch.touch();
    let (tx, mut rx) = mpsc::channel::<Result<Piece>>(4);
    let reader = tokio::task::spawn_blocking(move || read_segments(job, chunk, &tx));
    let wr = |e: iroh::endpoint::WriteError| net(true)(e.into());
    while let Some(p) = rx.recv().await {
        match p.map_err(SessErr::Io)? {
            Piece::Header(h) => stream
                .write_chunk(Bytes::copy_from_slice(&h))
                .await
                .map_err(wr)?,
            Piece::Data(b) => {
                let n = b.len() as u64;
                stream.write_chunk(b).await.map_err(wr)?;
                tracker.add_bytes(n);
                watch.touch();
            }
            Piece::SegDone(left) => {
                if left.fetch_sub(1, Ordering::AcqRel) == 1 {
                    tracker.file_done();
                }
            }
        }
    }
    reader.await.map_err(|e| SessErr::Other(e.into()))?;
    stream.finish().map_err(|e| net(true)(e.into()))?;
    match stream.stopped().await {
        Ok(None) => Ok(()),
        Ok(Some(code)) => Err(net(true)(anyhow!("peer stopped a data stream ({code})"))),
        Err(e) => Err(net(true)(e.into())),
    }
}

fn read_segments(job: Vec<Seg>, chunk: usize, tx: &mpsc::Sender<Result<Piece>>) {
    for seg in job {
        let h = encode_segment_header(seg.index, seg.offset, seg.len);
        if tx.blocking_send(Ok(Piece::Header(h))).is_err() {
            return;
        }
        match read_segment(&seg, chunk, tx) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                let _ = tx.blocking_send(Err(e));
                return;
            }
        }
        if tx.blocking_send(Ok(Piece::SegDone(seg.left))).is_err() {
            return;
        }
    }
}

/// Reads one segment into `chunk`-sized pieces. `Ok(false)` = the writer is gone.
fn read_segment(seg: &Seg, chunk: usize, tx: &mpsc::Sender<Result<Piece>>) -> Result<bool> {
    if seg.len == 0 {
        return Ok(true);
    }
    let path = &*seg.abs;
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    f.seek(SeekFrom::Start(seg.offset))
        .with_context(|| format!("seek {}", path.display()))?;
    let mut left = seg.len;
    let mut short = false;
    while left > 0 {
        let want = usize::try_from(left.min(chunk as u64)).unwrap_or(chunk);
        let mut v = vec![0u8; want];
        if !short {
            let mut filled = 0;
            while filled < want {
                let n = f
                    .read(&mut v[filled..])
                    .with_context(|| format!("read {}", path.display()))?;
                if n == 0 {
                    // Shrunk since the manifest: the zero padding keeps the
                    // framing intact and fails the receiver's hash check.
                    tracing::warn!("{} shorter than announced", path.display());
                    short = true;
                    break;
                }
                filled += n;
            }
        }
        left -= want as u64;
        if tx.blocking_send(Ok(Piece::Data(Bytes::from(v)))).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Blocking writes (and reads for the rolling hash) in flight for one
/// receive. Closed before `.part` files are trimmed so that no late write
/// lands behind the cut.
#[derive(Debug, Default)]
struct Gate {
    st: Mutex<(bool, usize)>,
    cv: Condvar,
}

impl Gate {
    fn enter(&self) -> bool {
        let mut g = self.st.lock();
        if g.0 {
            return false;
        }
        g.1 += 1;
        true
    }

    fn exit(&self) {
        let mut g = self.st.lock();
        g.1 -= 1;
        if g.1 == 0 {
            self.cv.notify_all();
        }
    }

    /// Closes the gate and waits (≤ `limit`) for running work; true when none is left.
    fn close_and_wait(&self, limit: Duration) -> bool {
        let mut g = self.st.lock();
        g.0 = true;
        let deadline = Instant::now() + limit;
        while g.1 > 0 {
            if self.cv.wait_until(&mut g, deadline).timed_out() {
                break;
            }
        }
        g.1 == 0
    }
}

/// Leaves the gate when dropped (also on panic).
struct InGate(Arc<RecvState>);

impl Drop for InGate {
    fn drop(&mut self) {
        self.0.gate.exit();
    }
}

/// Rolling blake3 of a multi-segment file: the contiguous prefix is hashed
/// (read back from the page cache) as it grows, so finishing a 20 GB video
/// does not need a full re-read.
#[derive(Debug)]
struct Rolling {
    hasher: blake3::Hasher,
    upto: u64,
}

/// Receiver state of one wanted file.
#[derive(Debug)]
struct FileSlot {
    item: RecvItem,
    /// Bytes this session must deliver (`size - item.offset`).
    remaining: u64,
    prepared: tokio::sync::OnceCell<()>,
    st: Mutex<SlotState>,
    rolling: Mutex<Option<Rolling>>,
    catching_up: AtomicBool,
}

#[derive(Debug, Default)]
struct SlotState {
    /// Ranges announced by segment headers (never overlapping).
    claimed: Vec<(u64, u64)>,
    /// Ranges written to disk, sorted and merged.
    done: Vec<(u64, u64)>,
    done_bytes: u64,
    /// Some segment did not cover the whole remainder (rolling hash in use).
    multi: bool,
    finishing: bool,
    resolved: bool,
}

impl FileSlot {
    /// Validates a segment header against this file and claims its range.
    /// Returns true when the segment is the whole remainder (incremental hash).
    fn claim(&self, offset: u64, len: u64) -> Result<bool, SessErr> {
        let label = &self.item.label;
        let size = self.item.entry.size;
        let end = offset
            .checked_add(len)
            .filter(|e| *e <= size && offset >= self.item.offset)
            .ok_or_else(|| {
                SessErr::Other(anyhow!(
                    "{label}: segment {offset}+{len} outside {}..{size}",
                    self.item.offset
                ))
            })?;
        let whole = offset == self.item.offset && len == self.remaining;
        let mut st = self.st.lock();
        if st.finishing || st.resolved {
            return Err(SessErr::Other(anyhow!("{label}: segment after the file was complete")));
        }
        if len > 0 {
            if st.claimed.iter().any(|&(a, b)| offset < b && a < end) {
                return Err(SessErr::Other(anyhow!("{label}: overlapping segment at {offset}")));
            }
            st.claimed.push((offset, end));
        }
        if !whole {
            st.multi = true;
        }
        Ok(whole)
    }

    fn mark_done(&self, start: u64, end: u64) {
        let mut st = self.st.lock();
        st.done_bytes += end - start;
        let done = &mut st.done;
        let pos = done.partition_point(|&(a, _)| a < start);
        done.insert(pos, (start, end));
        // Merge neighbours (ranges never overlap, they may touch).
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(done.len());
        for &(a, b) in done.iter() {
            match merged.last_mut() {
                Some(last) if last.1 >= a => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        *done = merged;
    }

    /// End of the contiguous on-disk prefix (what a resume may trust).
    fn watermark(&self) -> u64 {
        let st = self.st.lock();
        let w = self.item.offset;
        match st.done.first() {
            Some(&(a, b)) if a <= w => b.max(w),
            _ => w,
        }
    }

    /// True for exactly one caller once every byte is on disk.
    fn try_finish(&self) -> bool {
        let mut st = self.st.lock();
        if !st.finishing && !st.resolved && st.done_bytes == self.remaining {
            st.finishing = true;
            true
        } else {
            false
        }
    }
}

/// Shared state of one xfer/2 receive.
#[derive(Debug)]
struct RecvState {
    files: HashMap<u32, Arc<FileSlot>>,
    base_c: PathBuf,
    /// Files not resolved (verified or mismatched) yet.
    pending: AtomicUsize,
    summary: Mutex<RecvSummary>,
    notify: Notify,
    gate: Gate,
    watch: Watch,
    tracker: Arc<Tracker>,
    hashes: Arc<HashCache>,
    chunk: usize,
}

/// On an unfinished receive (error, disconnect, pause, cancel): waits for the
/// blocking writes, then trims every unresolved `.part` to its contiguous
/// watermark so that resuming from the `.part` size stays correct.
struct Trim {
    st: Arc<RecvState>,
    armed: bool,
}

impl Drop for Trim {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if !self.st.gate.close_and_wait(Duration::from_secs(30)) {
            tracing::warn!("receive ended with disk writes still running; .part files not trimmed");
            return;
        }
        for slot in self.st.files.values() {
            if slot.st.lock().resolved {
                continue;
            }
            let w = slot.watermark();
            let Ok(f) = std::fs::OpenOptions::new()
                .write(true)
                .open(&slot.item.part)
            else {
                continue;
            };
            match f.metadata() {
                Ok(m) if m.len() > w => match f.set_len(w) {
                    Ok(()) => tracing::debug!(
                        "trimmed {} from {} to {w} bytes",
                        slot.item.part.display(),
                        m.len()
                    ),
                    Err(e) => tracing::warn!("trim {}: {e}", slot.item.part.display()),
                },
                _ => {}
            }
        }
    }
}

/// xfer/2 receiver: accepts data streams until every wanted file is verified
/// (or failed verification), writing each segment at its offset into the
/// file's `.part`.
pub(crate) async fn receive_files_v2(
    conn: &Connection,
    base_c: &Path,
    items: HashMap<u32, RecvItem>,
    pipe: &Pipe,
    tracker: &Arc<Tracker>,
    hashes: &Arc<HashCache>,
) -> Result<RecvSummary, SessErr> {
    let files: HashMap<u32, Arc<FileSlot>> = items
        .into_iter()
        .map(|(i, item)| {
            let remaining = item.entry.size.saturating_sub(item.offset);
            let slot = FileSlot {
                item,
                remaining,
                prepared: tokio::sync::OnceCell::new(),
                st: Mutex::new(SlotState::default()),
                rolling: Mutex::new(None),
                catching_up: AtomicBool::new(false),
            };
            (i, Arc::new(slot))
        })
        .collect();
    let st = Arc::new(RecvState {
        pending: AtomicUsize::new(files.len()),
        files,
        base_c: base_c.to_path_buf(),
        summary: Mutex::new(RecvSummary::default()),
        notify: Notify::new(),
        gate: Gate::default(),
        watch: Watch::new(),
        tracker: tracker.clone(),
        hashes: hashes.clone(),
        chunk: pipe.chunk(),
    });
    let mut trim = Trim {
        st: st.clone(),
        armed: true,
    };
    let stall = stall_timeout();
    let mut dog = watchdog(stall);
    let mut set: JoinSet<Result<(), SessErr>> = JoinSet::new();
    while st.pending.load(Ordering::Acquire) > 0 {
        tokio::select! {
            r = conn.accept_uni() => {
                let stream = r.map_err(|e| net(true)(e.into()))?;
                st.watch.touch();
                let st = st.clone();
                set.spawn(async move { recv_stream(stream, st).await });
            }
            Some(res) = set.join_next() => match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(join) => return Err(SessErr::Other(anyhow!("receive task failed: {join}"))),
            },
            _ = st.notify.notified() => {}
            _ = dog.tick() => {
                if st.watch.idle() >= stall {
                    return Err(net(true)(anyhow!("receive: no progress for {}s", stall.as_secs())));
                }
            }
        }
    }
    trim.armed = false;
    set.abort_all();
    let summary = std::mem::take(&mut *st.summary.lock());
    Ok(summary)
}

/// Reads a segment header; `None` at a clean end of stream.
async fn read_seg_header(stream: &mut RecvStream) -> Result<Option<[u8; SEG_HEADER_LEN]>, SessErr> {
    let mut h = [0u8; SEG_HEADER_LEN];
    let mut filled = 0;
    while filled < h.len() {
        match stream
            .read(&mut h[filled..])
            .await
            .map_err(|e| net(true)(e.into()))?
        {
            Some(n) => filled += n,
            None if filled == 0 => return Ok(None),
            None => return Err(SessErr::Other(anyhow!("stream ended inside a segment header"))),
        }
    }
    Ok(Some(h))
}

async fn recv_stream(mut stream: RecvStream, st: Arc<RecvState>) -> Result<(), SessErr> {
    let _open = st.tracker.stream_open();
    let mut bufs: Vec<Vec<u8>> = Vec::new();
    while let Some(h) = read_seg_header(&mut stream).await? {
        st.watch.touch();
        let (index, offset, len) = decode_segment_header(&h);
        let slot = st
            .files
            .get(&index)
            .cloned()
            .ok_or_else(|| SessErr::Other(anyhow!("segment for unexpected file #{index}")))?;
        let whole = slot.claim(offset, len)?;
        prepare(&st, &slot, whole).await?;
        let mut hasher = None;
        if whole {
            let part = slot.item.part.clone();
            let prefix = slot.item.offset;
            let st2 = st.clone();
            hasher = Some(
                blocking(move || hash_range(&part, blake3::Hasher::new(), 0, prefix, &st2.watch))
                    .await?,
            );
        }
        if len > 0 {
            hasher = recv_segment(&mut stream, &st, &slot, offset, len, hasher, &mut bufs).await?;
        }
        if slot.try_finish() {
            finalize(&st, &slot, hasher).await?;
        }
    }
    Ok(())
}

/// Runs blocking file work; its errors are local I/O errors.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T, SessErr> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| SessErr::Other(e.into()))?
        .map_err(SessErr::Io)
}

/// Feeds `[from, to)` of `path` into `h`, touching the watch per MiB.
fn hash_range(
    path: &Path,
    mut h: blake3::Hasher,
    from: u64,
    to: u64,
    watch: &Watch,
) -> Result<blake3::Hasher> {
    if to <= from {
        return Ok(h);
    }
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    f.seek(SeekFrom::Start(from))?;
    let mut left = to - from;
    let mut buf = vec![0u8; MIB as usize];
    while left > 0 {
        let want = usize::try_from(left.min(MIB)).unwrap_or(MIB as usize);
        f.read_exact(&mut buf[..want])
            .with_context(|| format!("read {}", path.display()))?;
        h.update(&buf[..want]);
        left -= want as u64;
        watch.touch();
    }
    Ok(h)
}

/// First segment of a file: creates parents, checks the path, cuts the
/// `.part` to the resume offset; a multi-segment file gets its rolling hash
/// (prefix already on disk hashed).
async fn prepare(st: &Arc<RecvState>, slot: &Arc<FileSlot>, whole: bool) -> Result<(), SessErr> {
    slot.prepared
        .get_or_try_init(|| async {
            let part = slot.item.part.clone();
            let base = st.base_c.clone();
            let offset = slot.item.offset;
            blocking(move || {
                if let Some(parent) = part.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create {}", parent.display()))?;
                }
                check_inside(&base, &part)?;
                let f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&part)
                    .with_context(|| format!("open {}", part.display()))?;
                f.set_len(offset)
                    .with_context(|| format!("truncate {}", part.display()))?;
                Ok(())
            })
            .await?;
            if !whole {
                let part = slot.item.part.clone();
                let st2 = st.clone();
                let h = blocking(move || {
                    hash_range(&part, blake3::Hasher::new(), 0, offset, &st2.watch)
                })
                .await?;
                *slot.rolling.lock() = Some(Rolling { hasher: h, upto: offset });
            }
            Ok(())
        })
        .await
        .map(|_| ())
}

/// Result of one blocking chunk write.
struct Written {
    file: std::fs::File,
    hasher: Option<blake3::Hasher>,
    buf: Vec<u8>,
    start: u64,
    len: usize,
    res: std::io::Result<()>,
}

/// Receives one segment's bytes: fills `chunk`-sized buffers from the stream
/// and writes them on a blocking thread (double-buffered: the next buffer
/// fills while the previous one is written and, for a whole-file segment,
/// hashed).
#[allow(clippy::too_many_arguments)]
async fn recv_segment(
    stream: &mut RecvStream,
    st: &Arc<RecvState>,
    slot: &Arc<FileSlot>,
    offset: u64,
    len: u64,
    mut hasher: Option<blake3::Hasher>,
    bufs: &mut Vec<Vec<u8>>,
) -> Result<Option<blake3::Hasher>, SessErr> {
    let part = slot.item.part.clone();
    let mut file = Some(
        blocking(move || {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(&part)
                .with_context(|| format!("open {}", part.display()))?;
            f.seek(SeekFrom::Start(offset))
                .with_context(|| format!("seek {}", part.display()))?;
            Ok(f)
        })
        .await?,
    );
    let chunk = st.chunk;
    let end = offset + len;
    let mut pos = offset;
    let mut pending: Option<tokio::task::JoinHandle<Written>> = None;
    let mut buf = bufs.pop().unwrap_or_else(|| vec![0u8; chunk]);
    while pos < end {
        let want = usize::try_from((end - pos).min(chunk as u64)).unwrap_or(chunk);
        let mut filled = 0;
        while filled < want {
            match stream
                .read(&mut buf[filled..want])
                .await
                .map_err(|e| net(true)(e.into()))?
            {
                Some(n) => filled += n,
                None => {
                    return Err(SessErr::Other(anyhow!(
                        "{}: stream ended inside a segment",
                        slot.item.label
                    )))
                }
            }
        }
        st.watch.touch();
        if let Some(p) = pending.take() {
            let w = written(p, st, slot).await?;
            file = Some(w.file);
            hasher = w.hasher;
            bufs.push(w.buf);
        }
        if !st.gate.enter() {
            return Err(SessErr::Other(anyhow!("receive ended")));
        }
        let guard = InGate(st.clone());
        let mut f = file.take().expect("file handle is back");
        let mut h = hasher.take();
        let data = std::mem::replace(&mut buf, bufs.pop().unwrap_or_else(|| vec![0u8; chunk]));
        let start = pos;
        pending = Some(tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let res = f.write_all(&data[..want]);
            if let (Ok(()), Some(h)) = (&res, h.as_mut()) {
                h.update(&data[..want]);
            }
            Written {
                file: f,
                hasher: h,
                buf: data,
                start,
                len: want,
                res,
            }
        }));
        pos += want as u64;
    }
    if let Some(p) = pending.take() {
        let w = written(p, st, slot).await?;
        hasher = w.hasher;
        bufs.push(w.buf);
    }
    bufs.push(buf);
    bufs.truncate(2);
    Ok(hasher)
}

/// Completes a chunk write: records the range, counts the bytes, advances the
/// rolling hash of a multi-segment file.
async fn written(
    p: tokio::task::JoinHandle<Written>,
    st: &Arc<RecvState>,
    slot: &Arc<FileSlot>,
) -> Result<Written, SessErr> {
    let w = p.await.map_err(|e| SessErr::Other(e.into()))?;
    if let Err(e) = &w.res {
        return Err(SessErr::Io(anyhow!(
            "write {}: {e}",
            slot.item.part.display()
        )));
    }
    slot.mark_done(w.start, w.start + w.len as u64);
    st.tracker.add_bytes(w.len as u64);
    st.watch.touch();
    maybe_catch_up(st, slot);
    Ok(w)
}

/// Starts a background catch-up of the rolling hash when enough new
/// contiguous data is on disk (one at a time per file).
fn maybe_catch_up(st: &Arc<RecvState>, slot: &Arc<FileSlot>) {
    if slot.catching_up.load(Ordering::Acquire) {
        return;
    }
    // Never block this (async) thread on a running catch-up.
    let upto = match slot.rolling.try_lock().as_deref() {
        Some(Some(r)) => r.upto,
        _ => return,
    };
    let w = slot.watermark();
    if w <= upto || w - upto < CATCH_UP {
        return;
    }
    if slot.catching_up.swap(true, Ordering::AcqRel) {
        return;
    }
    if !st.gate.enter() {
        slot.catching_up.store(false, Ordering::Release);
        return;
    }
    let guard = InGate(st.clone());
    let slot = slot.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let st = &_guard.0;
        let mut r = slot.rolling.lock();
        if let Some(roll) = r.as_mut() {
            let w = slot.watermark();
            let h = std::mem::take(&mut roll.hasher);
            match hash_range(&slot.item.part, h, roll.upto, w, &st.watch) {
                Ok(h) => {
                    roll.hasher = h;
                    roll.upto = w;
                }
                Err(e) => {
                    // Leave it to the final verification (which re-reads).
                    tracing::debug!("rolling hash of {}: {e:#}", slot.item.label);
                    *r = None;
                }
            }
        }
        drop(r);
        slot.catching_up.store(false, Ordering::Release);
    });
}

enum Fin {
    Ok,
    Mismatch(String),
}

/// All bytes of a file are on disk: sync once, verify blake3, rename (never
/// over a target that appeared meanwhile when `no_clobber`).
async fn finalize(
    st: &Arc<RecvState>,
    slot: &Arc<FileSlot>,
    hasher: Option<blake3::Hasher>,
) -> Result<(), SessErr> {
    let st2 = st.clone();
    let slot2 = slot.clone();
    let fin = blocking(move || {
        let item = &slot2.item;
        let part = &item.part;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(part)
            .with_context(|| format!("open {}", part.display()))?;
        f.sync_data()
            .with_context(|| format!("sync {}", part.display()))?;
        let len = f.metadata()?.len();
        drop(f);
        let hash = match hasher {
            Some(h) => *h.finalize().as_bytes(),
            None => {
                // Multi-segment file: finish the rolling hash (waits for a
                // running catch-up), or re-read the whole file without one.
                let roll = slot2.rolling.lock().take();
                let (h, from) = match roll {
                    Some(r) => (r.hasher, r.upto),
                    None => (blake3::Hasher::new(), 0),
                };
                *hash_range(part, h, from, len, &st2.watch)?
                    .finalize()
                    .as_bytes()
            }
        };
        if len != item.entry.size || hash != item.entry.hash {
            tracing::warn!("hash mismatch for {} ({len}/{} bytes)", item.label, item.entry.size);
            let _ = std::fs::remove_file(part);
            return Ok(Fin::Mismatch(item.label.clone()));
        }
        if item.no_clobber && std::fs::symlink_metadata(&item.final_path).is_ok() {
            tracing::warn!(
                "{} appeared while receiving {}; not replacing it",
                item.final_path.display(),
                item.label
            );
            let _ = std::fs::remove_file(part);
            return Ok(Fin::Mismatch(format!(
                "{} (target appeared during transfer)",
                item.label
            )));
        }
        std::fs::rename(part, &item.final_path).with_context(|| {
            format!("rename {} -> {}", part.display(), item.final_path.display())
        })?;
        st2.hashes.insert(&item.final_path, item.entry.hash);
        Ok(Fin::Ok)
    })
    .await?;
    let moved = slot.remaining;
    {
        let mut sum = st.summary.lock();
        sum.bytes += moved;
        match fin {
            Fin::Ok => {
                sum.files += 1;
                st.tracker.file_done();
            }
            Fin::Mismatch(label) => {
                sum.mismatched.push(label);
                st.tracker.sub_bytes(slot.item.entry.size);
            }
        }
    }
    slot.st.lock().resolved = true;
    st.pending.fetch_sub(1, Ordering::AcqRel);
    st.notify.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_checks() {
        let f = |size| FileEntry {
            rel: "a".into(),
            size,
            hash: [0; 32],
        };
        let ok = Manifest {
            files: vec![f(3), f(4)],
            total: 7,
        };
        assert_eq!(check_manifest(&ok).unwrap(), 7);
        let lie = Manifest {
            files: vec![f(3), f(4)],
            total: 1,
        };
        assert!(check_manifest(&lie).is_err());
        let overflow = Manifest {
            files: vec![f(u64::MAX), f(2)],
            total: 1,
        };
        assert!(check_manifest(&overflow).is_err());
    }
}
