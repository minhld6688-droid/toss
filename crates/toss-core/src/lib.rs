//! # toss-core
//!
//! Core of Toss, a Blip-style P2P file transfer app: signed self-contained
//! tickets, the `toss/xfer/1` transfer protocol and the `toss/ctl/1` remote
//! control protocol, on top of an [iroh](https://docs.rs/iroh) endpoint
//! (n0 discovery + relays).
//!
//! Contracts: `contracts/wire.md` and `contracts/core-api.md` in the repository.
//! Everything is tokio-based and logs through `tracing`; nothing prints.

#![warn(missing_docs)]

pub mod contacts;
pub mod ctl;
pub mod fsutil;
pub mod jobs;
pub mod nearby;
pub mod net;
pub mod node;
pub mod paths;
pub mod resume;
pub mod ticket;
pub mod transfer;
pub mod util;
pub mod wire;

mod client;
mod host;
mod lanprobe;
mod mdns;
mod netdiag;
mod peer;
mod tuning;
mod vi;
mod xfer;

/// Test hooks (not part of the contract).
#[doc(hidden)]
pub mod testing {
    pub use crate::client::force_xfer1_client;
    pub use crate::xfer::set_timeouts;

    /// Sends one raw `toss/peer/1` request to `addr` and returns the answer
    /// (tests of the contact checks).
    pub async fn peer_request(
        node: &crate::Node,
        addr: iroh::EndpointAddr,
        req: &crate::wire::PeerReq,
    ) -> anyhow::Result<crate::wire::PeerResp> {
        let conn = node
            .inner
            .connect_alpns(addr, crate::ALPN_PEER, Vec::new())
            .await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        crate::wire::write_msg(&mut send, req).await?;
        let resp = crate::wire::read_msg_max(&mut recv, crate::wire::MAX_HELLO).await;
        conn.close(0u32.into(), b"test");
        resp
    }

    /// v0.8: puts `peer` into `node`'s nearby store as if mDNS had seen it
    /// (two nodes in one process usually do not see each other's multicast).
    pub fn add_nearby(node: &crate::Node, peer: crate::NearbyPeer) {
        crate::peer::nearby_seen(&node.inner, peer)
    }

    /// v0.8: as if mDNS reported `id` (hex) expired.
    pub fn expire_nearby(node: &crate::Node, id: &str) {
        crate::peer::nearby_gone(&node.inner, &[id.to_string()])
    }

    /// v0.8: pretends nearby entry `id` was last seen `secs` ago, then runs
    /// the staleness sweep (entries older than 3 min are dropped).
    pub fn age_nearby(node: &crate::Node, id: &str, secs: u64) {
        node.inner.nearby.age(id, secs);
        let gone = node.inner.nearby.sweep(std::time::Instant::now());
        if !gone.is_empty() {
            crate::peer::nearby_gone(&node.inner, &gone);
            crate::peer::nearby_changed(&node.inner);
        }
    }

    /// v0.8: the mDNS user data this node currently advertises.
    pub fn advertised_user_data(node: &crate::Node) -> String {
        crate::nearby::encode_user_data(
            node.device_name(),
            &crate::contacts::platform(),
            &node.inner.cfg.app_version,
        )
    }
}

pub use contacts::{Contact, IncomingPush, InviteInfo};
pub use ctl::CtlClient;
pub use jobs::{JobEvent, JobRecord};
pub use nearby::{NearbyPeer, ReceivePolicy};
pub use net::{ConnectReport, NetConfig, NetDiag, Tuning};
pub use node::{HostConfig, Node, NodeConfig, Root};
pub use ticket::{HostAddr, Ticket, TicketBody, TicketSpec};
pub use transfer::{ConnPath, Transfer, TransferEvent, TransferId, TransferState};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ALPN of the transfer protocol.
pub const ALPN_XFER: &[u8] = b"toss/xfer/1";
/// ALPN of the v0.4 transfer protocol (segmented data streams; contracts/speed-v0.4.md §4.3).
pub const ALPN_XFER2: &[u8] = b"toss/xfer/2";
/// ALPN of the control protocol.
pub const ALPN_CTL: &[u8] = b"toss/ctl/1";
/// ALPN of the v0.5 contacts protocol (pairing, presence, pushes; contacts-v0.5.md §2.2).
pub const ALPN_PEER: &[u8] = b"toss/peer/1";
