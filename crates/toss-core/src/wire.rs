//! Protocol messages for `toss/xfer/1` and `toss/ctl/1` (wire.md §3–§4) and
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
            DenyReason::BadSignature => f.write_str("bad signature"),
            DenyReason::Expired => f.write_str("ticket expired"),
            DenyReason::Revoked => f.write_str("ticket revoked"),
            DenyReason::UnknownRoot => f.write_str("unknown root"),
            DenyReason::BadPath => f.write_str("bad path"),
            DenyReason::NotAllowed => f.write_str("not allowed"),
            DenyReason::Busy => f.write_str("host busy"),
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
