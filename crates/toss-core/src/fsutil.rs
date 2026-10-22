//! Filesystem helpers: walking, hashing (with a size/mtime keyed cache),
//! `.part` naming and no-overwrite renames.

use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::SystemTime,
};

use anyhow::{Context, Result};
use parking_lot::{Condvar, Mutex};

use crate::paths::{sanitize_rel, RelPath};

/// Suffix of partially received files.
pub const PART_SUFFIX: &str = ".part";
const BUF: usize = 1 << 20;

/// One file found by [`walk`].
#[derive(Clone, Debug)]
pub struct WalkedFile {
    /// Path relative to the walk base (or file name for a single file).
    pub rel: RelPath,
    /// Absolute path.
    pub abs: PathBuf,
    /// Size.
    pub size: u64,
}

/// Hidden = name starts with `.` or (Windows) hidden/system attribute.
pub fn is_hidden(name: &str, meta: &Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if meta.file_attributes() & (HIDDEN | SYSTEM) != 0 {
            return true;
        }
    }
    let _ = meta;
    false
}

/// Should this directory entry be skipped when building a manifest?
fn skip_entry(name: &str, meta: &Metadata) -> bool {
    meta.file_type().is_symlink() || is_hidden(name, meta) || name.ends_with(PART_SUFFIX)
}

/// Walks `base` recursively (sorted, deterministic). Hidden files, `.part`
/// files and symlinks are skipped. Names that fail [`sanitize_rel`] are
/// skipped with a warning. If `base` is a file, returns just that file with
/// `prefix/<file name>` as rel.
pub fn walk(base: &Path, prefix: &RelPath) -> Result<Vec<WalkedFile>> {
    let meta = std::fs::metadata(base).with_context(|| format!("stat {}", base.display()))?;
    let mut out = Vec::new();
    if meta.is_file() {
        out.push(WalkedFile {
            rel: prefix.clone(),
            abs: base.to_path_buf(),
            size: meta.len(),
        });
        return Ok(out);
    }
    walk_dir(base, prefix, &mut out)?;
    Ok(out)
}

fn walk_dir(dir: &Path, prefix: &RelPath, out: &mut Vec<WalkedFile>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = match e.file_name().into_string() {
            Ok(n) => n,
            Err(n) => {
                tracing::warn!("skipping non-UTF-8 name {:?} in {}", n, dir.display());
                continue;
            }
        };
        let meta = match e.path().symlink_metadata() {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!("skipping {}: {err}", e.path().display());
                continue;
            }
        };
        if skip_entry(&name, &meta) {
            continue;
        }
        let rel = match sanitize_rel(&name) {
            Ok(r) => prefix.join(&r),
            Err(err) => {
                tracing::warn!("skipping {}: {err:#}", e.path().display());
                continue;
            }
        };
        if meta.is_dir() {
            walk_dir(&e.path(), &rel, out)?;
        } else if meta.is_file() {
            out.push(WalkedFile {
                rel,
                abs: e.path(),
                size: meta.len(),
            });
        }
    }
    Ok(())
}

/// `<path>.part`.
pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_os_string();
    s.push(PART_SUFFIX);
    PathBuf::from(s)
}

/// blake3 of a whole file.
pub fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF];
    loop {
        let n = f
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(*h.finalize().as_bytes())
}

/// Hasher pre-fed with the first `len` bytes of `path` (for resumed `.part` files).
pub fn hash_prefix(path: &Path, len: u64) -> Result<blake3::Hasher> {
    let mut h = blake3::Hasher::new();
    if len == 0 {
        return Ok(h);
    }
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    f.seek(SeekFrom::Start(0))?;
    let mut left = len;
    let mut buf = vec![0u8; BUF];
    while left > 0 {
        let want = usize::try_from(left.min(BUF as u64)).unwrap_or(BUF);
        let n = f
            .read(&mut buf[..want])
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            anyhow::bail!("{} shorter than expected", path.display());
        }
        h.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(h)
}

/// (size, mtime, blake3) of a hashed file.
type CachedHash = (u64, SystemTime, [u8; 32]);

