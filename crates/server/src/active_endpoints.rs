use std::{collections::HashMap, sync::Mutex};

use pab_protocol::EndpointKey;

#[derive(Debug, Default)]
pub(crate) struct ActiveEndpoints {
    connections: Mutex<HashMap<EndpointKey, usize>>,
}

impl ActiveEndpoints {
    pub fn connected(&self, endpoint_key: EndpointKey) {
        let mut connections = self.connections.lock().expect("active endpoint lock");
        *connections.entry(endpoint_key).or_default() += 1;
    }

    pub fn disconnected(&self, endpoint_key: EndpointKey) {
        let mut connections = self.connections.lock().expect("active endpoint lock");
        if let Some(count) = connections.get_mut(&endpoint_key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                connections.remove(&endpoint_key);
            }
        }
    }

    pub fn is_connected(&self, endpoint_key: EndpointKey) -> bool {
        self.connections
            .lock()
            .expect("active endpoint lock")
            .contains_key(&endpoint_key)
    }
}
