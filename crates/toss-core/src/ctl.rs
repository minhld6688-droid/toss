//! `toss/ctl/1`: remote host control with an admin ticket (wire.md §4).

use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::sync::Mutex;

use crate::{
    host::{issue_ticket_and_prehash, verify_common, MAX_DEVICE},
    jobs::JobRecord,
    node::{list_dir, Inner},
    ticket::{Ticket, TicketSpec},
    util::clean_text,
    wire::{
        read_msg, read_msg_max, write_msg, CtlRequest, CtlResponse, DenyReason, DirEntry,
        HostStatus, MAX_HELLO,
    },
    xfer::pre_auth_timeout,
    ALPN_CTL,
};

/// Client for a host's control channel. Reconnects transparently once if the
/// connection was lost between calls.
#[derive(Debug)]
pub struct CtlClient {
    inner: Arc<Inner>,
    ticket: Ticket,
    conn: Mutex<Option<(Connection, SendStream, RecvStream)>>,
}

impl CtlClient {
    pub(crate) async fn connect(inner: Arc<Inner>, ticket: Ticket) -> Result<CtlClient> {
        if !ticket.body.admin {
            bail!("not an admin ticket");
        }
        ticket.verify_signature().map_err(|e| anyhow!(e))?;
        if ticket.is_expired() {
            bail!("admin ticket expired");
        }
        let c = CtlClient {
            inner,
            ticket,
            conn: Mutex::new(None),
        };
        {
            let mut g = c.conn.lock().await;
            *g = Some(c.dial().await?);
        }
        Ok(c)
    }

    async fn dial(&self) -> Result<(Connection, SendStream, RecvStream)> {
        let conn = self
            .inner
            .dial_host(&self.ticket, ALPN_CTL)
            .await
            .context("host offline")?;
        let (mut send, mut recv) = conn.open_bi().await.context("open control stream")?;
        let auth = CtlRequest::Auth {
            ticket: self.ticket.clone(),
            device: self.inner.cfg.device_name.clone(),
        };
        write_msg(&mut send, &auth).await?;
        match read_msg::<_, CtlResponse>(&mut recv).await? {
            CtlResponse::Ok => Ok((conn, send, recv)),
            CtlResponse::Denied { reason } => {
                conn.close(0u32.into(), b"denied");
                bail!("host refused admin ticket: {reason}")
            }
            other => bail!("unexpected response {other:?}"),
        }
    }

    async fn request(&self, req: CtlRequest) -> Result<CtlResponse> {
        let mut g = self.conn.lock().await;
        for attempt in 0..2 {
            if g.is_none() {
                *g = Some(self.dial().await?);
            }
            let Some((_, send, recv)) = g.as_mut() else {
                continue;
            };
            let res = async {
                write_msg(send, &req).await?;
                read_msg::<_, CtlResponse>(recv).await
            }
            .await;
            match res {
                Ok(CtlResponse::Denied { reason }) => bail!("host refused: {reason}"),
                Ok(r) => return Ok(r),
                Err(e) if attempt == 0 => {
                    tracing::debug!("control connection lost ({e:#}); reconnecting");
                    *g = None;
                }
                Err(e) => return Err(e),
            }
        }
        bail!("control request failed")
    }

