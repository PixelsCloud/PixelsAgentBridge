use pab_protocol::{EndpointKey, RelayProbeGrant, RelayProbeSnapshot};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Active {
    grant: RelayProbeGrant,
    deadline: Instant,
    remaining: u64,
}
#[derive(Debug)]
pub(crate) struct RelayProbes {
    node: String,
    boot: uuid::Uuid,
    active: Option<Active>,
    history: VecDeque<(EndpointKey, EndpointKey)>,
}
impl RelayProbes {
    pub fn new(node: String, boot: uuid::Uuid) -> Self {
        Self {
            node,
            boot,
            active: None,
            history: VecDeque::new(),
        }
    }
    pub fn applied(&self) -> Option<uuid::Uuid> {
        self.active
            .as_ref()
            .filter(|p| p.deadline > Instant::now())
            .map(|p| p.grant.probe_id)
    }
    pub fn apply(
        &mut self,
        snapshot: RelayProbeSnapshot,
        started: Instant,
        now: Instant,
    ) -> Result<(), &'static str> {
        if snapshot.node_id != self.node || snapshot.boot_id != self.boot {
            return Err("probe process mismatch");
        }
        let Some(grant) = snapshot.grant else {
            self.active = None;
            return Ok(());
        };
        let validity = grant
            .expires_at_unix_ms
            .checked_sub(snapshot.issued_at_unix_ms)
            .ok_or("probe validity overflow")?;
        if grant.probe_id.is_nil()
            || grant.first == grant.second
            || grant.max_bytes == 0
            || grant.max_bytes > 65_536
            || validity > 10_000
        {
            return Err("invalid probe grant");
        }
        let deadline = started + Duration::from_millis(validity.saturating_sub(250).max(0) as u64);
        if let Some(current) = self
            .active
            .as_mut()
            .filter(|p| p.grant.probe_id == grant.probe_id)
        {
            if current.grant != grant {
                return Err("probe grant cannot change after acceptance");
            }
            current.deadline = current.deadline.min(deadline);
            return Ok(());
        }
        if deadline <= now {
            self.active = None;
            return Ok(());
        }
        self.history.push_back((grant.first, grant.second));
        while self.history.len() > 16 {
            self.history.pop_front();
        }
        self.active = Some(Active {
            remaining: grant.max_bytes,
            grant,
            deadline,
        });
        Ok(())
    }
    pub fn admits(&self, key: EndpointKey) -> bool {
        self.active.as_ref().is_some_and(|p| {
            p.deadline > Instant::now()
                && p.remaining > 0
                && (p.grant.first == key || p.grant.second == key)
        })
    }
    pub fn available(&self, first: EndpointKey, second: EndpointKey, bytes: u64) -> Option<bool> {
        self.active
            .as_ref()
            .filter(|p| pair((p.grant.first, p.grant.second), first, second))
            .map(|p| p.deadline > Instant::now() && p.remaining >= bytes)
    }
    pub fn consume(&mut self, bytes: u64) {
        if let Some(p) = &mut self.active {
            p.remaining = p.remaining.saturating_sub(bytes);
        }
    }
    pub fn was_probe(&self, first: EndpointKey, second: EndpointKey) -> bool {
        self.history.iter().any(|p| pair(*p, first, second))
    }
}
fn pair(pair: (EndpointKey, EndpointKey), a: EndpointKey, b: EndpointKey) -> bool {
    pair == (a, b) || pair == (b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe_is_exact_bounded_and_repeated_snapshots_do_not_reset_quota() {
        let boot = uuid::Uuid::new_v4();
        let now = Instant::now();
        let first = EndpointKey::new([1; 32]);
        let second = EndpointKey::new([2; 32]);
        let snapshot = RelayProbeSnapshot {
            node_id: "a".into(),
            boot_id: boot,
            issued_at_unix_ms: 1000,
            grant: Some(RelayProbeGrant {
                probe_id: uuid::Uuid::new_v4(),
                first,
                second,
                expires_at_unix_ms: 11000,
                max_bytes: 100,
            }),
        };
        let mut probes = RelayProbes::new("a".into(), boot);
        probes.apply(snapshot.clone(), now, now).unwrap();
        assert_eq!(probes.available(first, second, 100), Some(true));
        assert_eq!(probes.available(first, EndpointKey::new([3; 32]), 1), None);
        probes.consume(100);
        probes.apply(snapshot, now, now).unwrap();
        assert_eq!(probes.available(first, second, 1), Some(false));
        assert!(!probes.admits(first));
    }
}
