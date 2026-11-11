//! Backup receipts.
//!
//! A put ends with `Done { ok: true }` only after the receiving host has checked
//! every file's blake3 and synced it to disk (or already held an identical copy).
//! At that moment the sender writes a receipt: which local files went out, with
//! the size, modification time and hash they had. Later, [`free`] deletes exactly
//! those files, and only the ones still unchanged since the receipt, so an editor
//! can clear their SSD knowing the studio holds the same bytes.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::now_unix;

const DIR: &str = "receipts";
/// Junk the OS drops into folders; removed so emptied folders can go too.
const JUNK: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

/// One local file the receiver confirmed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReceiptFile {
    /// Absolute local path.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Modification time when the receipt was written: seconds, nanoseconds.
    pub mtime: (u64, u32),
    /// blake3, hex.
    pub hash: String,
}

/// What [`free`] did.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Freed {
    /// When (unix seconds).
    pub at: u64,
    /// Files deleted.
    pub files: u32,
    /// Bytes freed.
    pub bytes: u64,
    /// Files left in place because they changed (or vanished) after the backup.
    pub kept: Vec<String>,
}

/// A confirmed backup.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Receipt {
    /// Transfer id.
    pub id: String,
    /// Job name of the transfer.
    pub job: String,
    /// Receiving machine, as the transfer names it.
    pub to: String,
    /// When the receiver confirmed (unix seconds).
    pub at: u64,
    /// What the user picked (folders or files).
    pub roots: Vec<PathBuf>,
    /// Every file sent.
    pub files: Vec<ReceiptFile>,
    /// Total bytes sent.
    pub bytes: u64,
    /// Set once the local copies were freed.
    #[serde(default)]
    pub freed: Option<Freed>,
}

/// A receipt without its file list (for lists in the UI).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReceiptSummary {
    /// Transfer id.
    pub id: String,
    /// Job name.
    pub job: String,
    /// Receiving machine.
    pub to: String,
    /// When the receiver confirmed.
    pub at: u64,
    /// Files confirmed.
    pub files: u32,
    /// Bytes confirmed.
    pub bytes: u64,
    /// Set once freed.
    pub freed: Option<Freed>,
}

impl Receipt {
    /// The receipt without its file list.
    pub fn summary(&self) -> ReceiptSummary {
        ReceiptSummary {
            id: self.id.clone(),
            job: self.job.clone(),
            to: self.to.clone(),
            at: self.at,
            files: self.files.len() as u32,
            bytes: self.bytes,
            freed: self.freed.clone(),
        }
    }
}

pub(crate) fn mtime_of(m: &std::fs::Metadata) -> (u64, u32) {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or((0, 0), |d| (d.as_secs(), d.subsec_nanos()))
}

fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// `<id>.json` holds the summary (small, read by lists), `<id>.files.json` the file list
/// (thousands of entries, read only to free).
fn file(data_dir: &Path, id: &str, ext: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("bad receipt id");
    }
    Ok(dir(data_dir).join(format!("{id}.{ext}")))
}

#[derive(Serialize, Deserialize)]
struct Head {
    #[serde(flatten)]
    summary: ReceiptSummary,
    roots: Vec<PathBuf>,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).with_context(|| format!("save {}", path.display()))?;
    Ok(())
}

/// Writes (or replaces) a receipt atomically: the file list first, then the summary that
/// makes it visible.
pub fn save(data_dir: &Path, r: &Receipt) -> Result<()> {
    std::fs::create_dir_all(dir(data_dir))?;
    write_atomic(&file(data_dir, &r.id, "files.json")?, &serde_json::to_vec(&r.files)?)?;
    let head = Head { summary: r.summary(), roots: r.roots.clone() };
    write_atomic(&file(data_dir, &r.id, "json")?, &serde_json::to_vec_pretty(&head)?)
}

/// The summary of one receipt, if there is one (cheap: never reads the file list).
pub fn summary(data_dir: &Path, id: &str) -> Option<ReceiptSummary> {
    let bytes = std::fs::read(file(data_dir, id, "json").ok()?).ok()?;
    serde_json::from_slice::<Head>(&bytes).ok().map(|h| h.summary)
}

/// Reads one receipt, file list included.
pub fn load(data_dir: &Path, id: &str) -> Result<Receipt> {
    let head = std::fs::read(file(data_dir, id, "json")?).with_context(|| format!("Không có biên nhận {id}"))?;
    let head: Head = serde_json::from_slice(&head)?;
    let files = std::fs::read(file(data_dir, id, "files.json")?).with_context(|| format!("Biên nhận {id} thiếu danh sách file"))?;
    let s = head.summary;
    Ok(Receipt {
        id: s.id,
        job: s.job,
        to: s.to,
        at: s.at,
        roots: head.roots,
        files: serde_json::from_slice(&files)?,
        bytes: s.bytes,
        freed: s.freed,
    })
}

/// Every receipt, newest first (summaries only).
pub fn list(data_dir: &Path) -> Vec<ReceiptSummary> {
    let mut out: Vec<ReceiptSummary> = std::fs::read_dir(dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".json") && !n.ends_with(".files.json")
        })
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Head>(&b).ok())
        .map(|h| h.summary)
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.at));
    out
}

