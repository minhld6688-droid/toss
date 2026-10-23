//! The only file that touches the v0.5 `toss-core` contacts API (contracts/contacts-v0.5.md §3).
//! `contacts.rs` (the `Core` methods) and `commands.rs` call these functions only. Types in
//! `crate::ipc` have the same serde shape as the core structs, hence `via_serde`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use toss_core::Node;

use crate::ipc::{Contact, IncomingPush, InviteInfo};

/// Core struct -> shell mirror (identical serde shape).
pub fn via_serde<T: Serialize, U: DeserializeOwned>(t: &T) -> Result<U> {
    Ok(serde_json::from_value(serde_json::to_value(t)?)?)
}

/// Pushes land in `<dir>/<contact>/<job>` (called on node start and whenever `dest_dir` changes).
pub fn set_inbox_dir(node: &Node, dir: PathBuf) {
    node.set_inbox_dir(dir)
}

pub fn contacts(node: &Node) -> Result<Vec<Contact>> {
    via_serde(&node.contacts())
}

pub fn create_invite(node: &Node, days: u32, page_host: &str) -> Result<InviteInfo> {
    via_serde(&node.create_invite(days, page_host)?)
}

pub fn revoke_invites(node: &Node) -> Result<()> {
    node.revoke_invites()
}

pub fn parse_invite(text: &str) -> Result<InviteInfo> {
    via_serde(&Node::parse_invite(text)?)
}

pub async fn accept_invite(node: &Node, text: &str) -> Result<Contact> {
    via_serde(&node.accept_invite(text).await?)
}

pub fn rename_contact(node: &Node, id: &str, name: &str) -> Result<()> {
    node.rename_contact(id, name)
}

pub fn set_contact_auto_accept(node: &Node, id: &str, on: bool) -> Result<()> {
    node.set_contact_auto_accept(id, on)
}

pub fn remove_contact(node: &Node, id: &str) -> Result<()> {
    node.remove_contact(id)
}

/// Returns the transfer id.
pub async fn send_to_contact(node: &Node, id: &str, paths: Vec<PathBuf>) -> Result<String> {
    Ok(node.send_to_contact(id, paths).await?.id.clone())
}

pub fn incoming_pushes(node: &Node) -> Result<Vec<IncomingPush>> {
    via_serde(&node.incoming_pushes())
}

pub fn answer_push(node: &Node, push_id: &str, accept: bool) -> Result<()> {
    node.answer_push(push_id, accept)
}

pub async fn refresh_presence(node: &Node) {
    node.refresh_presence().await
}
