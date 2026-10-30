//! Log file (`<data_dir>/logs/toss.log`, rotated at 5 MB or when the UTC day changes, 3 files
//! kept) next to stderr, and the "Xuất log" zip for support (contracts/speed-v0.4.md §3).
//!
//! `tracing_appender::rolling` only rotates by time and names files `toss.log.<date>`, so the
//! writer here does the rotation itself and `tracing_appender::non_blocking` keeps the disk off
//! the hot path.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde_json::Value;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Used when `RUST_LOG` is not set.
// swarm_discovery (mDNS) and localsend::multicast warn on EVERY packet while macOS blocks the
// local network (EHOSTUNREACH several times a second); lanprobe already reports that state.
pub const DEFAULT_FILTER: &str =
    "info,iroh=warn,toss_core=info,swarm_discovery=error,localsend::multicast=error";
/// Rotate the current file once it would grow past this.
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Files kept: `toss.log`, `toss.1.log`, `toss.2.log`.
const KEEP: usize = 3;
const BASE: &str = "toss";

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Installs the global subscriber: stderr + rolling file. A file that cannot be opened
/// only costs the file layer.
pub fn init(dir: &Path) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let file = match Rotating::open(dir.to_path_buf()) {
        Ok(w) => {
            let (nb, guard) = tracing_appender::non_blocking(w);
            let _ = GUARD.set(guard);
            Some(fmt::layer().with_ansi(false).with_writer(nb))
        }
        Err(e) => {
            eprintln!("log file {}: {e:#}", dir.display());
            None
        }
    };
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(io::stderr))
        .with(file)
        .try_init();
    // A GUI build has no console: make panics land in the log as well.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        prev(info);
    }));
}

fn day_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

/// `toss.log`, `toss.1.log`, …
fn file_name(n: usize) -> String {
    if n == 0 {
        format!("{BASE}.log")
    } else {
        format!("{BASE}.{n}.log")
    }
}

struct Rotating {
    dir: PathBuf,
    file: File,
    size: u64,
    day: u64,
    max: u64,
}

impl Rotating {
    fn open(dir: PathBuf) -> Result<Rotating> {
        Self::open_with(dir, MAX_BYTES)
    }

    fn open_with(dir: PathBuf, max: u64) -> Result<Rotating> {
        fs::create_dir_all(&dir).with_context(|| format!("tạo {}", dir.display()))?;
        let path = dir.join(file_name(0));
        // A file left from an earlier day starts a new generation, so every launch day is separate.
        let stale = fs::metadata(&path)
            .ok()
            .filter(|m| m.len() > 0)
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| day_of(t) != day_of(SystemTime::now()));
        if stale {
            shift(&dir);
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Rotating {
            dir,
            file,
            size,
            day: day_of(SystemTime::now()),
            max,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        let _ = self.file.flush();
        shift(&self.dir);
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(file_name(0)))?;
        self.size = 0;
        self.day = day_of(SystemTime::now());
        Ok(())
    }
}

/// Moves `toss.log` → `toss.1.log` → … and drops the oldest.
fn shift(dir: &Path) {
    let _ = fs::remove_file(dir.join(file_name(KEEP - 1)));
    for n in (0..KEEP - 1).rev() {
        let from = dir.join(file_name(n));
        if from.exists() {
            let _ = fs::rename(&from, dir.join(file_name(n + 1)));
        }
    }
}

