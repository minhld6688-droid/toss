//! Protocol messages for `toss/xfer/1`, `toss/ctl/1` (wire.md §3–§4) and
//! `toss/peer/1` (contacts-v0.5.md §2.2), and
//! the length-prefixed postcard framing.
//!
//! Every control message is `u32 LE length` + `postcard(msg)`, max 4 MiB.
//! Postcard encodes enums by variant index and structs by field order, so the
//! declaration order here *is* the wire format. Serde renames only affect JSON.

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub use crate::jobs::{JobEvent, JobRecord};
use crate::ticket::{Ticket, TicketSpec};

/// Maximum size of one framed control message.
pub const MAX_MSG: usize = 4 * 1024 * 1024;

/// Maximum size of the first message of a connection ([`ClientHello`] or
/// [`CtlRequest::Auth`]), read before the peer is authenticated. A ticket is
/// ~250 bytes; this leaves ample room for names while keeping what an
/// unauthenticated peer can make the host buffer small.
pub const MAX_HELLO: usize = 64 * 1024;

/// First message of a `toss/xfer/1` connection.
#[allow(non_camel_case_types)]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientHello {
    /// Version 1 hello.
    v1 {
        /// The bearer ticket.
        ticket: Ticket,
        /// Client device name (shown on the host).
        device: String,
        /// Requested direction.
        op: Op,
        /// Client app version.
        app_version: String,
    },
}

/// Transfer direction.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// host → client
    Get,
    /// client → host
    Put,
}

/// Host answer to [`ClientHello`].
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum HostHello {
    /// Accepted; `job` is the job name.
    Ok {
        /// Job name.
        job: String,
    },
    /// Refused; the host closes the connection.
    Denied {
        /// Why.
        reason: DenyReason,
    },
}

/// Why a host refused a request.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    /// Signature invalid or wrong host.
    BadSignature,
    /// Ticket expired.
    Expired,
    /// Ticket revoked.
    Revoked,
    /// Root alias not configured on the host.
    UnknownRoot,
    /// Path invalid, missing or escaping its root.
    BadPath,
    /// Operation not allowed by the ticket (or host mode off).
    NotAllowed,
    /// Too many concurrent jobs; retry later.
    Busy,
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // v0.8: Vietnamese — these texts reach the UI as transfer errors.
            DenyReason::BadSignature => f.write_str("Link không hợp lệ (sai chữ ký hoặc sai máy)"),
            DenyReason::Expired => f.write_str("Link đã hết hạn — xin link mới"),
            DenyReason::Revoked => f.write_str("Link đã bị thu hồi"),
            DenyReason::UnknownRoot => {
                f.write_str("Máy chia sẻ không còn thư mục này (đã bỏ chia sẻ?)")
            }
            DenyReason::BadPath => {
                f.write_str("File hoặc thư mục trong link không còn trên máy chia sẻ")
            }
            DenyReason::NotAllowed => f.write_str("Link này không cho phép việc này"),
            DenyReason::Busy => f.write_str("Máy chia sẻ đang bận"),
            DenyReason::Other(s) => write!(f, "{s}"),
        }
    }
}

/// One file in a [`Manifest`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative path, `/`-separated.
    pub rel: String,
    /// Size in bytes.
    pub size: u64,
    /// blake3 of the content.
    pub hash: [u8; 32],
}

/// The list of files the sender offers.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Files.
    pub files: Vec<FileEntry>,
    /// Sum of sizes.
    pub total: u64,
}

/// Files the receiver still needs.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Want {
    /// Items.
    pub items: Vec<WantItem>,
}

/// One wanted file, resumed from `offset`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WantItem {
    /// Index into `Manifest::files`.
    pub index: u32,
    /// Byte offset to resume from.
    pub offset: u64,
}

/// End-of-transfer summary.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Done {
    /// Files completed in this session.
    pub files: u32,
    /// Bytes moved in this session.
    pub bytes: u64,
    /// Everything verified.
    pub ok: bool,
    /// Free text (renames, failures).
    pub msg: String,
}

