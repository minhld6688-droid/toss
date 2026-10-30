//! v0.8 LocalSend bridge adapter (contracts/nearby-v0.8.md §2–§3). The ONLY file that names
//! `toss-ls` types: everything above it speaks the shell mirrors below ([`LsDev`], [`LsEv`]), so
//! the event logic in `nearby.rs` is unit-tested without a running bridge.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::sync::mpsc;

use toss_ls::{LsBridge, LsConfig, LsDevice, LsEvent};

/// LocalSend's default port; the bridge falls back to 53318..=53327 (then any) when it is busy.
pub const LS_PORT: u16 = toss_ls::DEFAULT_PORT;
/// Last port of the range the firewall rules open.
pub const LS_PORT_LAST: u16 = *toss_ls::FALLBACK_PORTS.end();
/// LocalSend's multicast group (discovery announcements).
pub const LS_MULTICAST: &str = "224.0.0.167:53317";

/// A LocalSend device as the shell sees it (mirror of `toss_ls::LsDevice`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LsDev {
    pub fingerprint: String,
    pub alias: String,
    pub model: Option<String>,
    /// mobile | desktop | web | headless | server
    pub kind: String,
    pub ip: String,
    /// The device announces `device_model = "Toss"` (another Toss on the LAN).
    pub is_toss: bool,
}

/// Mirror of `toss_ls::LsEvent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LsEv {
    DevicesChanged,
    Incoming {
        req_id: String,
        fingerprint: String,
        alias: String,
        model: Option<String>,
        files: u32,
        bytes: u64,
        names: Vec<String>,
        text: Option<String>,
    },
    Started {
        id: String,
        outgoing: bool,
        peer: String,
        job: String,
        files: u32,
        bytes: u64,
    },
    Progress {
        id: String,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u32,
        files_total: u32,
        speed_bps: u64,
    },
    Done {
        id: String,
        dest: Option<PathBuf>,
        files: u32,
        bytes: u64,
    },
    Error {
        id: String,
        msg: String,
    },
    Text {
        fingerprint: String,
        alias: String,
        text: String,
    },
    Port {
        port: u16,
        fallback: bool,
    },
}

fn dev(d: LsDevice) -> LsDev {
    LsDev {
        fingerprint: d.fingerprint,
        alias: d.alias,
        model: d.model,
        kind: d.kind,
        ip: d.ip,
        is_toss: d.is_toss,
    }
}

fn ev(e: LsEvent) -> LsEv {
    match e {
        LsEvent::DevicesChanged => LsEv::DevicesChanged,
        LsEvent::Incoming {
            req_id,
            fingerprint,
            alias,
            model,
            files,
            bytes,
            names,
            text,
            ..
        } => LsEv::Incoming {
            req_id,
            fingerprint,
            alias,
            model,
            files,
            bytes,
            names,
            text,
        },
        LsEvent::Started {
            id,
            outgoing,
            peer,
            job,
            files,
            bytes,
        } => LsEv::Started {
            id,
            outgoing,
            peer,
            job,
            files,
            bytes,
        },
        LsEvent::Progress {
            id,
            bytes_done,
            bytes_total,
            files_done,
            files_total,
            speed_bps,
        } => LsEv::Progress {
            id,
            bytes_done,
            bytes_total,
            files_done,
            files_total,
            speed_bps,
        },
        LsEvent::Done {
            id,
            dest,
            files,
            bytes,
        } => LsEv::Done {
            id,
            dest,
            files,
            bytes,
        },
        LsEvent::Error { id, msg } => LsEv::Error { id, msg },
        LsEvent::Text {
            fingerprint,
            alias,
            text,
        } => LsEv::Text {
            fingerprint,
            alias,
            text,
        },
        LsEvent::Port { port, fallback } => LsEv::Port { port, fallback },
    }
}

/// A running bridge (cheap to clone).
#[derive(Clone)]
pub struct Ls {
    bridge: LsBridge,
}

impl Ls {
    /// Starts the LocalSend server + discovery (`alias` = device name, files land under `dest`).
    pub async fn start(
        alias: &str,
        data_dir: &Path,
        dest: &Path,
    ) -> Result<(Ls, mpsc::Receiver<LsEv>)> {
        let (bridge, mut rx) = LsBridge::start(LsConfig {
            alias: alias.to_string(),
            data_dir: data_dir.to_path_buf(),
            dest_dir: dest.to_path_buf(),
            port: LS_PORT,
        })
        .await?;
        let (tx, out) = mpsc::channel(256);
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if tx.send(ev(e)).await.is_err() {
                    break;
                }
            }
        });
        Ok((Ls { bridge }, out))
    }

    pub fn port(&self) -> u16 {
        self.bridge.port()
    }

    pub fn devices(&self) -> Vec<LsDev> {
        self.bridge.devices().into_iter().map(dev).collect()
    }

    pub async fn rescan(&self) {
        self.bridge.rescan().await
    }

    pub async fn set_alias(&self, alias: &str) {
        self.bridge.set_alias(alias).await
    }

    pub fn set_dest(&self, dir: PathBuf) {
        self.bridge.set_dest(dir)
    }

    pub fn answer(&self, req_id: &str, accept: bool) {
        self.bridge.answer(req_id, accept)
    }

    /// Returns the transfer id.
    pub async fn send(&self, fingerprint: &str, items: Vec<PathBuf>) -> Result<String> {
        self.bridge.send(fingerprint, items).await
    }

    pub async fn send_text(&self, fingerprint: &str, text: &str) -> Result<()> {
        self.bridge.send_text(fingerprint, text).await
    }

    pub fn cancel(&self, transfer_id: &str) {
        self.bridge.cancel(transfer_id)
    }

    /// Browser download page on the LAN; returns its URLs (`http://<lan-ip>:<port>`).
    pub async fn web_share_start(&self, items: Vec<PathBuf>) -> Result<Vec<String>> {
        Ok(self.bridge.web_share_start(items).await?.urls)
    }

    pub async fn web_share_stop(&self) {
        self.bridge.web_share_stop().await
    }

    pub async fn stop(&self) {
        self.bridge.stop().await
    }
}