impl Write for Rotating {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let today = day_of(SystemTime::now());
        if self.size > 0 && (self.size + buf.len() as u64 > self.max || today != self.day) {
            if let Err(e) = self.rotate() {
                // Keep logging into the open file rather than losing the line.
                eprintln!("log rotate: {e}");
            }
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

// ---- "Xuất log" -----------------------------------------------------------------------------

/// `settings.json` as safe to send to support: no `key`, and the admin link (a credential
/// that can mint download links on the host) is blanked.
pub fn redact_settings(raw: &[u8]) -> Vec<u8> {
    let mut v: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("key");
        o.remove("secret_key");
        if o.get("admin_ticket").is_some_and(|t| !t.is_null()) {
            o.insert("admin_ticket".into(), Value::String("<đã ẩn>".into()));
        }
    }
    serde_json::to_vec_pretty(&v).unwrap_or_default()
}

/// Local date-time stamp for the zip name; UTC (no time-zone database here).
pub fn stamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Writes `<dest_dir>/toss-logs-<stamp>.zip` with `logs/*` (regular files of `<data_dir>/logs`),
/// `settings.json` (redacted) and the given extra entries. Never touches `key`.
pub fn export_zip(data_dir: &Path, extra: &[(&str, Vec<u8>)], dest_dir: &Path) -> Result<PathBuf> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    fs::create_dir_all(dest_dir).with_context(|| format!("tạo {}", dest_dir.display()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let out = dest_dir.join(format!("toss-logs-{}.zip", stamp(now)));
    let mut zip =
        ZipWriter::new(File::create(&out).with_context(|| format!("tạo {}", out.display()))?);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut add = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, opts)?;
        zip.write_all(bytes)?;
        Ok(())
    };
    if let Ok(rd) = fs::read_dir(data_dir.join("logs")) {
        let mut files: Vec<_> = rd.flatten().filter(|e| e.path().is_file()).collect();
        files.sort_by_key(|e| e.file_name());
        for e in files {
            let name = e.file_name().to_string_lossy().into_owned();
            match fs::read(e.path()) {
                Ok(b) => add(&format!("logs/{name}"), &b)?,
                Err(err) => tracing::warn!("export logs: {name}: {err}"),
            }
        }
    }
    if let Ok(raw) = fs::read(data_dir.join("settings.json")) {
        add("settings.json", &redact_settings(&raw))?;
    }
    for (name, bytes) in extra {
        add(name, bytes)?;
    }
    zip.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("toss-logs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rotates_by_size_and_keeps_three() {
        let dir = tmp("rot");
        let mut w = Rotating::open_with(dir.clone(), 100).unwrap();
        for i in 0..10 {
            w.write_all(format!("line {i:03} {}\n", "x".repeat(40)).as_bytes())
                .unwrap();
        }
        w.flush().unwrap();
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["toss.1.log", "toss.2.log", "toss.log"]);
        // newest line is in toss.log
        assert!(fs::read_to_string(dir.join("toss.log"))
            .unwrap()
            .contains("line 009"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_file_from_an_earlier_day_starts_a_new_generation() {
        let dir = tmp("stale");
        fs::write(dir.join("toss.log"), "old day\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 86_400);
        File::options()
            .write(true)
            .open(dir.join("toss.log"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut w = Rotating::open(dir.clone()).unwrap();
        w.write_all(b"today\n").unwrap();
        w.flush().unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("toss.1.log")).unwrap(),
            "old day\n"
        );
        assert_eq!(fs::read_to_string(dir.join("toss.log")).unwrap(), "today\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_redacted() {
        let raw = br#"{"device_name":"May","key":"SECRET","admin_ticket":"toss1abc","host":null}"#;
        let out = String::from_utf8(redact_settings(raw)).unwrap();
        assert!(!out.contains("SECRET") && !out.contains("toss1abc"));
        assert!(out.contains("\"device_name\""));
        let none = String::from_utf8(redact_settings(br#"{"admin_ticket":null}"#)).unwrap();
        assert!(none.contains("null"));
    }

    #[test]
    fn stamp_formats_utc() {
        assert_eq!(stamp(0), "1970-01-01-000000");
        assert_eq!(stamp(1_792_627_200 + 3_661), "2026-10-22-010101");
    }

    #[test]
    fn zip_has_logs_and_redacted_settings_but_never_the_key() {
        let data = tmp("zip-data");
        let dest = tmp("zip-dest");
        fs::create_dir_all(data.join("logs")).unwrap();
        fs::write(data.join("logs/toss.log"), "hello log\n").unwrap();
        fs::write(data.join("key"), "TOP SECRET KEY").unwrap();
        fs::write(
            data.join("settings.json"),
            r#"{"device_name":"May","admin_ticket":"toss1xyz"}"#,
        )
        .unwrap();
        let out = export_zip(&data, &[("diagnostics.json", b"{}".to_vec())], &dest).unwrap();
        assert!(out.starts_with(&dest));
        let mut z = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["diagnostics.json", "logs/toss.log", "settings.json"]
        );
        let mut s = String::new();
        z.by_name("settings.json")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert!(!s.contains("toss1xyz"));
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&dest);
    }
}
