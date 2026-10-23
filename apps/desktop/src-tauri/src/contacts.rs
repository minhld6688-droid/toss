//! v0.5 "Danh bạ máy": the `Core` methods behind the contact commands (contracts/contacts-v0.5.md §3/§4).
//! Validation and Vietnamese messages live here; every call into `toss-core` goes through
//! `core_v05.rs` (stubs until WS-E's API is merged).

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::{
    core::Core,
    core_v05,
    ipc::{Contact, IncomingPush, InviteInfo},
};

/// Longest contact name we store (the invite itself caps the device name at 64).
const MAX_NAME: usize = 64;

/// macOS "Local Network" privacy pane (what the denial probe tells the user to open).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const LOCAL_NETWORK_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

fn clean_contact_name(n: &str) -> Result<String> {
    let n = n.trim();
    if n.is_empty() {
        bail!("Tên máy không được để trống");
    }
    if n.chars().count() > MAX_NAME {
        bail!("Tên máy quá dài (tối đa {MAX_NAME} ký tự)");
    }
    if n.chars().any(char::is_control) {
        bail!("Tên máy có ký tự không hợp lệ");
    }
    Ok(n.to_string())
}

fn clean_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty() {
        bail!("Thiếu mã máy");
    }
    Ok(id)
}

impl Core {
    /// Contacts with their current presence. Also nudges a presence round in the background so the
    /// next `contacts:changed` / `contact:presence` events carry fresh state.
    pub async fn list_contacts(&self) -> Result<Vec<Contact>> {
        let node = self.node().await?;
        let list = core_v05::contacts(&node)?;
        if !list.is_empty() {
            let n = node.clone();
            tauri::async_runtime::spawn(async move { core_v05::refresh_presence(&n).await });
        }
        Ok(list)
    }

    pub async fn create_invite(&self, days: u32) -> Result<InviteInfo> {
        let node = self.node().await?;
        let page_host = self.settings().page_host();
        core_v05::create_invite(&node, days.clamp(1, 30), &page_host)
    }

    pub async fn revoke_invites(&self) -> Result<()> {
        let node = self.node().await?;
        core_v05::revoke_invites(&node)
    }

    /// Decodes an invite (`tossinv1…`, `toss://i/…` or a web link) without pairing: the UI asks
    /// "Kết nối với <name>?" first. Additive command, not in the frozen contract.
    pub fn parse_invite(&self, text: &str) -> Result<InviteInfo> {
        core_v05::parse_invite(text.trim())
    }

    pub async fn accept_invite(&self, text: String) -> Result<Contact> {
        let node = self.node().await?;
        core_v05::accept_invite(&node, text.trim()).await
    }

    pub async fn rename_contact(&self, id: String, name: String) -> Result<()> {
        let name = clean_contact_name(&name)?;
        let node = self.node().await?;
        core_v05::rename_contact(&node, clean_id(&id)?, &name)
    }

    pub async fn set_contact_auto_accept(&self, id: String, on: bool) -> Result<()> {
        let node = self.node().await?;
        core_v05::set_contact_auto_accept(&node, clean_id(&id)?, on)
    }

    pub async fn remove_contact(&self, id: String) -> Result<()> {
        let node = self.node().await?;
        core_v05::remove_contact(&node, clean_id(&id)?)
    }

    pub async fn send_to_contact(&self, id: String, paths: Vec<String>) -> Result<String> {
        if paths.is_empty() {
            bail!("Chưa chọn file hoặc folder nào");
        }
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        if let Some(p) = paths.iter().find(|p| !p.exists()) {
            bail!("Không tìm thấy {}", p.display());
        }
        let node = self.node().await?;
        core_v05::send_to_contact(&node, clean_id(&id)?, paths).await
    }

    pub async fn list_incoming_pushes(&self) -> Result<Vec<IncomingPush>> {
        let node = self.node().await?;
        core_v05::incoming_pushes(&node)
    }

    pub async fn answer_push(&self, push_id: String, accept: bool) -> Result<()> {
        let node = self.node().await?;
        core_v05::answer_push(&node, push_id.trim(), accept)
    }
}

/// Opens the macOS Local Network privacy pane; a no-op elsewhere.
pub fn open_local_network_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let st = std::process::Command::new("open")
            .arg(LOCAL_NETWORK_URL)
            .status()
            .map_err(|e| anyhow::anyhow!("Không mở được Cài đặt hệ thống: {e}"))?;
        if !st.success() {
            bail!("Không mở được Cài đặt hệ thống");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_names() {
        assert_eq!(clean_contact_name("  Máy Đạt  ").unwrap(), "Máy Đạt");
        assert!(clean_contact_name("   ").is_err());
        assert!(clean_contact_name(&"x".repeat(65)).is_err());
        assert!(clean_contact_name("a\nb").is_err());
        assert!(clean_id(" ").is_err());
    }

    #[test]
    fn local_network_pane_url() {
        assert!(LOCAL_NETWORK_URL.ends_with("Privacy_LocalNetwork"));
        assert!(open_local_network_settings().is_ok() || cfg!(target_os = "macos"));
    }
}
