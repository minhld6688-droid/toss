//! Same-LAN discovery over mDNS (WS-A, `contracts/speed-v0.4.md` §2 layer 0).
//!
//! Every node with `NetConfig::mdns` advertises its endpoint id + direct
//! addresses under the `_toss._udp.local` service (via
//! [`iroh_mdns_address_lookup`], i.e. `swarm-discovery`) and resolves peers
//! the same way. It is registered as an iroh address lookup service, so iroh
//! uses it whenever it looks a peer up (no path selected yet); Toss also asks
//! it explicitly when re-punching a connection that is stuck on the relay.
//! A network without multicast only loses this layer: start never fails.

use std::time::Duration;

use iroh::{address_lookup::AddressLookup, Endpoint, EndpointAddr, EndpointId};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use n0_future::StreamExt;

/// mDNS service name (`<id>._toss._udp.local`): Toss nodes only see each other,
/// not every iroh app on the LAN.
pub(crate) const SERVICE_NAME: &str = "toss";

/// Starts mDNS advertising + lookup and registers it with the endpoint.
/// Errors (no IPv4/IPv6 multicast, endpoint closed) are returned as text for
/// diagnostics; the caller logs and continues without mDNS.
pub(crate) fn start(endpoint: &Endpoint) -> Result<MdnsAddressLookup, String> {
    let mdns = MdnsAddressLookup::builder()
        .service_name(SERVICE_NAME)
        .build(endpoint.id())
        .map_err(|e| format!("{e:#}"))?;
    endpoint
        .address_lookup()
        .map_err(|e| format!("{e:#}"))?
        .add(mdns.clone());
    Ok(mdns)
}

/// What mDNS knows about `id`, waiting at most `wait` for an answer.
pub(crate) async fn resolve(
    mdns: &MdnsAddressLookup,
    id: EndpointId,
    wait: Duration,
) -> Option<EndpointAddr> {
    let mut stream = mdns.resolve(id)?;
    match tokio::time::timeout(wait, stream.next()).await {
        Ok(Some(Ok(item))) => Some(item.into_endpoint_addr()),
        _ => None,
    }
}
