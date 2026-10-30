//! OS right-click integration: "Gửi bằng Toss" on folders and (v0.8) files.
//!
//! - Windows: per-user registry verbs under `HKCU\Software\Classes\Directory\shell\Toss`
//!   (right-click on a folder) and `...\Directory\Background\shell\Toss` (right-click inside a
//!   folder), both running `"<exe>" --share "<folder>"`; v0.8 `...\*\shell\Toss` (any file)
//!   running `"<exe>" --send "<file>"`, plus a **Send to → Toss** shortcut
//!   `%APPDATA%\Microsoft\Windows\SendTo\Toss.lnk` (`"<exe>" --send <selected items…>`).
//! - macOS: a Finder Quick Action bundle in `~/Library/Services` (see `services.rs`), for every
//!   file and folder (`public.item`).
//! - Linux (dev only): nothing.
//!
//! Every launch goes through tauri-plugin-single-instance to the running app, which opens
//! "Gửi tới ai?" (`shell::handle_args`). `apply(on)` is idempotent and is called at every startup
//! with the saved setting.

use anyhow::Result;

/// Menu label (Explorer verb / Quick Action name).
pub const LABEL: &str = "Gửi bằng Toss";

/// Quick Action names of older versions, removed when the current one is installed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const LEGACY_LABELS: [&str; 1] = ["Tạo link Toss"];

/// argv flag carrying the folder to share (0.3–0.6 verbs; still accepted).
pub const SHARE_FLAG: &str = "--share";

/// v0.8 argv flag: every following argument is a file / folder to send.
pub const SEND_FLAG: &str = "--send";

/// Registry keys (under HKCU), the flag, and the placeholder Explorer substitutes:
/// `%1` = the item right-clicked, `%V` = the folder whose background was right-clicked.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WIN_KEYS: [(&str, &str, &str); 3] = [
    (r"Software\Classes\Directory\shell\Toss", SHARE_FLAG, "%1"),
    (
        r"Software\Classes\Directory\Background\shell\Toss",
        SHARE_FLAG,
        "%V",
    ),
    (r"Software\Classes\*\shell\Toss", SEND_FLAG, "%1"),
];

/// File name of the Send to shortcut (shown as "Toss").
#[cfg_attr(not(windows), allow(dead_code))]
pub const SENDTO_LNK: &str = "Toss.lnk";

/// One Explorer verb to write.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinEntry {
    /// Key path under HKCU (the verb key; the command goes in its `command` subkey).
    pub key: &'static str,
    /// Default value of the verb key (menu text).
    pub label: String,
    /// `Icon` value of the verb key.
    pub icon: String,
    /// Default value of `<key>\command`.
    pub command: String,
}

/// `"<exe>" <flag> "<placeholder>"`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn win_command(exe: &str, flag: &str, placeholder: &str) -> String {
    format!("\"{exe}\" {flag} \"{placeholder}\"")
}

/// Every verb `set_context_menu(true)` writes on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn win_entries(exe: &str) -> Vec<WinEntry> {
    WIN_KEYS
        .iter()
        .map(|(key, flag, ph)| WinEntry {
            key,
            label: LABEL.to_string(),
            icon: format!("\"{exe}\",0"),
            command: win_command(exe, flag, ph),
        })
        .collect()
}

/// PowerShell that (re)creates the Send to shortcut `lnk` → `"<exe>" --send` (Explorer appends
/// the selected items). Run with `-EncodedCommand` (paths may be non-ASCII).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sendto_script(exe: &str, lnk: &str) -> String {
    use crate::firewall::ps_quote;
    format!(
        "$ErrorActionPreference='Stop'; $s=(New-Object -ComObject WScript.Shell).CreateShortcut({lnk}); \
$s.TargetPath={exe}; $s.Arguments={args}; $s.IconLocation={icon}; $s.Description={desc}; $s.Save()",
        lnk = ps_quote(lnk),
        exe = ps_quote(exe),
        args = ps_quote(SEND_FLAG),
        icon = ps_quote(&format!("{exe},0")),
        desc = ps_quote(LABEL),
    )
}

