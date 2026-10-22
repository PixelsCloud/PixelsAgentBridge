//! Admin-only DNS display, distinct from an observed machine/egress IP.
use std::{collections::HashMap, net::IpAddr, time::{Duration, Instant}};
use tokio::sync::Mutex;

#[derive(Default)]
pub(super) struct RelayAddressCache {
    entries: Mutex<HashMap<String, (Instant, Vec<String>)>>,
}

impl RelayAddressCache {
    pub(super) async fn resolve(&self, value: &str) -> Vec<String> {
        let Ok(url) = url::Url::parse(value) else { return Vec::new(); };
        let Some(host) = url.host() else { return Vec::new(); };
        let hostname = match host {
            url::Host::Ipv4(ip) => return vec![ip.to_string()],
            url::Host::Ipv6(ip) => return vec![ip.to_string()],
            url::Host::Domain(name) => name.to_owned(),
        };
        {
            let cache = self.entries.lock().await;
            if let Some((expires, ips)) = cache.get(&hostname) {
                if *expires > Instant::now() { return ips.clone(); }
            }
        }
        let mut ips = match tokio::time::timeout(Duration::from_millis(800), tokio::net::lookup_host((hostname.as_str(), url.port_or_known_default().unwrap_or(443)))).await {
            Ok(Ok(addresses)) => addresses.map(|address| address.ip()).collect::<Vec<IpAddr>>(),
            _ => Vec::new(),
        };
        ips.sort_unstable();
        ips.dedup();
        let ips: Vec<String> = ips.into_iter().map(|ip| ip.to_string()).collect();
        let mut cache = self.entries.lock().await;
        cache.retain(|_, (expires, _)| *expires > Instant::now());
        if cache.len() >= 256 { cache.clear(); }
        cache.insert(hostname, (Instant::now() + Duration::from_secs(if ips.is_empty() { 10 } else { 60 }), ips.clone()));
        ips
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn literal_addresses_are_not_resolved_as_dns_names() {
        let cache = RelayAddressCache::default();
        assert_eq!(cache.resolve("https://192.0.2.10:443/").await, ["192.0.2.10"]);
        assert_eq!(cache.resolve("https://[2001:db8::1]/").await, ["2001:db8::1"]);
        assert!(cache.resolve("invalid").await.is_empty());
    }
}