/// Files hashed at once when building a manifest or prehashing a job.
pub const HASH_THREADS: usize = 4;

/// Signals the end of one in-flight hash to threads waiting for the same file.
#[derive(Debug, Default)]
struct InFlight {
    done: Mutex<bool>,
    cv: Condvar,
}

/// Cache of file hashes keyed by path, validated by size + mtime. Two threads
/// asking for the same uncached file (a prehash and a manifest build) hash it
/// once: the second waits for the first.
#[derive(Debug, Default)]
pub struct HashCache {
    map: Mutex<HashMap<PathBuf, CachedHash>>,
    inflight: Mutex<HashMap<PathBuf, Arc<InFlight>>>,
}

/// Removes an in-flight marker (also on error/panic) and wakes waiters.
struct InFlightGuard<'a> {
    cache: &'a HashCache,
    path: &'a Path,
    entry: Arc<InFlight>,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.cache.inflight.lock().remove(self.path);
        *self.entry.done.lock() = true;
        self.entry.cv.notify_all();
    }
}

impl HashCache {
    fn cached(&self, path: &Path, meta: &Metadata) -> Option<[u8; 32]> {
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        match self.map.lock().get(path) {
            Some((s, m, h)) if *s == meta.len() && *m == mtime => Some(*h),
            _ => None,
        }
    }

    /// Hash of `path`, from cache when size and mtime are unchanged.
    pub fn hash(&self, path: &Path) -> Result<[u8; 32]> {
        loop {
            let meta =
                std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
            if let Some(h) = self.cached(path, &meta) {
                return Ok(h);
            }
            let other = {
                let mut inflight = self.inflight.lock();
                match inflight.get(path) {
                    Some(e) => e.clone(),
                    None => {
                        let e = Arc::new(InFlight::default());
                        inflight.insert(path.to_path_buf(), e.clone());
                        drop(inflight);
                        let _guard = InFlightGuard {
                            cache: self,
                            path,
                            entry: e,
                        };
                        return self.hash_now(path, &meta);
                    }
                }
            };
            let mut done = other.done.lock();
            while !*done {
                other.cv.wait(&mut done);
            }
            // Re-check the cache; if the other thread could not cache it (file
            // changed, error), the next round hashes it here.
        }
    }

    fn hash_now(&self, path: &Path, meta: &Metadata) -> Result<[u8; 32]> {
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let h = hash_file(path)?;
        // Re-stat: only cache if the file did not change while hashing.
        if let Ok(meta2) = std::fs::metadata(path) {
            if meta2.len() == meta.len() && meta2.modified().ok() == Some(mtime) {
                self.map
                    .lock()
                    .insert(path.to_path_buf(), (meta.len(), mtime, h));
            }
        }
        Ok(h)
    }

    /// Hashes `paths` with up to `threads` files at once; results in input order.
    pub fn hash_many(&self, paths: &[PathBuf], threads: usize) -> Vec<Result<[u8; 32]>> {
        let threads = threads.clamp(1, paths.len().max(1));
        if threads == 1 {
            return paths.iter().map(|p| self.hash(p)).collect();
        }
        let next = AtomicUsize::new(0);
        let out: Mutex<Vec<Option<Result<[u8; 32]>>>> =
            Mutex::new((0..paths.len()).map(|_| None).collect());
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(p) = paths.get(i) else { break };
                    let r = self.hash(p);
                    out.lock()[i] = Some(r);
                });
            }
        });
        out.into_inner()
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(anyhow::anyhow!("hash worker failed"))))
            .collect()
    }

    /// Records a freshly verified file.
    pub fn insert(&self, path: &Path, hash: [u8; 32]) {
        if let Ok(meta) = std::fs::metadata(path) {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            self.map
                .lock()
                .insert(path.to_path_buf(), (meta.len(), mtime, hash));
        }
    }
}

/// Folders being prehashed right now (one prehash per folder at a time).
static PREHASHING: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> =
    std::sync::LazyLock::new(Default::default);