/// Register (`on`) or remove the right-click entries for the current user. Idempotent.
pub fn apply(on: bool) -> Result<()> {
    imp::apply(on)
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;

    use anyhow::{anyhow, Context, Result};
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    /// `%APPDATA%\Microsoft\Windows\SendTo\Toss.lnk`.
    fn sendto_lnk() -> Option<PathBuf> {
        dirs::data_dir().map(|d| {
            d.join("Microsoft")
                .join("Windows")
                .join("SendTo")
                .join(super::SENDTO_LNK)
        })
    }

    fn write_sendto(exe: &str, lnk: &std::path::Path) -> Result<()> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = super::sendto_script(exe, &lnk.to_string_lossy());
        let out = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &crate::firewall::encoded_command(&script),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .context("không chạy được PowerShell")?;
        if !out.status.success() {
            return Err(anyhow!(
                "không tạo được lối tắt Send to: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }

    pub fn apply(on: bool) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let lnk = sendto_lnk();
        if on {
            let exe = std::env::current_exe().context("không tìm thấy đường dẫn Toss.exe")?;
            let exe = exe.to_string_lossy().into_owned();
            let entries = super::win_entries(&exe);
            // The exe moved (reinstall elsewhere) when the stored command differs.
            let old_cmd: Option<String> = hkcu
                .open_subkey(format!(r"{}\command", entries[2].key))
                .and_then(|k| k.get_value(""))
                .ok();
            let moved = old_cmd.as_deref() != Some(entries[2].command.as_str());
            for e in &entries {
                let (verb, _) = hkcu
                    .create_subkey(e.key)
                    .with_context(|| format!("không ghi được registry {}", e.key))?;
                verb.set_value("", &e.label)?;
                verb.set_value("Icon", &e.icon)?;
                // Keep the verb when many items are selected (Explorer hides it above 15).
                verb.set_value("MultiSelectModel", &String::from("Player"))?;
                let (cmd, _) = verb.create_subkey("command")?;
                cmd.set_value("", &e.command)?;
            }
            if let Some(lnk) = lnk {
                if moved || !lnk.exists() {
                    write_sendto(&exe, &lnk)?;
                }
            }
        } else {
            for (key, _, _) in super::WIN_KEYS {
                match hkcu.delete_subkey_all(key) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e).with_context(|| format!("không xoá được registry {key}"))
                    }
                }
            }
            if let Some(lnk) = lnk.filter(|l| l.exists()) {
                std::fs::remove_file(&lnk)
                    .with_context(|| format!("không xoá được {}", lnk.display()))?;
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{anyhow, Result};

    pub fn apply(on: bool) -> Result<()> {
        let dir = crate::services::services_dir()
            .ok_or_else(|| anyhow!("không tìm thấy thư mục ~/Library/Services"))?;
        if on {
            if crate::services::install(&dir)? {
                crate::services::refresh();
            }
        } else if crate::services::remove(&dir)? {
            crate::services::refresh();
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    /// Linux is a dev platform only: no file-manager integration.
    pub fn apply(_on: bool) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_entries() {
        let exe = r"C:\Users\Minh\AppData\Local\Toss\Toss.exe";
        let e = win_entries(exe);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].key, r"Software\Classes\Directory\shell\Toss");
        assert_eq!(
            e[0].command,
            r#""C:\Users\Minh\AppData\Local\Toss\Toss.exe" --share "%1""#
        );
        assert_eq!(
            e[1].key,
            r"Software\Classes\Directory\Background\shell\Toss"
        );
        assert_eq!(
            e[1].command,
            r#""C:\Users\Minh\AppData\Local\Toss\Toss.exe" --share "%V""#
        );
        assert_eq!(e[2].key, r"Software\Classes\*\shell\Toss");
        assert_eq!(
            e[2].command,
            r#""C:\Users\Minh\AppData\Local\Toss\Toss.exe" --send "%1""#
        );
        for x in &e {
            assert_eq!(x.label, "Gửi bằng Toss");
            assert_eq!(x.icon, format!("\"{exe}\",0"));
        }
    }

    #[test]
    fn sendto_shortcut_script() {
        let s = sendto_script(
            r"C:\Users\Lê O'Neil\Toss\Toss.exe",
            r"C:\Users\Lê O'Neil\AppData\Roaming\Microsoft\Windows\SendTo\Toss.lnk",
        );
        assert!(
            s.contains(r"CreateShortcut('C:\Users\Lê O''Neil\AppData\Roaming\Microsoft\Windows\SendTo\Toss.lnk')"),
            "{s}"
        );
        assert!(s.contains(r"$s.TargetPath='C:\Users\Lê O''Neil\Toss\Toss.exe'"));
        assert!(s.contains("$s.Arguments='--send'"));
        assert!(s.ends_with("$s.Save()"));
        assert!(!s.contains('\n'));
    }

    #[test]
    fn hooks_nsh_removes_the_same_keys() {
        let nsh = include_str!("../bundle/hooks.nsh");
        for (key, _, _) in WIN_KEYS {
            assert!(
                nsh.contains(&format!("DeleteRegKey HKCU \"{key}\"")),
                "hooks.nsh must delete {key}"
            );
        }
        assert!(nsh.contains(&format!(r"Microsoft\Windows\SendTo\{SENDTO_LNK}")));
    }
}
