//! Shared data-plane pieces of `toss/xfer/1`: sending files on uni streams and
//! receiving them into verified `.part` files. Used by both directions
//! (host get / client put send; client get / host put receive).

use std::{
    collections::HashMap,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::Arc,
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
    wire::{encode_stream_header, read_stream_header, FileEntry},
};

const CHUNK: usize = 256 * 1024;

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
    let mut stream = conn.open_uni().await.map_err(|e| net(true)(e.into()))?;
    stream
        .write_all(&encode_stream_header(item.index, item.offset))
        .await
        .map_err(|e| net(true)(e.into()))?;
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
        stream
            .write_all(&buf[..n])
            .await
            .map_err(|e| net(true)(e.into()))?;
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
        tokio::select! {
            r = conn.accept_uni(), if accepted < expected => {
                let stream = r.map_err(|e| net(true)(e.into()))?;
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
    let (index, offset) = read_stream_header(&mut stream).await.map_err(net(true))?;
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
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| net(true)(e.into()))?;
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
