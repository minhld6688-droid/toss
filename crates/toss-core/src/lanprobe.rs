//! v0.5 Local Network denial probe (contracts/contacts-v0.5.md §2.4).
//!
//! macOS 15+ asks the user before an app may talk to the local network. An
//! unsigned build that was denied (or never got the prompt) can still use the
//! internet, so iroh silently falls back to the relay (≈1 MB/s instead of
//! 50+ MB/s on the same LAN): every send to a LAN or multicast address fails
//! with `EHOSTUNREACH`. We send one UDP datagram to the mDNS group and to the
//! default gateway; both failing that way while the machine has a private IPv4
//! address means Toss is blocked. Other OSes: always `false`.

use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use crate::{node::Inner, transfer::TransferEvent};

/// Diagnostics reason shown first while blocked.
pub(crate) const REASON: &str = "macOS đang chặn Toss truy cập mạng cục bộ — vào Cài đặt hệ \
     thống → Quyền riêng tư & Bảo mật → Mạng cục bộ và bật Toss, rồi mở lại Toss";

/// Probe period.
const INTERVAL: Duration = Duration::from_secs(60);

/// The errors a denied Local Network permission produces on a LAN send
/// (`EHOSTUNREACH`, `ENETUNREACH`, `EPERM`).
pub(crate) fn is_denial(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::PermissionDenied
    )
}

/// Blocked = a private IPv4 address exists and the multicast send failed with a
/// denial error. v0.8: the gateway send is no longer required to fail too — on a real
/// Mac (fresh unsigned build, permission not granted yet) every multicast send failed
/// with `EHOSTUNREACH` while a unicast datagram to the router still went out, and
/// nearby discovery (mDNS + LocalSend multicast) was dead. The gateway result is kept
/// for the log only.
pub(crate) fn decide(
    has_private_v4: bool,
    multicast: &io::Result<()>,
    _gateway: Option<&io::Result<()>>,
) -> bool {
    let denied = |r: &io::Result<()>| matches!(r, Err(e) if is_denial(e));
    has_private_v4 && denied(multicast)
}

/// One UDP datagram from a fresh socket.
fn send_one(dst: SocketAddr) -> io::Result<()> {
    let sock = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.send_to(&[0u8], dst).map(|_| ())
}

fn is_private_v4(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.is_private())
}

/// Runs the probe (blocking: a few syscalls). `local` = this node's addresses.
fn probe_now(local: &[IpAddr]) -> bool {
    let router = netwatch::interfaces::HomeRouter::new();
    let has_private = local.iter().any(is_private_v4)
        || router
            .as_ref()
            .and_then(|r| r.my_ip)
            .is_some_and(|ip| is_private_v4(&ip));
    let multicast = send_one(SocketAddr::from((Ipv4Addr::new(224, 0, 0, 251), 5353)));
    let gateway = router
        .as_ref()
        .filter(|r| is_private_v4(&r.gateway))
        .map(|r| send_one(SocketAddr::new(r.gateway, 9)));
    let blocked = decide(has_private, &multicast, gateway.as_ref());
    tracing::debug!(
        has_private,
        ?multicast,
        ?gateway,
        blocked,
        "local network probe"
    );
    blocked
}

/// Probes now; `false` on every OS but macOS.
pub(crate) fn probe(local: &[IpAddr]) -> bool {
    if cfg!(target_os = "macos") {
        probe_now(local)
    } else {
        false
    }
}

/// Probes on start and every 60 s (macOS only), keeps `Inner::lan_blocked`
/// and emits `LanBlocked` on change.
pub(crate) fn spawn(inner: &Arc<Inner>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        loop {
            {
                let Some(inner) = weak.upgrade() else { return };
                if inner.endpoint.is_closed() {
                    return;
                }
                let local: Vec<IpAddr> = inner.endpoint.addr().ip_addrs().map(|a| a.ip()).collect();
                let blocked = tokio::task::spawn_blocking(move || probe(&local))
                    .await
                    .unwrap_or(false);
                if inner.lan_blocked.swap(blocked, Ordering::SeqCst) != blocked {
                    if blocked {
                        tracing::warn!("macOS blocks local network access: nearby devices are hidden and same-LAN transfers go through the relay");
                    } else {
                        tracing::info!("local network access OK");
                    }
                    inner.emit(TransferEvent::LanBlocked { blocked });
                }
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn errno_classification() {
        fn err(errno: i32) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(errno))
        }
        // Raw errno values differ per OS; use this platform's.
        #[cfg(target_os = "linux")]
        let (ehostunreach, enetunreach, eperm, econnrefused, enobufs) = (113, 101, 1, 111, 105);
        #[cfg(target_os = "macos")]
        let (ehostunreach, enetunreach, eperm, econnrefused, enobufs) = (65, 51, 1, 61, 55);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            for e in [ehostunreach, enetunreach, eperm] {
                assert!(is_denial(&io::Error::from_raw_os_error(e)), "errno {e}");
            }
            for e in [econnrefused, enobufs] {
                assert!(!is_denial(&io::Error::from_raw_os_error(e)), "errno {e}");
            }
            let denied = err(ehostunreach);
            let ok: io::Result<()> = Ok(());
            assert!(decide(true, &denied, Some(&err(enetunreach))));
            assert!(decide(true, &denied, None));
            assert!(!decide(false, &denied, Some(&err(ehostunreach))));
            assert!(!decide(true, &ok, Some(&err(ehostunreach))));
            // v0.8: multicast denied is enough, even when the router still answers.
            assert!(decide(true, &denied, Some(&ok)));
            assert!(!decide(true, &err(enobufs), None));
        }
    }

    #[test]
    fn kinds_and_other_os() {
        let e = |k| io::Error::new(k, "x");
        assert!(is_denial(&e(io::ErrorKind::HostUnreachable)));
        assert!(!is_denial(&e(io::ErrorKind::TimedOut)));
        if !cfg!(target_os = "macos") {
            assert!(!probe(&["192.168.1.2".parse().unwrap()]));
        }
    }
}
