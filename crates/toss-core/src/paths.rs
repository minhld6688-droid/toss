//! Path safety: every relative path that crosses the wire (ticket `path` / `ret`,
//! manifest `rel`) goes through [`sanitize_rel`], and every filesystem location
//! derived from one goes through [`resolve_inside`] (wire.md §1 item 4).
//!
//! Rules applied on every platform (security):
//! - `/` is the only separator; `\` is rejected (it is a separator on Windows).
//! - no NUL or other control characters, no `:` (drive letters, NTFS streams);
//! - not absolute (leading `/`), no `.` / `..` segments, no segment made only of
//!   dots and spaces (Windows folds `"... "` into `..`-like names);
//! - each segment ≤ 255 bytes, whole path ≤ 4096 bytes.
//!
//! Extra rules on Windows (names Windows cannot create or that alias devices):
//! reserved device names (`CON`, `NUL`, `COM1`, …), `<>"|?*`, trailing dot/space.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const MAX_SEGMENT: usize = 255;
const MAX_TOTAL: usize = 4096;

/// A validated, normalized relative path (segments joined by `/`).
///
/// An empty `RelPath` (no segments) denotes the base directory itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct RelPath {
    segments: Vec<String>,
}

impl RelPath {
    /// The `/`-joined form, as it appears on the wire.
    pub fn as_str(&self) -> String {
        self.segments.join("/")
    }

    /// The segments.
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// True for the empty path (base directory itself).
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The path as a native relative [`PathBuf`].
    pub fn to_path(&self) -> PathBuf {
        self.segments.iter().collect()
    }

    /// The last segment, if any.
    pub fn file_name(&self) -> Option<&str> {
        self.segments.last().map(String::as_str)
    }

    /// Returns a copy with the last segment replaced.
    pub fn with_file_name(&self, name: &str) -> RelPath {
        let mut segments = self.segments.clone();
        if let Some(last) = segments.last_mut() {
            *last = name.to_string();
        } else {
            segments.push(name.to_string());
        }
        RelPath { segments }
    }