    /// Host status.
    pub async fn status(&self) -> Result<HostStatus> {
        match self.request(CtlRequest::Status).await? {
            CtlResponse::Status(s) => Ok(s),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// One level of `root/rel` on the host.
    pub async fn list_dir(&self, root: &str, rel: &str) -> Result<Vec<DirEntry>> {
        let req = CtlRequest::ListDir {
            root: root.into(),
            path: rel.into(),
        };
        match self.request(req).await? {
            CtlResponse::Dir { entries } => Ok(entries),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// Mints a job ticket on the host.
    pub async fn create_ticket(&self, spec: TicketSpec) -> Result<Ticket> {
        match self.request(CtlRequest::CreateTicket(spec)).await? {
            CtlResponse::Ticket(t) => Ok(t),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// Jobs on the host.
    pub async fn list_jobs(&self) -> Result<Vec<JobRecord>> {
        match self.request(CtlRequest::ListJobs).await? {
            CtlResponse::Jobs(j) => Ok(j),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// Revokes a ticket on the host.
    pub async fn revoke(&self, id: [u8; 16]) -> Result<()> {
        match self.request(CtlRequest::Revoke { id }).await? {
            CtlResponse::Ok => Ok(()),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// The ticket of a job stored on the host (re-signed by the host, identical
    /// to the one handed out at creation), e.g. to offer "copy link" for jobs
    /// listed with [`CtlClient::list_jobs`]. Fails for admin tickets, unknown
    /// ids, and on hosts older than this request.
    pub async fn job_ticket(&self, id: [u8; 16]) -> Result<Ticket> {
        match self.request(CtlRequest::JobTicket { id }).await? {
            CtlResponse::Ticket(t) => Ok(t),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// v0.5.1: asks the host's desktop app to update itself now (it checks the
    /// release feed, installs and restarts; running transfers are paused and
    /// resume after the restart). Fails on hosts older than 0.5.1.
    pub async fn request_update(&self) -> Result<()> {
        match self.request(CtlRequest::RequestUpdate).await? {
            CtlResponse::Ok => Ok(()),
            other => bail!("unexpected response {other:?}"),
        }
    }

    /// Closes the connection.
    pub async fn close(&self) {
        if let Some((conn, mut send, _)) = self.conn.lock().await.take() {
            let _ = send.finish();
            conn.close(0u32.into(), b"bye");
        }
    }
}

/// Host side of `toss/ctl/1`.
pub(crate) async fn handle_ctl(inner: Arc<Inner>, conn: Connection) {
    if let Err(e) = serve_ctl(&inner, &conn).await {
        tracing::info!(remote = %conn.remote_id().fmt_short(), alpn = "toss/ctl/1", "control session ended: {e:#}");
    }
    conn.close(0u32.into(), b"bye");
}

async fn serve_ctl(inner: &Arc<Inner>, conn: &Connection) -> Result<()> {
    // The whole unauthenticated phase is bounded in time and size.
    let (mut send, mut recv, first) = tokio::time::timeout(pre_auth_timeout(), async {
        let (send, mut recv) = conn.accept_bi().await?;
        let first: CtlRequest = read_msg_max(&mut recv, MAX_HELLO).await?;
        anyhow::Ok((send, recv, first))
    })
    .await
    .context("auth timeout")??;
    let CtlRequest::Auth { ticket, device } = first else {
        write_msg(
            &mut send,
            &CtlResponse::Denied {
                reason: DenyReason::NotAllowed,
            },
        )
        .await?;
        bail!("denied: first request was not Auth");
    };
    let device = clean_text(&device, MAX_DEVICE);
    let verdict = verify_common(inner, &ticket).and_then(|()| {
        ticket
            .body
            .admin
            .then_some(())
            .ok_or(DenyReason::NotAllowed)
    });
    if let Err(reason) = verdict {
        write_msg(
            &mut send,
            &CtlResponse::Denied {
                reason: reason.clone(),
            },
        )
        .await?;
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
        bail!("denied: admin auth from {device}: {reason}");
    }
    tracing::info!(remote = %conn.remote_id().fmt_short(), %device, "admin connected");
    write_msg(&mut send, &CtlResponse::Ok).await?;
    loop {
        let req: CtlRequest = match read_msg(&mut recv).await {
            Ok(r) => r,
            Err(_) => return Ok(()), // client went away
        };
        // Revocation of the admin ticket takes effect immediately (an
        // unreadable revocation list counts as revoked).
        if inner.jobs.is_revoked(&ticket.body.id) || ticket.is_expired() {
            write_msg(
                &mut send,
                &CtlResponse::Denied {
                    reason: DenyReason::Revoked,
                },
            )
            .await?;
            return Ok(());
        }
        let resp = answer(inner, req, &device).await;
        write_msg(&mut send, &resp).await?;
    }
}

async fn answer(inner: &Arc<Inner>, req: CtlRequest, device: &str) -> CtlResponse {
    let other = |e: anyhow::Error| CtlResponse::Denied {
        reason: DenyReason::Other(format!("{e:#}")),
    };
    let Some(rt) = inner.host_runtime() else {
        return CtlResponse::Denied {
            reason: DenyReason::NotAllowed,
        };
    };
    let inner = inner.clone();
    let device = device.to_string();
    let res = tokio::task::spawn_blocking(move || -> Result<CtlResponse> {
        Ok(match req {
            CtlRequest::Auth { .. } => CtlResponse::Ok,
            CtlRequest::Status => match inner.host_status() {
                Some(s) => CtlResponse::Status(s),
                None => CtlResponse::Denied {
                    reason: DenyReason::NotAllowed,
                },
            },
            CtlRequest::ListDir { root, path } => {
                if rt.root(&root).is_none() {
                    CtlResponse::Denied {
                        reason: DenyReason::UnknownRoot,
                    }
                } else {
                    CtlResponse::Dir {
                        entries: list_dir(&rt, &root, &path)?,
                    }
                }
            }
            CtlRequest::CreateTicket(spec) => {
                CtlResponse::Ticket(issue_ticket_and_prehash(&inner, &rt, spec)?)
            }
            CtlRequest::ListJobs => CtlResponse::Jobs(inner.jobs.list()),
            CtlRequest::Revoke { id } => {
                inner.jobs.revoke(id)?;
                CtlResponse::Ok
            }
            CtlRequest::RequestUpdate => {
                tracing::info!(%device, "update requested by admin");
                inner.emit(crate::transfer::TransferEvent::UpdateRequested {
                    device: device.clone(),
                });
                CtlResponse::Ok
            }
            CtlRequest::JobTicket { id } => {
                let host = *inner.endpoint.id().as_bytes();
                match inner.jobs.body(id, host) {
                    // Never hand out admin tickets: a holder could keep control
                    // after its own admin ticket is revoked.
                    Some(mut body) if !body.admin => {
                        body.set_host_addr(&inner.host_addr());
                        CtlResponse::Ticket(Ticket::sign(body, &inner.secret))
                    }
                    Some(_) => CtlResponse::Denied {
                        reason: DenyReason::NotAllowed,
                    },
                    None => CtlResponse::Denied {
                        reason: DenyReason::Other("unknown job".into()),
                    },
                }
            }
        })
    })
    .await;
    match res {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => other(e),
        Err(e) => other(e.into()),
    }
}
