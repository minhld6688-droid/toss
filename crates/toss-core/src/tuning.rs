//! QUIC transport tuning (WS-D). Scaffold: returns iroh's defaults.
//! Contract: `contracts/speed-v0.4.md` §4.

use iroh::endpoint::QuicTransportConfig;

use crate::net::Tuning;

/// Transport config for the endpoint (windows, congestion controller, stream limits).
pub(crate) fn transport_config(_t: &Tuning) -> QuicTransportConfig {
    QuicTransportConfig::default()
}
