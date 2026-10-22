//! Shared data-plane pieces of `toss/xfer/1`: sending files on uni streams and
//! receiving them into verified `.part` files. Used by both directions
//! (host get / client put send; client get / host put receive).

use std::{
    collections::HashMap,
    future::Future,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::{Connection, RecvStream};
use parking_lot::Mutex;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    task::JoinSet,
};

use crate::{
    fsutil::{hash_prefix, HashCache},
    paths::check_inside,
    transfer::Tracker,
    wire::{encode_stream_header, read_stream_header, FileEntry, Manifest},
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