/// Hashes every file under `src` into `cache` on a background thread
/// ([`HASH_THREADS`] files at once), so that the manifest of a get is ready
/// (nearly) at once when the client connects. Skipped while `src` is already
/// being prehashed. Returns the thread handle (`None` when skipped).
pub fn spawn_prehash(cache: Arc<HashCache>, src: PathBuf) -> Option<std::thread::JoinHandle<()>> {
    if !PREHASHING.lock().insert(src.clone()) {
        return None;
    }
    struct Unmark(PathBuf);
    impl Drop for Unmark {
        fn drop(&mut self) {
            PREHASHING.lock().remove(&self.0);
        }
    }
    let unmark = Unmark(src.clone());
    let spawned = std::thread::Builder::new()
        .name("toss-prehash".into())
        .spawn(move || {
            let _unmark = unmark;
            let started = std::time::Instant::now();
            let files = match walk(&src, &RelPath::default()) {
                Ok(f) => f,
                Err(e) => {
                    tracing::info!("prehash {}: {e:#}", src.display());
                    return;
                }
            };
            let bytes: u64 = files.iter().map(|f| f.size).sum();
            let paths: Vec<PathBuf> = files.into_iter().map(|f| f.abs).collect();
            let failed = cache
                .hash_many(&paths, HASH_THREADS)
                .iter()
                .filter(|r| r.is_err())
                .count();
            tracing::info!(
                files = paths.len(),
                bytes,
                failed,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "prehashed {}",
                src.display()
            );
        });
    match spawned {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("prehash thread: {e}");
            None
        }
    }
}

/// `"a.jpg"` → `"a (n).jpg"`; `"name"` / `".hidden"` → `"name (n)"`.
pub fn numbered_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(pos) if pos > 0 => format!("{} ({n}){}", &name[..pos], &name[pos..]),
        _ => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered() {
        assert_eq!(numbered_name("a.jpg", 2), "a (2).jpg");
        assert_eq!(numbered_name("a.tar.gz", 3), "a.tar (3).gz");
        assert_eq!(numbered_name("name", 2), "name (2)");
        assert_eq!(numbered_name(".hidden", 2), ".hidden (2)");
    }

    #[test]
    fn hash_many_and_prehash() {
        let d = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..9u8 {
            let p = d.path().join(format!("f{i}"));
            std::fs::write(&p, vec![i; 1000 + i as usize]).unwrap();
            paths.push(p);
        }
        let cache = HashCache::default();
        let hs = cache.hash_many(&paths, HASH_THREADS);
        for (p, h) in paths.iter().zip(hs) {
            let want = *blake3::hash(&std::fs::read(p).unwrap()).as_bytes();
            assert_eq!(h.unwrap(), want);
        }
        // Concurrent callers for the same file agree (and hash it once).
        let dup = vec![paths[0].clone(); 8];
        let fresh = HashCache::default();
        let hs = fresh.hash_many(&dup, 4);
        assert!(hs
            .iter()
            .all(|h| h.as_ref().unwrap() == hs[0].as_ref().unwrap()));
        assert!(fresh.inflight.lock().is_empty());
        let cache2 = Arc::new(HashCache::default());
        spawn_prehash(cache2.clone(), d.path().to_path_buf())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(cache2.map.lock().len(), 9);
    }

    #[test]
    fn walk_skips() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path();
        std::fs::create_dir_all(b.join("x/y")).unwrap();
        std::fs::write(b.join("x/y/f"), b"1").unwrap();
        std::fs::write(b.join("a"), b"22").unwrap();
        std::fs::write(b.join(".h"), b"").unwrap();
        std::fs::write(b.join("p.part"), b"").unwrap();
        std::fs::create_dir_all(b.join(".git")).unwrap();
        std::fs::write(b.join(".git/c"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(b.join("a"), b.join("ln")).unwrap();
        let w = walk(b, &RelPath::default()).unwrap();
        let rels: Vec<_> = w.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, vec!["a", "x/y/f"]);
        let one = walk(&b.join("a"), &sanitize_rel("a").unwrap()).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].size, 2);
    }
}