/// `toss/ctl/1` request.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum CtlRequest {
    /// Must be the first request; ticket must be a valid admin ticket.
    Auth {
        /// Admin ticket.
        ticket: Ticket,
        /// Device name.
        device: String,
    },
    /// Host status.
    Status,
    /// One directory level.
    ListDir {
        /// Root alias.
        root: String,
        /// Path under root.
        path: String,
    },
    /// Mint a job ticket.
    CreateTicket(TicketSpec),
    /// All jobs.
    ListJobs,
    /// Revoke a job ticket.
    Revoke {
        /// Ticket id.
        id: [u8; 16],
    },
    /// The (deterministically re-signed) ticket of a stored job, answered with
    /// [`CtlResponse::Ticket`]. Admin tickets are never handed out this way.
    /// Appended last so the postcard indices of the variants above stay stable.
    JobTicket {
        /// Ticket id.
        id: [u8; 16],
    },
    /// v0.5.1: ask the host app to check for and install an app update now
    /// (answered `Ok` once the request is handed to the app; the app may
    /// restart shortly after). Appended last: indices above stay stable.
    RequestUpdate,
}

/// `toss/ctl/1` response.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum CtlResponse {
    /// Success without payload.
    Ok,
    /// Refused.
    Denied {
        /// Why.
        reason: DenyReason,
    },
    /// Status.
    Status(HostStatus),
    /// Directory listing.
    Dir {
        /// Entries.
        entries: Vec<DirEntry>,
    },
    /// New ticket.
    Ticket(Ticket),
    /// Job list.
    Jobs(Vec<JobRecord>),
}

/// Host status snapshot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HostStatus {
    /// Host device name.
    pub device: String,
    /// Host app version.
    pub version: String,
    /// Root aliases.
    pub roots: Vec<String>,
    /// Running transfers.
    pub active: u32,
    /// Clients recently turned away with `Busy` (they retry every 30 s).
    pub queued: u32,
    /// Seconds since the node started.
    pub uptime_s: u64,
}

/// One entry of a directory listing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// File name.
    pub name: String,
    /// Directory?
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Modification time, unix seconds.
    pub mtime: u64,
}

/// Writes one framed message.
pub async fn write_msg<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let bytes = postcard::to_allocvec(msg).context("encode message")?;
    if bytes.len() > MAX_MSG {
        bail!("message too large ({} bytes)", bytes.len());
    }
    let len = u32::try_from(bytes.len()).context("message length")?;
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&bytes);
    w.write_all(&buf).await.context("write message")?;
    Ok(())
}

/// Reads one framed message (at most [`MAX_MSG`] bytes).
pub async fn read_msg<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    read_msg_max(r, MAX_MSG).await
}

/// Reads one framed message of at most `max` bytes (capped at [`MAX_MSG`]).
///
/// The buffer grows with the bytes actually received, so a peer announcing a
/// large length without sending it cannot make us allocate it.
pub async fn read_msg_max<R: AsyncRead + Unpin, T: DeserializeOwned>(
    r: &mut R,
    max: usize,
) -> Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .await
        .context("read message length")?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max.min(MAX_MSG) {
        bail!("message too large ({len} bytes)");
    }
    let mut buf = Vec::with_capacity(len.min(64 * 1024));
    let n = (&mut *r)
        .take(len as u64)
        .read_to_end(&mut buf)
        .await
        .context("read message body")?;
    if n != len {
        bail!("message truncated ({n} of {len} bytes)");
    }
    postcard::from_bytes(&buf).context("decode message")
}

/// Uni-stream header: `u32 LE index`, `u64 LE offset`.
pub fn encode_stream_header(index: u32, offset: u64) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[..4].copy_from_slice(&index.to_le_bytes());
    h[4..].copy_from_slice(&offset.to_le_bytes());
    h
}

/// Reads a uni-stream header.
pub async fn read_stream_header<R: AsyncRead + Unpin>(r: &mut R) -> Result<(u32, u64)> {
    let mut h = [0u8; 12];
    r.read_exact(&mut h).await.context("read stream header")?;
    let mut i = [0u8; 4];
    i.copy_from_slice(&h[..4]);
    let mut o = [0u8; 8];
    o.copy_from_slice(&h[4..]);
    Ok((u32::from_le_bytes(i), u64::from_le_bytes(o)))
}

/// Length of an xfer/2 segment header.
pub const SEG_HEADER_LEN: usize = 20;

/// xfer/2 segment header: `u32 LE index`, `u64 LE offset`, `u64 LE len`;
/// `len` bytes of file data follow. A data stream carries one or more
/// segments back to back and ends with the stream (contracts/speed-v0.4.md §4.3).
pub fn encode_segment_header(index: u32, offset: u64, len: u64) -> [u8; SEG_HEADER_LEN] {
    let mut h = [0u8; SEG_HEADER_LEN];
    h[..4].copy_from_slice(&index.to_le_bytes());
    h[4..12].copy_from_slice(&offset.to_le_bytes());
    h[12..].copy_from_slice(&len.to_le_bytes());
    h
}

