//! This registry lives only inside the isolated worker. No native object needs
//! Send/Sync, and every reference is scoped to a server-generated connection ID.
use pab_protocol::RequestId;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const OWNER_LIMIT: usize = 512;
const TOTAL_LIMIT: usize = 4096;
const IDLE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiOwner {
    pub connection: RequestId,
    pub helper_instance: String,
}

pub struct RegisteredElement<E> {
    pub owner: UiOwner,
    pub window_ref: String,
    pub parent_ref: Option<String>,
    pub element: E,
    touched: Instant,
}

pub struct UiRegistry<E> {
    entries: HashMap<String, RegisteredElement<E>>,
    owner_limit: usize,
    total_limit: usize,
    ttl: Duration,
}
impl<E> Default for UiRegistry<E> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            owner_limit: OWNER_LIMIT,
            total_limit: TOTAL_LIMIT,
            ttl: IDLE_TTL,
        }
    }
}
impl<E> UiRegistry<E> {
    pub fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| now.saturating_duration_since(e.touched) < self.ttl);
    }
    pub fn release_owner(&mut self, owner: &UiOwner) {
        self.entries.retain(|_, e| &e.owner != owner);
    }
    pub fn release_window(&mut self, window_ref: &str) {
        self.entries.retain(|_, e| e.window_ref != window_ref);
    }
    pub fn get(
        &mut self,
        reference: &str,
        owner: &UiOwner,
        now: Instant,
    ) -> Result<&RegisteredElement<E>, &'static str> {
        self.prune(now);
        let entry = self
            .entries
            .get_mut(reference)
            .filter(|e| &e.owner == owner)
            .ok_or("stale_element")?;
        entry.touched = now;
        Ok(entry)
    }
    /// Reuse only native identity equality; never reuse by title, role or path.
    pub fn insert(
        &mut self,
        owner: UiOwner,
        window_ref: String,
        parent_ref: Option<String>,
        element: E,
        now: Instant,
        equal: impl Fn(&E, &E) -> bool,
    ) -> Result<String, &'static str> {
        self.prune(now);
        if let Some((reference, entry)) = self.entries.iter_mut().find(|(_, e)| {
            e.owner == owner && e.window_ref == window_ref && equal(&e.element, &element)
        }) {
            entry.touched = now;
            entry.parent_ref = parent_ref;
            return Ok(reference.clone());
        }
        if self.entries.len() >= self.total_limit
            || self.entries.values().filter(|e| e.owner == owner).count() >= self.owner_limit
        {
            return Err("reference_budget");
        }
        let reference = RequestId::new().to_string();
        self.entries.insert(
            reference.clone(),
            RegisteredElement {
                owner,
                window_ref,
                parent_ref,
                element,
                touched: now,
            },
        );
        Ok(reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner() -> UiOwner {
        UiOwner {
            connection: RequestId::new(),
            helper_instance: "helper-a".into(),
        }
    }
    fn add(
        reg: &mut UiRegistry<u32>,
        owner: &UiOwner,
        id: u32,
        time: Instant,
    ) -> Result<String, &'static str> {
        reg.insert(owner.clone(), "window-a".into(), None, id, time, |a, b| {
            a == b
        })
    }
    #[test]
    fn separate_connections_never_share_references_even_for_same_native_object() {
        let mut reg = UiRegistry::default();
        let a = owner();
        let b = owner();
        let now = Instant::now();
        let ra = add(&mut reg, &a, 5, now).unwrap();
        let rb = add(&mut reg, &b, 5, now).unwrap();
        assert_ne!(ra, rb);
        assert!(reg.get(&ra, &b, now).is_err());
        assert!(reg.get(&rb, &a, now).is_err());
        assert_eq!(add(&mut reg, &a, 5, now).unwrap(), ra);
        let mut changed_helper = a.clone();
        changed_helper.helper_instance = "helper-b".into();
        assert!(reg.get(&ra, &changed_helper, now).is_err());
        reg.release_owner(&a);
        assert!(reg.get(&ra, &a, now).is_err());
        assert!(reg.get(&rb, &b, now).is_ok());
    }
    #[test]
    fn idle_expiry_is_not_renewed_by_wrong_owner_and_never_reuses_token() {
        let mut reg = UiRegistry::default();
        let a = owner();
        let now = Instant::now();
        let first = add(&mut reg, &a, 8, now).unwrap();
        assert!(reg.get(&first, &owner(), now + IDLE_TTL / 2).is_err());
        assert!(reg.get(&first, &a, now + IDLE_TTL).is_err());
        let second = add(&mut reg, &a, 8, now + IDLE_TTL).unwrap();
        assert_ne!(first, second);
        reg.release_window("window-a");
        assert!(reg.get(&second, &a, now + IDLE_TTL).is_err());
    }
    #[test]
    fn owner_and_global_limits_release_capacity_without_evicting_live_targets() {
        let mut reg = UiRegistry {
            owner_limit: 2,
            total_limit: 3,
            ..Default::default()
        };
        let a = owner();
        let b = owner();
        let now = Instant::now();
        let first = add(&mut reg, &a, 1, now).unwrap();
        add(&mut reg, &a, 2, now).unwrap();
        assert_eq!(add(&mut reg, &a, 3, now), Err("reference_budget"));
        assert_eq!(add(&mut reg, &a, 1, now).unwrap(), first);
        add(&mut reg, &b, 3, now).unwrap();
        assert_eq!(add(&mut reg, &b, 4, now), Err("reference_budget"));
        reg.release_owner(&a);
        assert!(add(&mut reg, &b, 4, now).is_ok());
    }
}
