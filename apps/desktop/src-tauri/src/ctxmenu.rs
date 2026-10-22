//! OS right-click integration: "Tạo link Toss" on folders.
//!
//! - Windows: per-user registry verbs under `HKCU\Software\Classes\Directory\shell\Toss`
//!   (right-click on a folder) and `...\Directory\Background\shell\Toss` (right-click
//!   inside a folder), both running `"<exe>" --share "<folder>"`.
//! - macOS: a Finder Quick Action bundle in `~/Library/Services` (see `services.rs`).
//! - Linux (dev only): nothing.
//!
//! `apply(on)` is idempotent and is called at every startup with the saved setting.

use anyhow::Result;

/// Menu label (Explorer verb / Quick Action name).
pub const LABEL: &str = "Tạo link Toss";

/// argv flag carrying the folder to share.
pub const SHARE_FLAG: &str = "--share";

/// Registry keys (under HKCU) and the placeholder Explorer substitutes for the folder:
/// `%1` = the folder right-clicked, `%V` = the folder whose background was right-clicked.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WIN_KEYS: [(&str, &str); 2] = [
    (r"Software\Classes\Directory\shell\Toss", "%1"),
    (r"Software\Classes\Directory\Background\shell\Toss", "%V"),
];

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

/// `"<exe>" --share "<placeholder>"`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn win_command(exe: &str, placeholder: &str) -> String {
    format!("\"{exe}\" {SHARE_FLAG} \"{placeholder}\"")
}

/// Everything `set_context_menu(true)` writes on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn win_entries(exe: &str) -> Vec<WinEntry> {
    WIN_KEYS
        .iter()
        .map(|(key, ph)| WinEntry {
            key,
            label: LABEL.to_string(),
            icon: format!("\"{exe}\",0"),
            command: win_command(exe, ph),
        })
        .collect()
}

/// Register (`on`) or remove the right-click entry for the current user. Idempotent.
pub fn apply(on: bool) -> Result<()> {
    imp::apply(on)
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    pub fn apply(on: bool) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if on {
            let exe = std::env::current_exe().context("không tìm thấy đường dẫn Toss.exe")?;
            let exe = exe.to_string_lossy().into_owned();
            for e in super::win_entries(&exe) {
                let (verb, _) = hkcu
                    .create_subkey(e.key)
                    .with_context(|| format!("không ghi được registry {}", e.key))?;
                verb.set_value("", &e.label)?;
                verb.set_value("Icon", &e.icon)?;
                let (cmd, _) = verb.create_subkey("command")?;
                cmd.set_value("", &e.command)?;
            }
        } else {
            for (key, _) in super::WIN_KEYS {
                match hkcu.delete_subkey_all(key) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e).with_context(|| format!("không xoá được registry {key}"))
                    }
                }
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
        assert_eq!(e.len(), 2);
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
        for x in &e {
            assert_eq!(x.label, "Tạo link Toss");
            assert_eq!(x.icon, format!("\"{exe}\",0"));
        }
    }

    #[test]
    fn hooks_nsh_removes_the_same_keys() {
        let nsh = include_str!("../bundle/hooks.nsh");
        for (key, _) in WIN_KEYS {
            assert!(
                nsh.contains(&format!("DeleteRegKey HKCU \"{key}\"")),
                "hooks.nsh must delete {key}"
            );
        }
    }
}
