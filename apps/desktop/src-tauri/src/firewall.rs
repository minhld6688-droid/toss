//! OS firewall check and "Cho phép qua firewall" (contracts/speed-v0.4.md §3).
//!
//! Windows: PowerShell `Get-NetFirewall*` to look for an enabled inbound UDP allow rule for this
//! exe; an elevated `New-NetFirewallRule` (one UAC prompt) to add it. macOS: `socketfilterfw`
//! (`--getglobalstate`, `--getappblocked`; `--add` + `--unblockapp` through an
//! `osascript … with administrator privileges` prompt). Linux: nothing to do.
//!
//! Everything platform-specific shells out with `std::process::Command`; the script builders and
//! output parsers are plain functions so they are unit-tested on every OS.
#![cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

#[allow(unused_imports)]
use anyhow::{anyhow, bail, Result};
use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FirewallState {
    Ok,
    Missing,
    Unknown,
}

/// How long a check result is reused (the UI polls every 5 s; a Windows check takes seconds).
const TTL: Duration = Duration::from_secs(30);
/// A check that hangs must not hang the diagnostics panel.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// Display name of the rule we add (and replace).
pub const RULE_NAME: &str = "Toss";

static CACHE: Mutex<Option<(Instant, FirewallState)>> = Mutex::new(None);
static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cached() -> Option<FirewallState> {
    CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .filter(|(at, _)| at.elapsed() < TTL)
        .map(|(_, s)| s)
}

/// Forget the cached check (after the rule was added).
pub fn invalidate() {
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Firewall state for this exe; cached for `TTL`, never blocks the runtime.
pub async fn state() -> FirewallState {
    if let Some(s) = cached() {
        return s;
    }
    let _gate = GATE.lock().await; // one check at a time
    if let Some(s) = cached() {
        return s;
    }
    let job = tokio::task::spawn_blocking(check);
    let s = match tokio::time::timeout(CHECK_TIMEOUT, job).await {
        Ok(Ok(s)) => s,
        _ => FirewallState::Unknown,
    };
    *CACHE.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), s));
    s
}

/// Adds the allow rule (blocking: waits for the admin prompt). Errors are Vietnamese text.
pub fn add_rule() -> Result<()> {
    let r = add_rule_impl();
    invalidate();
    r
}

#[cfg(not(any(windows, target_os = "macos")))]
fn check() -> FirewallState {
    FirewallState::Unknown
}

#[cfg(not(any(windows, target_os = "macos")))]
fn add_rule_impl() -> Result<()> {
    Ok(()) // nothing to do on Linux
}

// ---- Windows -----------------------------------------------------------------------------------

/// Prints `ok` or `missing`. Reads `$env:TOSS_EXE`. One line (no here-strings, no quoting issues).
/// Collects the filters first: an empty pipeline into `Get-NetFirewallRule` would list every rule.
const WIN_CHECK_PS: &str = "$ErrorActionPreference='Stop'; \
$exe=$env:TOSS_EXE; \
$on=@(Get-NetFirewallProfile | Where-Object { $_.Enabled -eq 'True' }); \
if ($on.Count -eq 0) { 'ok'; exit 0 }; \
$f=@(Get-NetFirewallApplicationFilter | Where-Object { $_.Program -ieq $exe }); \
if ($f.Count -eq 0) { 'missing'; exit 0 }; \
$r=@($f | Get-NetFirewallRule | Where-Object { $_.Direction -eq 'Inbound' -and $_.Enabled -eq 'True' } | Where-Object { @('UDP','Any') -contains (($_ | Get-NetFirewallPortFilter).Protocol) }); \
if (@($r | Where-Object { $_.Action -eq 'Block' }).Count -gt 0) { 'missing'; exit 0 }; \
if (@($r | Where-Object { $_.Action -eq 'Allow' }).Count -gt 0) { 'ok'; exit 0 }; \
'missing'";

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The elevated script: replaces every rule called "Toss", adds a UDP inbound allow for `exe`
/// on all profiles, and leaves the error text in `log` when it fails (the elevated process has
/// no console we can read).
fn win_add_script(exe: &str, log: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop'; try {{ \
Remove-NetFirewallRule -DisplayName {name} -ErrorAction SilentlyContinue; \
New-NetFirewallRule -DisplayName {name} -Direction Inbound -Action Allow -Protocol UDP -Program {exe} -Profile Any | Out-Null \
}} catch {{ [IO.File]::WriteAllText({log}, $_.Exception.Message); exit 1 }}",
        name = ps_quote(RULE_NAME),
        exe = ps_quote(exe),
        log = ps_quote(log),
    )
}

