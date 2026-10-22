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

pub mod ctl;
pub mod fsutil;
pub mod jobs;
pub mod node;
pub mod paths;
pub mod ticket;
pub mod transfer;
pub mod util;
pub mod wire;

mod client;
mod host;
mod xfer;

/// Test hooks (not part of the contract).
#[doc(hidden)]
pub mod testing {
    pub use crate::xfer::set_timeouts;
}

pub use ctl::CtlClient;
pub use jobs::{JobEvent, JobRecord};
pub use node::{HostConfig, Node, NodeConfig, Root};
pub use ticket::{Ticket, TicketBody, TicketSpec};
pub use transfer::{ConnPath, Transfer, TransferEvent, TransferId, TransferState};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ALPN of the transfer protocol.
pub const ALPN_XFER: &[u8] = b"toss/xfer/1";
/// ALPN of the control protocol.
pub const ALPN_CTL: &[u8] = b"toss/ctl/1";