/// Deletes the receipt's files that are still exactly as they were sent, then
/// the folders this left empty. Idempotent: a receipt freed before returns the
/// earlier result. Never follows symlinks, never touches anything outside the
/// receipt's roots.
pub fn free(data_dir: &Path, id: &str) -> Result<Freed> {
    let mut r = load(data_dir, id)?;
    if let Some(f) = &r.freed {
        return Ok(f.clone());
    }
    let mut done = Freed { at: now_unix(), ..Default::default() };
    for f in &r.files {
        if !r.roots.iter().any(|root| f.path.starts_with(root)) {
            done.kept.push(f.path.display().to_string());
            continue;
        }
        let Ok(m) = std::fs::symlink_metadata(&f.path) else {
            continue; // already gone
        };
        if !m.is_file() || m.len() != f.size || mtime_of(&m) != f.mtime {
            done.kept.push(f.path.display().to_string());
            continue;
        }
        match std::fs::remove_file(&f.path) {
            Ok(()) => {
                done.files += 1;
                done.bytes += f.size;
            }
            Err(e) => {
                tracing::warn!("free {}: {e}", f.path.display());
                done.kept.push(f.path.display().to_string());
            }
        }
    }
    for root in &r.roots {
        remove_empty_dirs(root);
    }
    r.freed = Some(done.clone());
    save(data_dir, &r)?;
    Ok(done)
}

/// Removes `root` and the folders under it that hold nothing but OS junk.
fn remove_empty_dirs(root: &Path) {
    let Ok(m) = std::fs::symlink_metadata(root) else { return };
    if !m.is_dir() {
        return;
    }
    let mut dirs = vec![root.to_path_buf()];
    let mut i = 0;
    while i < dirs.len() {
        if let Ok(rd) = std::fs::read_dir(&dirs[i]) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    dirs.push(e.path());
                }
            }
        }
        i += 1;
    }
    // deepest first
    for d in dirs.iter().rev() {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let entries: Vec<_> = rd.flatten().collect();
        let only_junk = entries.iter().all(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.file_type().is_ok_and(|t| t.is_file()) && (JUNK.contains(&name.as_ref()) || name.starts_with("._"))
        });
        if only_junk {
            for e in &entries {
                let _ = std::fs::remove_file(e.path());
            }
            let _ = std::fs::remove_dir(d);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn receipt_for(data: &Path, root: &Path, id: &str) -> Receipt {
        let mut files = Vec::new();
        let mut bytes = 0;
        for e in crate::fsutil::walk(root, &crate::paths::RelPath::default()).unwrap() {
            let m = std::fs::metadata(&e.abs).unwrap();
            bytes += m.len();
            files.push(ReceiptFile {
                path: e.abs.clone(),
                size: m.len(),
                mtime: mtime_of(&m),
                hash: hex::encode(crate::fsutil::hash_file(&e.abs).unwrap()),
            });
        }
        let r = Receipt {
            id: id.into(),
            job: "Job".into(),
            to: "Studio".into(),
            at: now_unix(),
            roots: vec![root.to_path_buf()],
            files,
            bytes,
            freed: None,
        };
        save(data, &r).unwrap();
        r
    }

    #[test]
    fn frees_unchanged_files_and_keeps_changed_ones() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let root = t.path().join("12A1");
        std::fs::create_dir_all(root.join("RAW/sub")).unwrap();
        std::fs::create_dir_all(root.join("JPG")).unwrap();
        std::fs::write(root.join("RAW/a.nef"), b"aaaa").unwrap();
        std::fs::write(root.join("RAW/sub/b.nef"), b"bbbb").unwrap();
        std::fs::write(root.join("JPG/c.jpg"), b"cc").unwrap();
        receipt_for(&data, &root, "r1");

        // after the backup: one file edited, a new one added, Finder junk appears
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("JPG/c.jpg"), b"edited").unwrap();
        std::fs::write(root.join("RAW/new.nef"), b"new").unwrap();
        std::fs::write(root.join("RAW/sub/.DS_Store"), b"x").unwrap();

        let f = free(&data, "r1").unwrap();
        assert_eq!(f.files, 2);
        assert_eq!(f.bytes, 8);
        assert_eq!(f.kept.len(), 1, "edited file kept: {:?}", f.kept);
        assert!(!root.join("RAW/a.nef").exists());
        assert!(!root.join("RAW/sub").exists(), "folder with only junk left is removed");
        assert!(root.join("RAW/new.nef").exists(), "files not in the receipt are never touched");
        assert!(root.join("JPG/c.jpg").exists());

        // idempotent, and listed as freed
        assert_eq!(free(&data, "r1").unwrap(), f);
        assert_eq!(list(&data)[0].freed.as_ref().unwrap().files, 2);
    }

    #[test]
    fn frees_whole_tree_when_nothing_changed() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let root = t.path().join("Job");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/x.nef"), b"x").unwrap();
        receipt_for(&data, &root, "r2");
        let f = free(&data, "r2").unwrap();
        assert_eq!((f.files, f.kept.len()), (1, 0));
        assert!(!root.exists(), "the emptied job folder goes too");
    }

    #[test]
    fn lists_read_only_the_small_summary() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let root = t.path().join("Job");
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..5 {
            std::fs::write(root.join(format!("{i}.nef")), [i as u8; 10]).unwrap();
        }
        receipt_for(&data, &root, "r3");
        assert!(data.join("receipts/r3.files.json").is_file());
        // a broken file list does not hide the receipt from lists, only from freeing
        std::fs::write(data.join("receipts/r3.files.json"), b"not json").unwrap();
        let s = summary(&data, "r3").unwrap();
        assert_eq!((s.files, s.bytes), (5, 50));
        assert_eq!(list(&data).len(), 1);
        assert!(free(&data, "r3").is_err());
        assert!(summary(&data, "nope").is_none());
    }

    #[test]
    fn rejects_bad_ids() {
        let t = tempfile::tempdir().unwrap();
        assert!(load(t.path(), "../x").is_err());
        assert!(free(t.path(), "nope").is_err());
    }
}