/// UTF-16LE + base64, the form `powershell -EncodedCommand` takes.
fn encoded_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    data_encoding::BASE64.encode(&bytes)
}

/// Runs elevated through `Start-Process -Verb RunAs`; exit 1223 = UAC refused.
fn win_launcher_script(encoded: &str) -> String {
    format!(
        "try {{ $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
-ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{encoded}'; exit $p.ExitCode }} \
catch {{ exit 1223 }}"
    )
}

#[cfg(windows)]
fn powershell(script: &str, exe: &str) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .env("TOSS_EXE", exe)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(windows)]
fn check() -> FirewallState {
    let Ok(exe) = std::env::current_exe() else {
        return FirewallState::Unknown;
    };
    match powershell(WIN_CHECK_PS, &exe.to_string_lossy()) {
        Ok(o) if o.status.success() => parse_win_check(&String::from_utf8_lossy(&o.stdout)),
        Ok(o) => {
            tracing::warn!(
                "firewall check failed ({}): {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            FirewallState::Unknown
        }
        Err(e) => {
            tracing::warn!("firewall check: {e}");
            FirewallState::Unknown
        }
    }
}

#[cfg(windows)]
fn add_rule_impl() -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = std::env::temp_dir().join(format!("toss-firewall-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let inner = win_add_script(&exe.to_string_lossy(), &log.to_string_lossy());
    let launcher = win_launcher_script(&encoded_command(&inner));
    let out = powershell(&launcher, &exe.to_string_lossy())
        .map_err(|e| anyhow!("Không chạy được PowerShell: {e}"))?;
    let code = out.status.code().unwrap_or(-1);
    let detail = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    match code {
        0 => Ok(()),
        1223 => bail!("Bạn đã từ chối quyền quản trị, firewall chưa được mở"),
        _ if !detail.trim().is_empty() => bail!("Không tạo được luật firewall: {}", detail.trim()),
        _ => bail!(
            "Không tạo được luật firewall (mã {code}) {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

// ---- macOS -------------------------------------------------------------------------------------

#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
const SOCKETFILTERFW: &str = "/usr/libexec/ApplicationFirewall/socketfilterfw";

/// The `.app` bundle around `exe` (`…/Toss.app/Contents/MacOS/Toss` → `…/Toss.app`), else `exe`.
#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn app_target(exe: &Path) -> PathBuf {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .unwrap_or(exe)
        .to_path_buf()
}

#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// AppleScript string literal body (`\` and `"` escaped).
#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn osa_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `do shell script … with administrator privileges` for `--add` + `--unblockapp`.
#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn mac_admin_script(app: &str) -> String {
    let q = sh_quote(app);
    let cmd = format!("{SOCKETFILTERFW} --add {q} ; {SOCKETFILTERFW} --unblockapp {q}");
    format!(
        "do shell script \"{}\" with administrator privileges",
        osa_escape(&cmd)
    )
}

/// `--getglobalstate`: "Firewall is disabled. (State = 0)" → off.
#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn parse_global_state(out: &str) -> Option<bool> {
    let o = out.to_lowercase();
    if o.contains("disabled") || o.contains("state = 0") {
        Some(false)
    } else if o.contains("enabled") || o.contains("state = 1") || o.contains("state = 2") {
        Some(true)
    } else {
        None
    }
}

/// `--getappblocked <app>`: blocked / permitted / not in the list.
#[cfg_attr(windows, allow(dead_code))] // macOS helpers; unit-tested on every OS
fn parse_app_blocked(out: &str) -> FirewallState {
    let o = out.to_lowercase();
    if o.contains("not blocked") || o.contains("permitted") || o.contains("allowed") {
        FirewallState::Ok
    } else if o.contains("blocked") || o.contains("not in the firewall") || o.contains("not part") {
        // Not in the list: an unattended host can never answer the "accept incoming" prompt.
        FirewallState::Missing
    } else {
        FirewallState::Unknown
    }
}

fn parse_win_check(out: &str) -> FirewallState {
    match out.trim().lines().last().map(str::trim) {
        Some("ok") => FirewallState::Ok,
        Some("missing") => FirewallState::Missing,
        _ => FirewallState::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn check() -> FirewallState {
    use std::process::Command;
    let Ok(exe) = std::env::current_exe() else {
        return FirewallState::Unknown;
    };
    let run = |args: &[&str]| {
        Command::new(SOCKETFILTERFW)
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    match run(&["--getglobalstate"])
        .as_deref()
        .and_then(parse_global_state)
    {
        Some(false) => return FirewallState::Ok,
        Some(true) => {}
        None => return FirewallState::Unknown,
    }
    let app = app_target(&exe);
    run(&["--getappblocked", &app.to_string_lossy()])
        .map_or(FirewallState::Unknown, |o| parse_app_blocked(&o))
}

#[cfg(target_os = "macos")]
fn add_rule_impl() -> Result<()> {
    let exe = std::env::current_exe()?;
    let app = app_target(&exe);
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(mac_admin_script(&app.to_string_lossy()))
        .output()
        .map_err(|e| anyhow!("Không chạy được osascript: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("-128") || err.to_lowercase().contains("cancel") {
        bail!("Bạn đã huỷ yêu cầu mật khẩu, firewall chưa được mở");
    }
    bail!("Không thêm được Toss vào firewall: {}", err.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_scripts() {
        let s = win_add_script(r"C:\Users\O'Neil\Toss\Toss.exe", r"C:\Temp\x.txt");
        assert!(
            s.contains("-Protocol UDP")
                && s.contains("-Direction Inbound")
                && s.contains("-Action Allow")
        );
        assert!(
            s.contains(r"-Program 'C:\Users\O''Neil\Toss\Toss.exe'"),
            "{s}"
        );
        assert!(s.contains("Remove-NetFirewallRule -DisplayName 'Toss'"));
        // base64 of UTF-16LE: "A" -> "QQA="
        assert_eq!(encoded_command("A"), "QQA=");
        let l = win_launcher_script("QQA=");
        assert!(l.contains("-Verb RunAs") && l.contains("'QQA='") && l.contains("1223"));
        assert!(
            !WIN_CHECK_PS.contains('\n'),
            "check script must be one line"
        );
        assert_eq!(parse_win_check("ok\r\n"), FirewallState::Ok);
        assert_eq!(parse_win_check("warning\nmissing"), FirewallState::Missing);
        assert_eq!(parse_win_check(""), FirewallState::Unknown);
    }

    #[test]
    fn mac_script_quotes_the_path() {
        let s = mac_admin_script("/Applications/Toss's \"App\".app");
        assert!(
            s.starts_with("do shell script \"") && s.ends_with("with administrator privileges")
        );
        assert!(
            s.contains("--add '/Applications/Toss'\\\\''s \\\"App\\\".app'"),
            "{s}"
        );
        assert!(s.contains("--unblockapp"));
        assert_eq!(
            app_target(Path::new("/Applications/Toss.app/Contents/MacOS/Toss")),
            PathBuf::from("/Applications/Toss.app")
        );
        assert_eq!(
            app_target(Path::new("/tmp/toss")),
            PathBuf::from("/tmp/toss")
        );
    }

    #[test]
    fn mac_output_parsing() {
        assert_eq!(
            parse_global_state("Firewall is disabled. (State = 0)"),
            Some(false)
        );
        assert_eq!(
            parse_global_state("Firewall is enabled. (State = 1)"),
            Some(true)
        );
        assert_eq!(parse_global_state("???"), None);
        assert_eq!(
            parse_app_blocked("Application at path ( /Applications/Toss.app ) is not blocked"),
            FirewallState::Ok
        );
        assert_eq!(
            parse_app_blocked("The application is blocked from receiving incoming connections"),
            FirewallState::Missing
        );
        assert_eq!(parse_app_blocked("some other text"), FirewallState::Unknown);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&FirewallState::Missing).unwrap(),
            "\"missing\""
        );
    }
}
