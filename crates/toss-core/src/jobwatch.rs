//! Studio auto-links: finds shooter folders under a jobs root and reports the
//! ones whose copy has finished, so the app makes one link per folder.
//!
//! Layout (any depth up to [`MAX_DEPTH`]): `<root>/…/<job>/1_RAW/<shooter>/…files`,
//! e.g. `D:\Jobs\2026\11\03.11.26 12A1 Doan Ket HBT\1_RAW\Tuan Future`.
//!
//! Cheap by design so it can run every half minute on a big archive on a HDD:
//! discovery lists folder names only, never the inside of a job except `1_RAW`;
//! a shooter folder is walked file by file only when it (or its `1_RAW`) changed
//! after `since`, i.e. old jobs are never read again.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};

/// Name of the folder holding one sub-folder per shooter.
pub const SHOOTER_PARENT: &str = "1_RAW";
/// Deepest level (below the root) where a job folder is looked for.
pub const MAX_DEPTH: usize = 5;

/// A shooter folder and what is in it right now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Path under the root, `/`-separated (`2026/11/<job>/1_RAW/<shooter>`).
    pub rel: String,
    /// Job folder name (the parent of `1_RAW`).
    pub job: String,
    /// Shooter folder name.
    pub shooter: String,
    /// Files inside (any depth).
    pub files: u64,
    /// Their total size.
    pub bytes: u64,
}

fn newest(m: &std::fs::Metadata) -> SystemTime {
    let a = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let b = m.created().unwrap_or(SystemTime::UNIX_EPOCH);
    a.max(b)
}

fn skip_name(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('$') || name.eq_ignore_ascii_case("System Volume Information")
}