/// Decodes an xfer/2 segment header into `(index, offset, len)`.
pub fn decode_segment_header(h: &[u8; SEG_HEADER_LEN]) -> (u32, u64, u64) {
    let mut i = [0u8; 4];
    i.copy_from_slice(&h[..4]);
    let mut o = [0u8; 8];
    o.copy_from_slice(&h[4..12]);
    let mut l = [0u8; 8];
    l.copy_from_slice(&h[12..]);
    (
        u32::from_le_bytes(i),
        u64::from_le_bytes(o),
        u64::from_le_bytes(l),
    )
}

// ───────────────────── toss/peer/1 (contacts-v0.5.md §2.2) ─────────────────────

/// Request on the first bi stream of a `toss/peer/1` connection (max 64 KiB).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum PeerReq {
    /// From a stranger holding an invite of the dialed device.
    Pair {
        /// The invite secret.
        secret: [u8; 16],
        /// Dialer device name.
        name: String,
        /// Dialer platform.
        platform: String,
        /// Dialer app version.
        version: String,
    },
    /// Presence ping from a contact.
    Hello {
        /// Sender device name.
        name: String,
        /// Sender app version.
        version: String,
    },
    /// A contact wants to send files; on `PushAccepted` the stream continues
    /// with xfer/2 put semantics (Manifest → Want → segments → Done).
    Push {
        /// Random id; the same id is reused when the push resumes.
        push_id: [u8; 16],
        /// Job (folder) name.
        job: String,
        /// Files in the job.
        files: u32,
        /// Bytes in the job.
        bytes: u64,
    },
    /// v0.8 (nearby-v0.8.md §1.2): a push from a device that is *not* a
    /// contact of the receiver, allowed only when the receiver sees the sender
    /// on its LAN (mDNS nearby store). Same data phase as [`PeerReq::Push`].
    /// Appended: the indices above stay stable (0.6 peers decode them).
    PushFrom {
        /// Random id; the same id is reused when the push resumes.
        push_id: [u8; 16],
        /// Job (folder) name.
        job: String,
        /// Files in the job.
        files: u32,
        /// Bytes in the job.
        bytes: u64,
        /// Sender device name.
        name: String,
        /// Sender platform.
        platform: String,
        /// Sender app version.
        version: String,
    },
    /// v0.8: a short text message (≤ [`MAX_TEXT`] bytes), answered `TextOk`.
    Text {
        /// Sender device name.
        name: String,
        /// The message.
        text: String,
    },
}

/// Longest text message (`PeerReq::Text`), bytes of UTF-8.
pub const MAX_TEXT: usize = 60 * 1024;

