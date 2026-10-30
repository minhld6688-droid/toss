//! Same-LAN discovery over mDNS (WS-A, `contracts/speed-v0.4.md` §2 layer 0;
//! v0.8 nearby store, `contracts/nearby-v0.8.md` §1.1).
//!
//! Every node with `NetConfig::mdns` advertises its endpoint id + direct
//! addresses under the `_toss._udp.local` service (via
//! [`iroh_mdns_address_lookup`], i.e. `swarm-discovery`) and resolves peers
//! the same way. It is registered as an iroh address lookup service, so iroh
//! uses it whenever it looks a peer up (no path selected yet); Toss also asks
//! it explicitly when re-punching a connection that is stuck on the relay.
//! A network without multicast only loses this layer: start never fails.
//!
//! v0.8: the service is wrapped in an [`Advertiser`] that adds the mDNS user
//! data `n=<name>;p=<platform>;v=<version>` to what is published on the LAN
//! only (the endpoint-wide user data would also go to pkarr / n0 DNS), and
//! [`spawn_nearby`] keeps the node's nearby store from the lookup's events.

use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use iroh::{
    address_lookup::{AddressLookup, EndpointData, Item, UserData},
    Endpoint, EndpointAddr, EndpointId,
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::{boxed::BoxStream, StreamExt};
use parking_lot::Mutex;

use crate::{
    nearby::{parse_user_data, NearbyPeer},
    node::Inner,
};

/// mDNS service name (`<id>._toss._udp.local`): Toss nodes only see each other,
/// not every iroh app on the LAN.
pub(crate) const SERVICE_NAME: &str = "toss";

/// How often the nearby store is checked for stale entries.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
/// Entries not announced for this long are looked up again (the mDNS cache
/// answers at once while the peer has not expired).
const REFRESH_AFTER: Duration = Duration::from_secs(60);

/// The mDNS lookup plus this node's user data, published on the LAN only.
#[derive(Debug, Clone)]
pub(crate) struct Advertiser {
    inner: MdnsAddressLookup,
    /// (user data, last endpoint data published by iroh).
    state: Arc<Mutex<(Option<UserData>, Option<EndpointData>)>>,
}

impl Advertiser {
    /// Sets the advertised user data and re-publishes the last known
    /// addresses with it.
    pub fn set_user_data(&self, text: &str) {
        let ud = match text.parse::<UserData>() {
            Ok(u) => Some(u),
            Err(_) => {
                tracing::warn!("mDNS user data too long, not advertised");
                None
            }
        };
        let data = {
            let mut st = self.state.lock();
            st.0 = ud.clone();
            st.1.clone()
        };
        if let Some(mut d) = data {
            d.set_user_data(ud);
            self.inner.publish(&d);
        }
    }
}

impl AddressLookup for Advertiser {
    fn publish(&self, data: &EndpointData) {
        let mut d = data.clone();
        {
            let mut st = self.state.lock();
            st.1 = Some(data.clone());
            d.set_user_data(st.0.clone());
        }
        self.inner.publish(&d);
    }

    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<BoxStream<Result<Item, iroh::address_lookup::Error>>> {
        self.inner.resolve(endpoint_id)
    }
}

/// Starts mDNS advertising (with `user_data`) + lookup and registers it with
/// the endpoint. Errors (no IPv4/IPv6 multicast, endpoint closed) are returned
/// as text for diagnostics; the caller logs and continues without mDNS.
pub(crate) fn start(
    endpoint: &Endpoint,
    user_data: &str,
) -> Result<(MdnsAddressLookup, Advertiser), String> {
    let mdns = MdnsAddressLookup::builder()
        .service_name(SERVICE_NAME)
        .build(endpoint.id())
        .map_err(|e| format!("{e:#}"))?;
    let adv = Advertiser {
        inner: mdns.clone(),
        state: Arc::new(Mutex::new((user_data.parse().ok(), None))),
    };
    endpoint
        .address_lookup()
        .map_err(|e| format!("{e:#}"))?
        .add(adv.clone());
    Ok((mdns, adv))
}

/// What mDNS knows about `id`, waiting at most `wait` for an answer.
pub(crate) async fn resolve(
    mdns: &MdnsAddressLookup,
    id: EndpointId,
    wait: Duration,
) -> Option<EndpointAddr> {
    resolve_item(mdns, id, wait)
        .await
        .map(|i| i.into_endpoint_addr())
}

async fn resolve_item(mdns: &MdnsAddressLookup, id: EndpointId, wait: Duration) -> Option<Item> {
    let mut stream = mdns.resolve(id)?;
    match tokio::time::timeout(wait, stream.next()).await {
        Ok(Some(Ok(item))) => Some(item),
        _ => None,
    }
}

/// A nearby entry from an mDNS answer.
fn peer_of(id: EndpointId, data: &EndpointData) -> NearbyPeer {
    let (name, platform, version) = data
        .user_data()
        .map(|u| parse_user_data(u.as_ref()))
        .unwrap_or_default();
    let mut addrs: Vec<String> = data.ip_addrs().map(|a| a.to_string()).collect();
    addrs.sort();
    addrs.dedup();
    NearbyPeer {
        id: crate::peer::id_hex(id),
        name,
        platform,
        version,
        addrs,
        last_seen: 0,
        contact: false,
    }
}

/// Subscribes to the mDNS lookup and keeps `inner.nearby` up to date
/// (Discovered → upsert, Expired → remove, not seen for 3 min → drop).
pub(crate) async fn spawn_nearby(inner: &Arc<Inner>) {
    let Some(mdns) = inner.mdns.clone() else {
        return;
    };
    let mut events = mdns.subscribe().await;
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            let Some(inner) = weak.upgrade() else { return };
            match ev {
                DiscoveryEvent::Discovered { endpoint_info, .. } => {
                    let peer = peer_of(endpoint_info.endpoint_id, &endpoint_info.data);
                    tracing::debug!(id = %endpoint_info.endpoint_id.fmt_short(), name = %peer.name, "nearby: discovered");
                    crate::peer::nearby_seen(&inner, peer);
                }
                DiscoveryEvent::Expired { endpoint_id } => {
                    tracing::debug!(id = %endpoint_id.fmt_short(), "nearby: expired");
                    crate::peer::nearby_gone(&inner, &[crate::peer::id_hex(endpoint_id)]);
                }
                _ => {}
            }
        }
    });
    tokio::spawn(sweep_loop(Arc::downgrade(inner), mdns));
}

/// Refreshes entries that were not announced for a while (the lookup answers
/// from its cache while the peer has not expired) and drops stale ones.
async fn sweep_loop(weak: Weak<Inner>, mdns: MdnsAddressLookup) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        let Some(inner) = weak.upgrade() else { return };
        if inner.endpoint.is_closed() {
            return;
        }
        for hex in inner.nearby.older_than(REFRESH_AFTER) {
            let Ok(id) = crate::peer::parse_endpoint_id(&hex) else {
                continue;
            };
            if let Some(item) = resolve_item(&mdns, id, Duration::from_secs(1)).await {
                crate::peer::nearby_seen(&inner, peer_of(id, &item.endpoint_info().data));
            }
        }
        let gone = inner.nearby.sweep(Instant::now());
        if !gone.is_empty() {
            tracing::debug!(n = gone.len(), "nearby: dropped stale entries");
            crate::peer::nearby_gone(&inner, &gone);
            crate::peer::nearby_changed(&inner);
        }
    }
}
