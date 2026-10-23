use pab_protocol::{RelayAdminState, RelayNodeControl, RelayNodeControlAck};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct NodeControl {
    node: String,
    boot: uuid::Uuid,
    applied: Option<(RelayNodeControl, Instant, Option<Instant>)>,
}

impl NodeControl {
    pub fn new(node: String, boot: uuid::Uuid) -> Self {
        Self { node, boot, applied: None }
    }

    pub fn apply(&mut self, snapshot: RelayNodeControl, started: Instant, now: Instant) -> Result<(), &'static str> {
        let validity = snapshot.expires_at_unix_ms.saturating_sub(snapshot.issued_at_unix_ms);
        if snapshot.node_id != self.node || snapshot.boot_id != self.boot || snapshot.revision == 0
            || !(1..=30_000).contains(&validity)
            || (snapshot.state == RelayAdminState::Maintenance) != snapshot.drain_at_unix_ms.is_some()
        { return Err("invalid node control snapshot"); }
        if let Some((old, _, _)) = &self.applied {
            if snapshot.revision < old.revision { return Err("stale node control revision"); }
            if snapshot.revision == old.revision && (snapshot.state != old.state || snapshot.operation_id != old.operation_id || snapshot.drain_at_unix_ms != old.drain_at_unix_ms) {
                return Err("node control changed without a revision");
            }
        }
        let already_drained = self.applied.as_ref().is_some_and(|(old,_,_)| old.state==RelayAdminState::Maintenance && snapshot.state==RelayAdminState::Maintenance && old.operation_id==snapshot.operation_id && !self.flags(now).1);
        let elapsed = now.saturating_duration_since(started);
        let remaining = Duration::from_millis(validity as u64).checked_sub(elapsed).filter(|v| !v.is_zero()).ok_or("node control lease expired in transit")?;
        let drain = snapshot.drain_at_unix_ms.map(|deadline| {
            let remaining = Duration::from_millis(deadline.saturating_sub(snapshot.issued_at_unix_ms).max(0) as u64).saturating_sub(elapsed);
            if already_drained { now } else {
                let candidate=now+remaining.min(Duration::from_secs(300));
                self.applied.as_ref().filter(|(old,_,_)|old.state==RelayAdminState::Maintenance && old.operation_id==snapshot.operation_id).and_then(|(_,_,drain)|*drain).map_or(candidate,|previous|previous.min(candidate))
            }
        });
        self.applied = Some((snapshot, now + remaining, drain));
        Ok(())
    }

    pub fn flags(&self, now: Instant) -> (bool, bool) {
        let Some((snapshot, expires, drain)) = &self.applied else { return (false, false); };
        if now >= *expires { return (false, false); }
        match snapshot.state {
            RelayAdminState::Active => (true, true),
            RelayAdminState::Maintenance => (false, drain.is_some_and(|at| now < at)),
            RelayAdminState::Disabled => (false, false),
        }
    }

    pub fn ack(&self, now: Instant) -> Option<RelayNodeControlAck> {
        let snapshot = &self.applied.as_ref()?.0;
        let (accepting, forwarding) = self.flags(now);
        Some(RelayNodeControlAck { operation_id: snapshot.operation_id, state: snapshot.state, accepting, forwarding })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(state: RelayAdminState) -> RelayNodeControl {
        RelayNodeControl { node_id:"test".into(),boot_id:uuid::Uuid::nil(),revision:1,operation_id:None,state,issued_at_unix_ms:1000,expires_at_unix_ms:16000,drain_at_unix_ms:(state==RelayAdminState::Maintenance).then_some(6000) }
    }
    #[test]
    fn admission_and_forwarding_follow_state_and_monotonic_deadlines() {
        let now=Instant::now();let mut control=NodeControl::new("test".into(),uuid::Uuid::nil());
        assert_eq!(control.flags(now),(false,false));
        control.apply(sample(RelayAdminState::Active),now,now).unwrap();
        assert_eq!(control.flags(now),(true,true));
        assert_eq!(control.flags(now+Duration::from_secs(15)),(false,false));
        let mut maintenance=sample(RelayAdminState::Maintenance);maintenance.revision=2;
        control.apply(maintenance,now,now+Duration::from_secs(1)).unwrap();
        assert_eq!(control.flags(now+Duration::from_secs(4)),(false,true));
        assert_eq!(control.flags(now+Duration::from_secs(5)),(false,false));
        assert!(control.apply(sample(RelayAdminState::Active),now,now).is_err());
        let mut disabled=sample(RelayAdminState::Disabled);disabled.revision=3;
        control.apply(disabled,now,now).unwrap();assert_eq!(control.flags(now),(false,false));
        let mut resumed=sample(RelayAdminState::Active);resumed.revision=4;
        control.apply(resumed,now,now).unwrap();assert_eq!(control.flags(now),(true,true));
    }
    #[test]
    fn renewed_maintenance_cannot_reopen_a_drained_node() {
        let now=Instant::now();let mut control=NodeControl::new("test".into(),uuid::Uuid::nil());
        let mut snapshot=sample(RelayAdminState::Maintenance);snapshot.drain_at_unix_ms=Some(61000);
        control.apply(snapshot.clone(),now,now).unwrap();
        assert_eq!(control.flags(now+Duration::from_secs(16)),(false,false));
        snapshot.revision=2;snapshot.issued_at_unix_ms=17000;snapshot.expires_at_unix_ms=32000;
        control.apply(snapshot,now+Duration::from_secs(16),now+Duration::from_secs(16)).unwrap();
        assert_eq!(control.flags(now+Duration::from_secs(16)),(false,false));
    }
    #[test]
    fn snapshots_cannot_change_identity_or_extend_lifetime_by_network_delay() {
        let now=Instant::now();let mut control=NodeControl::new("test".into(),uuid::Uuid::nil());
        assert!(control.apply(sample(RelayAdminState::Active),now,now+Duration::from_secs(15)).is_err());
        let mut other=sample(RelayAdminState::Active);other.boot_id=uuid::Uuid::new_v4();
        assert!(control.apply(other,now,now).is_err());
        control.apply(sample(RelayAdminState::Active),now,now).unwrap();
        let mut reused=sample(RelayAdminState::Disabled);reused.operation_id=Some(uuid::Uuid::new_v4());
        assert!(control.apply(reused,now,now).is_err());
    }
}