/// Answer to a [`PeerReq`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum PeerResp {
    /// Pairing accepted; the answering device's own name and platform.
    Paired {
        /// Device name.
        name: String,
        /// Platform.
        platform: String,
        /// App version.
        version: String,
    },
    /// Presence answer.
    Pong {
        /// Device name.
        name: String,
        /// App version.
        version: String,
        /// A transfer is running.
        busy: bool,
    },
    /// Send the manifest.
    PushAccepted,
    /// The user (or the 10 min timeout) declined.
    PushDeclined {
        /// Why (Vietnamese).
        reason: String,
    },
    /// Waiting for the user; `PushAccepted` / `PushDeclined` follows on the same stream.
    PushQueued,
    /// Not a contact / bad invite.
    Denied {
        /// Why (Vietnamese).
        reason: String,
    },
    /// v0.8: answer to `PushFrom` — accepted, and the receiver now has the
    /// sender as a contact ("Luôn nhận từ máy này"); the sender adds the
    /// receiver as a contact too. Then the data phase follows like `PushAccepted`.
    PushAcceptedPaired {
        /// Receiver device name.
        name: String,
        /// Receiver platform.
        platform: String,
        /// Receiver app version.
        version: String,
    },
    /// v0.8: answer to `Text` — shown to the user.
    TextOk,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that announces `MAX_MSG` bytes and then delivers them slowly:
    /// the receive buffer must grow with the data, not be allocated upfront.
    struct Trickle {
        header: [u8; 4],
        pos: usize,
        body_left: usize,
        max_buf_seen: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            // The caller's free space tells how much it pre-allocated.
            self.max_buf_seen
                .fetch_max(buf.remaining(), std::sync::atomic::Ordering::Relaxed);
            if self.pos < 4 {
                let n = (4 - self.pos).min(buf.remaining());
                let start = self.pos;
                buf.put_slice(&self.header[start..start + n]);
                self.pos += n;
            } else if self.body_left > 0 {
                let n = 100.min(self.body_left).min(buf.remaining());
                buf.put_slice(&vec![0u8; n]);
                self.body_left -= n;
            }
            // else: EOF (peer went away before sending the announced body)
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn announced_length_is_not_preallocated() {
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut r = Trickle {
            header: (MAX_MSG as u32).to_le_bytes(),
            pos: 0,
            body_left: 1000,
            max_buf_seen: seen.clone(),
        };
        let err = read_msg::<_, Manifest>(&mut r).await.unwrap_err();
        assert!(format!("{err:#}").contains("truncated"), "{err:#}");
        let max = seen.load(std::sync::atomic::Ordering::Relaxed);
        assert!(max < 1024 * 1024, "reader saw a {max}-byte buffer");
    }

    #[tokio::test]
    async fn hello_cap() {
        let mut buf = Vec::new();
        write_msg(&mut buf, &"x".repeat(MAX_HELLO)).await.unwrap();
        assert!(read_msg_max::<_, String>(&mut buf.as_slice(), MAX_HELLO)
            .await
            .is_err());
        assert!(read_msg::<_, String>(&mut buf.as_slice()).await.is_ok());
    }

    #[test]
    fn ctl_variant_indices_are_stable() {
        // postcard encodes the variant index first; JobTicket must stay last.
        let enc = |r: &CtlRequest| postcard::to_allocvec(r).unwrap()[0];
        assert_eq!(enc(&CtlRequest::Status), 1);
        assert_eq!(enc(&CtlRequest::ListJobs), 4);
        assert_eq!(enc(&CtlRequest::Revoke { id: [0; 16] }), 5);
        assert_eq!(enc(&CtlRequest::JobTicket { id: [0; 16] }), 6);
    }

    #[test]
    fn peer_variant_indices_are_stable() {
        // 0.6 peers decode indices 0..=2 (req) / 0..=5 (resp); v0.8 appends.
        let req = |r: &PeerReq| postcard::to_allocvec(r).unwrap()[0];
        let resp = |r: &PeerResp| postcard::to_allocvec(r).unwrap()[0];
        assert_eq!(
            req(&PeerReq::Push {
                push_id: [0; 16],
                job: String::new(),
                files: 0,
                bytes: 0
            }),
            2
        );
        assert_eq!(
            req(&PeerReq::PushFrom {
                push_id: [0; 16],
                job: String::new(),
                files: 0,
                bytes: 0,
                name: String::new(),
                platform: String::new(),
                version: String::new(),
            }),
            3
        );
        assert_eq!(
            req(&PeerReq::Text {
                name: String::new(),
                text: String::new()
            }),
            4
        );
        assert_eq!(
            resp(&PeerResp::Denied {
                reason: String::new()
            }),
            5
        );
        assert_eq!(
            resp(&PeerResp::PushAcceptedPaired {
                name: String::new(),
                platform: String::new(),
                version: String::new()
            }),
            6
        );
        assert_eq!(resp(&PeerResp::TextOk), 7);
        // A maximal text message fits the 64 KiB first-message cap.
        let big = PeerReq::Text {
            name: "x".repeat(64 * 4),
            text: "x".repeat(MAX_TEXT),
        };
        assert!(postcard::to_allocvec(&big).unwrap().len() + 4 <= MAX_HELLO);
    }

    #[tokio::test]
    async fn framing_roundtrip() {
        let m = Manifest {
            files: vec![FileEntry {
                rel: "a/b".into(),
                size: 3,
                hash: [1; 32],
            }],
            total: 3,
        };
        let mut buf = Vec::new();
        write_msg(&mut buf, &m).await.unwrap();
        let back: Manifest = read_msg(&mut buf.as_slice()).await.unwrap();
        assert_eq!(back, m);

        let mut big = Vec::new();
        big.extend_from_slice(&((MAX_MSG as u32) + 1).to_le_bytes());
        assert!(read_msg::<_, Manifest>(&mut big.as_slice()).await.is_err());

        // Truncated body is an error, not a short decode.
        let mut short = buf.clone();
        short.truncate(buf.len() - 1);
        assert!(read_msg::<_, Manifest>(&mut short.as_slice())
            .await
            .is_err());

        let s = encode_segment_header(3, 1 << 33, 5);
        assert_eq!(&s[..4], &[3, 0, 0, 0]);
        assert_eq!(decode_segment_header(&s), (3, 1 << 33, 5));

        let h = encode_stream_header(7, 1 << 40);
        assert_eq!(
            read_stream_header(&mut h.as_slice()).await.unwrap(),
            (7, 1 << 40)
        );
    }
}
