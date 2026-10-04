use std::{collections::HashMap, sync::Mutex};

use pab_protocol::EndpointKey;

#[derive(Debug, Default)]
pub(crate) struct ActiveEndpoints {
    connections: Mutex<HashMap<EndpointKey, usize>>,
}

impl ActiveEndpoints {
    pub fn keys(&self) -> Vec<Vec<u8>> {
        self.connections
            .lock()
            .expect("active endpoint lock")
            .keys()
            .map(|key| key.as_bytes().to_vec())
            .collect()
    }
    pub fn connected(&self, endpoint_key: EndpointKey) -> bool {
        let mut connections = self.connections.lock().expect("active endpoint lock");
        let count = connections.entry(endpoint_key).or_default();
        *count += 1;
        *count == 1
    }

    pub fn disconnected(&self, endpoint_key: EndpointKey) -> bool {
        let mut connections = self.connections.lock().expect("active endpoint lock");
        if let Some(count) = connections.get_mut(&endpoint_key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                connections.remove(&endpoint_key);
                return true;
            }
        }
        false
    }

    pub fn is_connected(&self, endpoint_key: EndpointKey) -> bool {
        self.connections
            .lock()
            .expect("active endpoint lock")
            .contains_key(&endpoint_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presence_lasts_until_the_last_connection_closes() {
        let endpoints = ActiveEndpoints::default();
        let key = EndpointKey::new([7; 32]);
        assert!(endpoints.connected(key));
        assert!(!endpoints.connected(key));
        assert_eq!(endpoints.keys().len(), 1);
        assert!(!endpoints.disconnected(key));
        assert!(endpoints.is_connected(key));
        assert!(endpoints.disconnected(key));
        assert!(!endpoints.is_connected(key));
        assert!(!endpoints.disconnected(key));
        assert!(endpoints.keys().is_empty());
    }
}