/// Shooter folders under `root` that changed after `since`, with their file
/// count and size (hidden files, `.part` files and symlinks not counted).
pub fn scan(root: &Path, since: SystemTime) -> Vec<Found> {
    let mut jobs = Vec::new();
    find_jobs(root, String::new(), 0, &mut jobs);
    let mut out = Vec::new();
    for (job_rel, job_name) in jobs {
        let raw = root.join(&job_rel).join(SHOOTER_PARENT);
        let raw_new = std::fs::metadata(&raw).map(|m| newest(&m) >= since).unwrap_or(false);
        let Ok(rd) = std::fs::read_dir(&raw) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if skip_name(&name) || !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let fresh = raw_new || e.metadata().map(|m| newest(&m) >= since).unwrap_or(false);
            if !fresh {
                continue;
            }
            let (files, bytes) = count(&e.path());
            let rel = if job_rel.is_empty() {
                format!("{SHOOTER_PARENT}/{name}")
            } else {
                format!("{job_rel}/{SHOOTER_PARENT}/{name}")
            };
            out.push(Found { rel, job: job_name.clone(), shooter: name, files, bytes });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Folders holding a `1_RAW` folder, as (path under root, folder name).
fn find_jobs(dir: &Path, rel: String, depth: usize, out: &mut Vec<(String, String)>) {
    if dir.join(SHOOTER_PARENT).is_dir() {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push((rel, name));
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if skip_name(&name) || name.eq_ignore_ascii_case(SHOOTER_PARENT) || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        find_jobs(&e.path(), r, depth + 1, out);
    }
}

/// Files and bytes under `dir`, counted the way [`scan`] counts them.
pub fn measure(dir: &Path) -> (u64, u64) {
    count(dir)
}

fn count(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if crate::fsutil::is_hidden(&name, &m) || m.file_type().is_symlink() {
                continue;
            }
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() && !name.ends_with(crate::fsutil::PART_SUFFIX) {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (files, bytes)
}

/// Decides when a folder's copy is finished: its file count and size stayed the
/// same for `quiet`. Reports a folder again only when it settles at new numbers
/// (a shooter's second card), so the caller can refresh what it shows.
pub struct Settle {
    quiet: Duration,
    last: HashMap<String, ((u64, u64), Instant)>,
    reported: HashMap<String, (u64, u64)>,
}

impl Settle {
    /// A folder counts as finished after `quiet` without change.
    pub fn new(quiet: Duration) -> Self {
        Settle { quiet, last: HashMap::new(), reported: HashMap::new() }
    }

    /// Feeds one scan; returns the folders that just settled.
    pub fn feed(&mut self, found: &[Found], now: Instant) -> Vec<Found> {
        let mut ready = Vec::new();
        self.last.retain(|rel, _| found.iter().any(|f| &f.rel == rel));
        for f in found {
            let key = (f.files, f.bytes);
            match self.last.get(&f.rel) {
                Some((k, since)) if *k == key => {
                    if f.files > 0
                        && now.duration_since(*since) >= self.quiet
                        && self.reported.get(&f.rel) != Some(&key)
                    {
                        self.reported.insert(f.rel.clone(), key);
                        ready.push(f.clone());
                    }
                }
                _ => {
                    self.last.insert(f.rel.clone(), (key, now));
                }
            }
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn finds_shooter_folders_at_any_depth_and_counts_files() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let job = r.join("2026/11/03.11.26 12A1 Doan Ket HBT");
        put(&job.join("1_RAW/Tuan Future/100NCZ_7/DSC_0001.NEF"), b"12345");
        put(&job.join("1_RAW/Tuan Future/DSC_0002.NEF"), b"123");
        put(&job.join("1_RAW/Tuan Future/.DS_Store"), b"x");
        put(&job.join("1_RAW/Tuan Future/DSC_0003.NEF.part"), b"xx");
        put(&job.join("1_RAW/Minhfatz/MFZ_1.NEF"), b"1");
        std::fs::create_dir_all(job.join("1_RAW/Empty")).unwrap();
        put(&job.join("Pick/should_not_count.jpg"), b"zzz");
        put(&r.join("Loose/1_RAW/Huu Duc CAV/a.nef"), b"ab");

        let found = scan(r, SystemTime::UNIX_EPOCH);
        let rels: Vec<_> = found.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(
            rels,
            [
                "2026/11/03.11.26 12A1 Doan Ket HBT/1_RAW/Empty",
                "2026/11/03.11.26 12A1 Doan Ket HBT/1_RAW/Minhfatz",
                "2026/11/03.11.26 12A1 Doan Ket HBT/1_RAW/Tuan Future",
                "Loose/1_RAW/Huu Duc CAV",
            ]
        );
        let tf = &found[2];
        assert_eq!((tf.job.as_str(), tf.shooter.as_str(), tf.files, tf.bytes), ("03.11.26 12A1 Doan Ket HBT", "Tuan Future", 2, 8));
    }

    #[test]
    fn old_folders_are_not_read_again() {
        let t = tempfile::tempdir().unwrap();
        put(&t.path().join("Job/1_RAW/A/a.nef"), b"a");
        let later = SystemTime::now() + Duration::from_secs(3600);
        assert!(scan(t.path(), later).is_empty());
    }

    #[test]
    fn settles_after_quiet_and_again_on_new_numbers() {
        let f = |files, bytes| Found { rel: "J/1_RAW/A".into(), job: "J".into(), shooter: "A".into(), files, bytes };
        let mut s = Settle::new(Duration::from_secs(120));
        let t0 = Instant::now();
        assert!(s.feed(&[f(10, 100)], t0).is_empty());
        assert!(s.feed(&[f(20, 200)], t0 + Duration::from_secs(30)).is_empty(), "still copying");
        assert!(s.feed(&[f(20, 200)], t0 + Duration::from_secs(100)).is_empty(), "not quiet long enough");
        assert_eq!(s.feed(&[f(20, 200)], t0 + Duration::from_secs(151)).len(), 1);
        assert!(s.feed(&[f(20, 200)], t0 + Duration::from_secs(300)).is_empty(), "reported once");
        assert!(s.feed(&[f(30, 300)], t0 + Duration::from_secs(330)).is_empty(), "second card copying");
        assert_eq!(s.feed(&[f(30, 300)], t0 + Duration::from_secs(460)).len(), 1, "settles at the new size");
        let empty = Found { files: 0, bytes: 0, ..f(0, 0) };
        let mut s = Settle::new(Duration::ZERO);
        assert!(s.feed(std::slice::from_ref(&empty), t0).is_empty());
        assert!(s.feed(&[empty], t0 + Duration::from_secs(1)).is_empty(), "empty folders never get a link");
    }
}
