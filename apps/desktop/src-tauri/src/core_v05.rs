//! COORDINATOR: this is the ONLY file that touches the v0.5 `toss-core` API (WS-E, contracts/contacts-v0.5.md §3).
//! Those methods did not exist yet when the shell was written, so every function below is a STUB
//! (empty list / `Err("chưa hỗ trợ")` / no-op). Once WS-E has merged, replace each stub body by the call in
//! its `COORDINATOR:` comment and delete `NOT_YET`. Types in `crate::ipc` have the same serde shape as the core
//! structs, hence `via_serde` for the conversion (or `.into()` if you prefer a `From` impl).
//!
//! Nothing else in the shell needs to change: `contacts.rs` (the `Core` methods) and `commands.rs` call these
//! functions only, and the generic event forwarder already serializes the new `TransferEvent` variants
//! (`contacts_changed`, `contact_presence`, `push_incoming`, `lan_blocked`).
#![allow(unused_variables)]

use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Serialize};
use toss_core::Node;

use crate::ipc::{Contact, IncomingPush, InviteInfo};

const NOT_YET: &str = "chưa hỗ trợ";

/// Core struct -> shell mirror (identical serde shape).
#[allow(dead_code)]
pub fn via_serde<T: Serialize, U: DeserializeOwned>(t: &T) -> Result<U> {
    Ok(serde_json::from_value(serde_json::to_value(t)?)?)
}

/// STUB. COORDINATOR: `node.set_inbox_dir(dir)`
/// (called when the node starts and whenever `dest_dir` changes; pushes land in `<dir>/<contact>/<job>`).
pub fn set_inbox_dir(node: &Node, dir: PathBuf) {}

/// STUB. COORDINATOR: `via_serde(&node.contacts())`
pub fn contacts(node: &Node) -> Result<Vec<Contact>> {
    Ok(Vec::new())
}

/// STUB. COORDINATOR: `via_serde(&node.create_invite(days, page_host)?)`
pub fn create_invite(node: &Node, days: u32, page_host: &str) -> Result<InviteInfo> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `node.revoke_invites()`
pub fn revoke_invites(node: &Node) -> Result<()> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `via_serde(&Node::parse_invite(text)?)` (associated fn, needs no node)
pub fn parse_invite(text: &str) -> Result<InviteInfo> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `via_serde(&node.accept_invite(text).await?)`
pub async fn accept_invite(node: &Node, text: &str) -> Result<Contact> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `node.rename_contact(id, name)`
pub fn rename_contact(node: &Node, id: &str, name: &str) -> Result<()> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `node.set_contact_auto_accept(id, on)`
pub fn set_contact_auto_accept(node: &Node, id: &str, on: bool) -> Result<()> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `node.remove_contact(id)`
pub fn remove_contact(node: &Node, id: &str) -> Result<()> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `Ok(node.send_to_contact(id, paths).await?.id.clone())` (transfer id)
pub async fn send_to_contact(node: &Node, id: &str, paths: Vec<PathBuf>) -> Result<String> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `via_serde(&node.incoming_pushes())`
pub fn incoming_pushes(node: &Node) -> Result<Vec<IncomingPush>> {
    Ok(Vec::new())
}

/// STUB. COORDINATOR: `node.answer_push(push_id, accept)`
pub fn answer_push(node: &Node, push_id: &str, accept: bool) -> Result<()> {
    bail!(NOT_YET)
}

/// STUB. COORDINATOR: `node.refresh_presence().await`
pub async fn refresh_presence(node: &Node) {}