    /// Concatenates two relative paths.
    pub fn join(&self, other: &RelPath) -> RelPath {
        let mut segments = self.segments.clone();
        segments.extend(other.segments.iter().cloned());
        RelPath { segments }
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// Validates a `/`-separated relative path received from a peer or a ticket.
///
/// Empty segments (`a//b`, trailing `/`) are dropped; `""` yields an empty
/// [`RelPath`]. Everything listed in the module docs is rejected.
pub fn sanitize_rel(s: &str) -> Result<RelPath> {
    if s.len() > MAX_TOTAL {
        bail!("path too long ({} bytes)", s.len());
    }
    if s.starts_with('/') {
        bail!("absolute path not allowed: {s:?}");
    }
    let mut segments = Vec::new();
    for seg in s.split('/') {
        if seg.is_empty() {
            continue;
        }
        check_segment(seg).with_context(|| format!("invalid path {s:?}"))?;
        segments.push(seg.to_string());
    }
    Ok(RelPath { segments })
}

fn check_segment(seg: &str) -> Result<()> {
    if seg.len() > MAX_SEGMENT {
        bail!("segment longer than {MAX_SEGMENT} bytes");
    }
    if seg == "." || seg == ".." {
        bail!("'.' and '..' segments are not allowed");
    }
    if seg.chars().all(|c| c == '.' || c == ' ') {
        bail!("segment made only of dots/spaces");
    }
    for c in seg.chars() {
        if c == '\0' {
            bail!("NUL character");
        }
        if c.is_control() {
            bail!("control character");
        }
        if c == '\\' {
            bail!("backslash not allowed");
        }
        if c == ':' {
            bail!("':' not allowed");
        }
    }
    if cfg!(windows) {
        check_segment_windows(seg)?;
    }
    Ok(())
}

/// Windows-specific name rules. Public for tests on other platforms.
pub fn check_segment_windows(seg: &str) -> Result<()> {
    if seg
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        bail!("character not allowed on Windows");
    }
    if seg.ends_with('.') || seg.ends_with(' ') {
        bail!("trailing dot/space not allowed on Windows");
    }
    let stem = seg
        .split('.')
        .next()
        .unwrap_or(seg)
        .trim_end()
        .to_ascii_uppercase();
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let is_port =
        |p: &str| stem.len() == 4 && stem.starts_with(p) && stem.as_bytes()[3].is_ascii_digit();
    if RESERVED.contains(&stem.as_str()) || is_port("COM") || is_port("LPT") {
        bail!("reserved device name on Windows");
    }
    Ok(())
}

/// Turns an arbitrary human string (job name) into one safe folder name.
///
/// Invalid characters become `_`, trailing dots/spaces are trimmed, length is
/// capped at 80 chars; an unusable result yields `fallback`.
pub fn sanitize_component(name: &str, fallback: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let out = out.trim_start().to_string();
    if out.is_empty() || check_segment(&out).is_err() || check_segment_windows(&out).is_err() {
        // Reserved device names get a suffix; everything else falls back.
        if !out.is_empty() && check_segment(&out).is_ok() {
            let suffixed = format!("{out}_");
            if check_segment_windows(&suffixed).is_ok() {
                return suffixed;
            }
        }
        return fallback.to_string();
    }
    out
}

/// Canonicalizes without Windows verbatim (`\\?\`) prefixes where possible.
pub fn canonicalize(p: &Path) -> Result<PathBuf> {
    dunce::canonicalize(p).with_context(|| format!("canonicalize {}", p.display()))
}

/// Resolves `rel` (validated with [`sanitize_rel`]) under `root` and makes sure
/// the result stays inside `root` after resolving symlinks.
///
/// The target does not need to exist: the deepest existing ancestor is
/// canonicalized and must be inside the canonical root. If the target exists
/// (even as a dangling symlink) it is canonicalized itself and checked too.
pub fn resolve_inside(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = sanitize_rel(rel)?;
    resolve_rel(root, &rel)
}

/// Same as [`resolve_inside`] for an already validated [`RelPath`].
pub fn resolve_rel(root: &Path, rel: &RelPath) -> Result<PathBuf> {
    let root_c = canonicalize(root)?;
    let target = root_c.join(rel.to_path());
    check_inside(&root_c, &target)?;
    Ok(target)
}

/// Checks that `target` (absolute, under canonical `root_c`) does not escape
/// `root_c` through symlinks.
pub fn check_inside(root_c: &Path, target: &Path) -> Result<()> {
    // Walk up to the deepest existing ancestor (symlink_metadata so a dangling
    // symlink counts as existing and gets rejected by canonicalize below).
    let mut probe = target.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&probe).is_ok() {
            break;
        }
        if !probe.pop() {
            bail!("no existing ancestor for {}", target.display());
        }
    }
    let real = dunce::canonicalize(&probe)
        .with_context(|| format!("resolve {} (dangling symlink?)", probe.display()))?;
    if !real.starts_with(root_c) {
        bail!(
            "path escapes root: {} resolves to {}",
            target.display(),
            real.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        assert_eq!(sanitize_rel("").unwrap().as_str(), "");
        assert_eq!(sanitize_rel("a").unwrap().as_str(), "a");
        assert_eq!(sanitize_rel("2026/12A3").unwrap().as_str(), "2026/12A3");
        assert_eq!(sanitize_rel("a//b/").unwrap().as_str(), "a/b");
        assert_eq!(
            sanitize_rel("IMG 0001.CR3").unwrap().as_str(),
            "IMG 0001.CR3"
        );
        assert_eq!(sanitize_rel(".hidden/x").unwrap().as_str(), ".hidden/x");
        assert_eq!(
            sanitize_rel("Ảnh lớp 12A3/ảnh.jpg")
                .unwrap()
                .segments()
                .len(),
            2
        );
        assert_eq!(sanitize_rel("a..b/c...d").unwrap().as_str(), "a..b/c...d");
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        for bad in [
            "..",
            "../x",
            "a/../b",
            "a/..",
            ".",
            "./a",
            "/etc/passwd",
            "/",
            "a\\..\\b",
            "..\\x",
            "C:\\Windows",
            "C:/Windows",
            "c:",
            "a/b:stream",
            "a\0b",
            "a/\nb",
            "...",
            ". .",
            "a/ . /b",
        ] {
            assert!(sanitize_rel(bad).is_err(), "should reject {bad:?}");
        }
        let long = "a".repeat(256);
        assert!(sanitize_rel(&long).is_err());
        let total = "ab/".repeat(2000);
        assert!(sanitize_rel(&total).is_err());
    }

    #[test]
    fn windows_rules() {
        for bad in [
            "CON", "con.txt", "NUL", "Com1", "lpt9.log", "a?", "x*", "a.", "a ",
        ] {
            assert!(check_segment_windows(bad).is_err(), "should reject {bad:?}");
        }
        for ok in ["CONSOLE", "COM10", "a.b", "nul_", "file"] {
            assert!(check_segment_windows(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn component() {
        assert_eq!(sanitize_component("12A3 RAW", "x"), "12A3 RAW");
        assert_eq!(sanitize_component("a/b\\c:d", "x"), "a_b_c_d");
        assert_eq!(sanitize_component("..", "fb"), "fb");
        assert_eq!(sanitize_component("  name. ", "fb"), "name");
        assert_eq!(sanitize_component("", "fb"), "fb");
        assert_eq!(sanitize_component("CON", "fb"), "CON_");
        assert_eq!(sanitize_component(&"x".repeat(200), "fb").len(), 80);
    }

    #[test]
    fn resolve_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/f.txt"), b"x").unwrap();
        let r = resolve_inside(&root, "sub/f.txt").unwrap();
        assert!(r.ends_with("sub/f.txt"));
        // Non-existing tail is fine.
        assert!(resolve_inside(&root, "new/dir/file").is_ok());
        assert!(resolve_inside(&root, "").is_ok());
        assert!(resolve_inside(&root, "../root/sub").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"s").unwrap();
        symlink(&outside, root.join("link")).unwrap();
        symlink(outside.join("secret"), root.join("filelink")).unwrap();
        symlink(dir.path().join("nowhere"), root.join("dangling")).unwrap();
        // Inside symlink is OK.
        std::fs::create_dir_all(root.join("real")).unwrap();
        symlink(root.join("real"), root.join("inlink")).unwrap();

        assert!(resolve_inside(&root, "link").is_err());
        assert!(resolve_inside(&root, "link/secret").is_err());
        assert!(resolve_inside(&root, "link/new/file").is_err());
        assert!(resolve_inside(&root, "filelink").is_err());
        assert!(resolve_inside(&root, "dangling").is_err());
        assert!(resolve_inside(&root, "dangling/x").is_err());
        assert!(resolve_inside(&root, "inlink/x").is_ok());
    }
}
