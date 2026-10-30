//! File collection (sending) and safe save paths (receiving).

use localsend::util::filename::{self, Options, Rules};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One file to send: protocol-v2 file name (`/`-separated, folder root
/// included), local path and size.
#[derive(Clone, Debug)]
pub(crate) struct OutFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Expands files and folders into the files to send. Folders keep their name
/// as the top directory; empty directories carry nothing and are skipped;
/// symlinked directories are not followed (no loops).
pub(crate) fn collect(items: &[PathBuf]) -> Vec<OutFile> {
    let mut out = Vec::new();
    for item in items {
        let Ok(meta) = std::fs::metadata(item) else {
            continue;
        };
        if meta.is_file() {
            let name = item
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            out.push(OutFile {
                name,
                path: item.clone(),
                size: meta.len(),
            });
        } else if meta.is_dir() {
            let root = item
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .or_else(|| {
                    item.canonicalize()
                        .ok()?
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "folder".into());
            walk(item, &root, &mut out, 0);
        }
    }
    out
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<OutFile>, depth: u32) {
    if depth > 64 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, &name, out, depth + 1);
        } else if ft.is_file() || ft.is_symlink() {
            // A symlink is sent when it points at a regular file.
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => out.push(OutFile {
                    name,
                    path,
                    size: m.len(),
                }),
                _ => {}
            }
        }
    }
}

/// The rules of the filesystem files are written to (this machine). `Universal` would also
/// apply Windows' reserved device names on macOS/Linux, turning very common Vietnamese
/// folder names ("con", "aux"…) into "_".
const RULES: Rules = Rules::current();

/// Windows device names (`CON`, `NUL`, `COM1`…), with or without an extension.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// Sanitizes one path component with LocalSend's filename rules. On Windows a reserved
/// device name keeps its letters ("con" → "con_", "aux.txt" → "aux_.txt") instead of
/// LocalSend's bare "_", like Toss's own `paths::sanitize_component`.
pub(crate) fn sanitize_component(s: &str, placeholder: &str) -> String {
    let fixed;
    let s = if RULES == Rules::Windows && is_reserved_device_name(s) {
        let cut = s.find('.').unwrap_or(s.len());
        fixed = format!("{}_{}", &s[..cut], &s[cut..]);
        fixed.as_str()
    } else {
        s
    };
    filename::sanitize_with(
        s,
        RULES,
        &Options {
            replacement: "_",
            placeholder,
        },
    )
}

/// Turns a peer-supplied relative path into sanitized components.
/// `None` for absolute paths, drive letters, `..` anywhere, or no usable name.
pub(crate) fn safe_rel(name: &str) -> Option<Vec<String>> {
    if name.is_empty() || name.len() > 4096 || name.starts_with('/') || name.starts_with('\\') {
        return None;
    }
    let b = name.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return None;
    }
    let mut out = Vec::new();
    for c in name.split(['/', '\\']) {
        match c {
            "" | "." => continue,
            ".." => return None,
            c => out.push(sanitize_component(c, "untitled")),
        }
    }
    if out.is_empty() || out.len() > 64 {
        return None;
    }
    Some(out)
}

/// Whether anything (file, dir, even a dangling symlink) occupies `p`.
fn taken(p: &Path, reserved: &HashSet<PathBuf>) -> bool {
    reserved.contains(p) || std::fs::symlink_metadata(p).is_ok()
}

/// `p`, or `stem (n).ext` for the first free n ≥ 2.
pub(crate) fn unique(p: PathBuf, reserved: &HashSet<PathBuf>, is_dir: bool) -> PathBuf {
    if !taken(&p, reserved) {
        return p;
    }
    let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let fname = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = if is_dir {
        (fname.clone(), String::new())
    } else {
        match fname.rfind('.') {
            Some(i) if i > 0 => (fname[..i].to_string(), fname[i..].to_string()),
            _ => (fname.clone(), String::new()),
        }
    };
    for n in 2..100_000u32 {
        let cand = parent.join(format!("{stem} ({n}){ext}"));
        if !taken(&cand, reserved) {
            return cand;
        }
    }
    parent.join(format!("{stem} ({}){ext}", uuid::Uuid::new_v4()))
}

/// Remote string → one display line (control chars removed).
pub(crate) fn single_line(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' | '\n' | '\r' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Remote multi-line text: keeps line breaks/tabs, drops other control chars.
pub(crate) fn multi_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        for bad in [
            "../x",
            "a/../b",
            "/etc/passwd",
            "\\\\srv\\x",
            "C:\\x",
            "c:x",
            "..",
            "",
            "a\\..\\b",
        ] {
            assert!(safe_rel(bad).is_none(), "{bad}");
        }
        assert_eq!(safe_rel("a/b c/d.txt").unwrap(), vec!["a", "b c", "d.txt"]);
        assert_eq!(safe_rel("a\\b").unwrap(), vec!["a", "b"]);
        assert_eq!(safe_rel("./a//b").unwrap(), vec!["a", "b"]);
        if cfg!(windows) {
            assert_eq!(safe_rel("x/con.txt").unwrap(), vec!["x", "con_.txt"]);
            assert_eq!(safe_rel("q?.txt").unwrap(), vec!["q_.txt"]);
        } else {
            // macOS/Linux keep names their filesystem accepts.
            assert_eq!(safe_rel("x/con.txt").unwrap(), vec!["x", "con.txt"]);
        }
        if cfg!(target_os = "macos") {
            assert_eq!(safe_rel("a:b.txt").unwrap(), vec!["a_b.txt"]);
        }
        assert_eq!(
            safe_rel("Ảnh lớp 12/ổ.jpg").unwrap(),
            vec!["Ảnh lớp 12", "ổ.jpg"]
        );
    }

    #[test]
    fn unique_names() {
        let d = tempfile::tempdir().unwrap();
        let r = HashSet::new();
        std::fs::write(d.path().join("a.jpg"), b"").unwrap();
        assert_eq!(
            unique(d.path().join("a.jpg"), &r, false),
            d.path().join("a (2).jpg")
        );
        std::fs::write(d.path().join("a (2).jpg"), b"").unwrap();
        assert_eq!(
            unique(d.path().join("a.jpg"), &r, false),
            d.path().join("a (3).jpg")
        );
        std::fs::create_dir(d.path().join("job.v1")).unwrap();
        assert_eq!(
            unique(d.path().join("job.v1"), &r, true),
            d.path().join("job.v1 (2)")
        );
        assert_eq!(
            unique(d.path().join(".rc"), &r, false),
            d.path().join(".rc")
        );
    }
}

#[cfg(test)]
mod reserved_tests {
    use super::*;

    #[test]
    fn vietnamese_words_that_are_windows_device_names_survive() {
        for n in ["con", "Con gái", "aux", "nul.txt", "com1", "LPT9.log"] {
            let out = sanitize_component(n, "x");
            assert!(!out.starts_with('_'), "{n} -> {out}");
            assert!(
                out.to_lowercase().starts_with(&n[..3].to_lowercase()),
                "{n} -> {out}"
            );
        }
        assert!(is_reserved_device_name("con"));
        assert!(is_reserved_device_name("Aux.tar.gz"));
        assert!(!is_reserved_device_name("Con gái"));
        assert!(!is_reserved_device_name("com0"));
        assert!(!is_reserved_device_name("console"));
        if cfg!(windows) {
            assert_eq!(sanitize_component("con", "x"), "con_");
            assert_eq!(sanitize_component("aux.txt", "x"), "aux_.txt");
        } else {
            assert_eq!(sanitize_component("con", "x"), "con");
        }
    }
}
