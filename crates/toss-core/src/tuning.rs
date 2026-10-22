//! QUIC transport tuning and pipeline sizing (WS-D).
//! Contract: `contracts/speed-v0.4.md` §4.1–§4.2.

use std::{sync::Arc, time::Duration};

use iroh::endpoint::{Connection, QuicTransportConfig, VarInt};
use noq_proto::congestion::{Bbr3Config, CubicConfig};

use crate::{
    net::{Congestion, Tuning},
    transfer::{conn_path, conn_rtt_ms, ConnPath},
};

const MIB: u64 = 1024 * 1024;

/// RTT above which a transfer uses `streams_high_rtt` streams.
pub(crate) const HIGH_RTT_MS: u32 = 30;
/// Largest batch of small files carried by one stream.
pub(crate) const MAX_BATCH_BYTES: u64 = 16 * MIB;
/// Most files in one batch (keeps per-stream bookkeeping bounded).
pub(crate) const MAX_BATCH_FILES: usize = 4096;
/// Largest segment of a split file. Small segments keep the out-of-order
/// window (and so what a disconnect throws away) at ~streams × 64 MiB.
pub(crate) const MAX_SEGMENT: u64 = 64 * MIB;
/// Upper bound of the per-stream receive window actually configured.
///
/// noq's reassembly buffer refuses more than 1024 non-contiguous pieces per
/// stream and then kills the *connection* (`INTERNAL_ERROR: too many gaps in
/// stream buffer`). With an 8 MiB window (~7000 packets) a slow-reading
/// receiver plus packet loss gets there: the loopback bench died that way at
/// 4–8 MiB (BBR and Cubic) and never at 1–2 MiB. Bandwidth comes from more
/// streams instead: 8–16 streams × 2 MiB = 16–32 MiB in flight, above the
/// bandwidth-delay product of 1 Gbit/s at 130–260 ms.
pub(crate) const MAX_STREAM_WINDOW: u64 = 2 * MIB;

/// Transport config for the endpoint (windows, congestion controller, stream limits).
/// iroh's keep-alive / idle / multipath defaults are kept (the builder starts from them).
pub(crate) fn transport_config(t: &Tuning) -> QuicTransportConfig {
    let t = sane(t);
    let stream_window = (u64::from(t.stream_window_mb) * MIB).min(MAX_STREAM_WINDOW);
    let streams = u64::from(t.streams.max(t.streams_high_rtt));
    let conn_window = stream_window.saturating_mul(streams);
    let b = QuicTransportConfig::builder()
        .stream_receive_window(varint(stream_window))
        .receive_window(varint(conn_window))
        .send_window(u64::from(t.send_window_mb) * MIB)
        // Room for every data stream of a transfer plus a spare set while the
        // previous ones drain (and a few for xfer/1 peers).
        .max_concurrent_uni_streams(VarInt::from_u32(
            (2 * t.streams_high_rtt.max(t.streams)).max(100),
        ))
        .initial_rtt(Duration::from_millis(50));
    let b = match t.congestion {
        Congestion::Bbr => b.congestion_controller_factory(Arc::new(Bbr3Config::default())),
        Congestion::Cubic => b.congestion_controller_factory(Arc::new(CubicConfig::default())),
    };
    b.build()
}

fn varint(v: u64) -> VarInt {
    VarInt::from_u64(v).unwrap_or(VarInt::MAX)
}

/// `t` with every field clamped to a usable range (settings.json is user-editable).
pub(crate) fn sane(t: &Tuning) -> Tuning {
    Tuning {
        stream_window_mb: t.stream_window_mb.clamp(1, 256),
        send_window_mb: t.send_window_mb.clamp(1, 1024),
        congestion: t.congestion,
        streams: t.streams.clamp(1, 64),
        streams_high_rtt: t.streams_high_rtt.clamp(1, 64),
        chunk_kb: t.chunk_kb.clamp(16, 16 * 1024),
        range_split_mb: t.range_split_mb.max(1),
        small_file_kb: t.small_file_kb,
    }
}

/// Stream target for a connection: `streams`, or `streams_high_rtt` while the
/// path is relay or the RTT is above [`HIGH_RTT_MS`]; capped
/// by the host's `max_parallel_streams` when that is set (see [`host_stream_cap`]).
pub(crate) fn stream_target(conn: &Connection, t: &Tuning, cap: Option<u32>) -> usize {
    let path = conn_path(conn);
    let rtt = conn_rtt_ms(conn);
    target_for(path, rtt, t, cap)
}

pub(crate) fn target_for(
    path: ConnPath,
    rtt_ms: Option<u32>,
    t: &Tuning,
    cap: Option<u32>,
) -> usize {
    let high = path == ConnPath::Relay || rtt_ms.is_some_and(|r| r > HIGH_RTT_MS);
    let n = if high {
        t.streams_high_rtt.max(t.streams)
    } else {
        t.streams
    };
    let n = match cap {
        Some(c) if c > 0 => n.min(c),
        _ => n,
    };
    n.max(1) as usize
}

/// The host's `max_parallel_streams` as a cap on the tuning. `4` was the v0.3
/// default and is written into every saved `HostConfig`, so it is treated as
/// "not set" (the tuning decides); `0` too. Any other value caps the streams.
pub(crate) fn host_stream_cap(max_parallel_streams: u32) -> Option<u32> {
    match max_parallel_streams {
        0 | 4 => None,
        n => Some(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        let t = Tuning::default();
        assert_eq!(target_for(ConnPath::Direct, Some(1), &t, None), 8);
        assert_eq!(target_for(ConnPath::Direct, Some(80), &t, None), 16);
        assert_eq!(target_for(ConnPath::Relay, Some(5), &t, None), 16);
        assert_eq!(target_for(ConnPath::Relay, None, &t, Some(6)), 6);
        assert_eq!(host_stream_cap(4), None);
        assert_eq!(host_stream_cap(2), Some(2));
        let _ = transport_config(&t);
        let _ = transport_config(&Tuning {
            congestion: Congestion::Cubic,
            stream_window_mb: 0,
            ..t
        });
    }
}
