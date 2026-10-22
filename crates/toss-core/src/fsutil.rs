//! Filesystem helpers: walking, hashing (with a size/mtime keyed cache),
//! `.part` naming and no-overwrite renames.

use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use parking_lot::Mutex;

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

/// Cache of file hashes keyed by path, validated by size + mtime.
#[derive(Debug, Default)]
pub struct HashCache {
    map: Mutex<HashMap<PathBuf, CachedHash>>,
}

impl HashCache {
    /// Hash of `path`, from cache when size and mtime are unchanged.
    pub fn hash(&self, path: &Path) -> Result<[u8; 32]> {
        let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some((s, m, h)) = self.map.lock().get(path) {
            if *s == meta.len() && *m == mtime {
                return Ok(*h);
            }
        }
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
