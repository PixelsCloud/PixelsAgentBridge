//! Bounded, caller/device-bound execution references. No identity switching here.
//! The platform must refresh native account/session facts before resolve. Accepted
//! requests persist ResolvedExecution; replay reads that record before consulting
//! this registry so a new login can never silently retarget an old request.
use pab_protocol::{
    DeviceRef, ExecutionContextRef, ExecutionIdentity, ExecutionMode, ExecutionSelection,
    OperatorRef, RequestId,
};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedExecution {
    pub selection: ExecutionSelection,
    pub identity: ExecutionIdentity,
}

/// Distinct authenticated transports can share an endpoint key. Context cleanup
/// for one MCP connection must not invalidate another MCP's discovered contexts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionCaller {
    pub device: DeviceRef,
    pub actor: OperatorRef,
    pub connection: RequestId,
}

struct Entry {
    caller: ExecutionCaller,
    identity: ExecutionIdentity,
}

pub struct ExecutionContextRegistry {
    entries: HashMap<ExecutionContextRef, Entry>,
    capacity: usize,
}
impl Default for ExecutionContextRegistry {
    fn default() -> Self {
        Self::new(512)
    }
}
impl ExecutionContextRegistry {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
        }
    }

    pub fn register(
        &mut self,
        caller: ExecutionCaller,
        identity: ExecutionIdentity,
    ) -> Result<ExecutionContextRef, ExecutionResolveError> {
        identity
            .validate()
            .map_err(ExecutionResolveError::InvalidIdentity)?;
        if identity.mode == ExecutionMode::Service {
            return Err(ExecutionResolveError::ServiceNeedsNoReference);
        }
        if let Some((id, _)) = self
            .entries
            .iter()
            .find(|(_, v)| v.caller == caller && v.identity == identity)
        {
            return Ok(*id);
        }
        if self.entries.len() >= self.capacity {
            return Err(ExecutionResolveError::Capacity);
        }
        let id = ExecutionContextRef::new();
        self.entries.insert(id, Entry { caller, identity });
        Ok(id)
    }

    pub fn resolve(
        &self,
        caller: ExecutionCaller,
        selection: ExecutionSelection,
        observed: &ExecutionIdentity,
    ) -> Result<ResolvedExecution, ExecutionResolveError> {
        observed
            .validate()
            .map_err(ExecutionResolveError::InvalidIdentity)?;
        if observed.mode != selection.mode() {
            return Err(ExecutionResolveError::ModeMismatch);
        }
        if let Some(id) = selection.context_ref() {
            let entry = self
                .entries
                .get(&id)
                .ok_or(ExecutionResolveError::UnknownReference)?;
            if entry.caller != caller {
                return Err(ExecutionResolveError::WrongOwner);
            }
            if entry.identity.mode != selection.mode() {
                return Err(ExecutionResolveError::ModeMismatch);
            }
            if &entry.identity != observed {
                return Err(ExecutionResolveError::Stale);
            }
        }
        Ok(ResolvedExecution {
            selection,
            identity: observed.clone(),
        })
    }

    /// Retrieve native discovery facts only after checking the reference owner.
    /// Callers must refresh those facts and call resolve before accepting work.
    pub fn identity(
        &self,
        caller: ExecutionCaller,
        selection: ExecutionSelection,
    ) -> Result<&ExecutionIdentity, ExecutionResolveError> {
        let id = selection
            .context_ref()
            .ok_or(ExecutionResolveError::ServiceNeedsNoReference)?;
        let entry = self
            .entries
            .get(&id)
            .ok_or(ExecutionResolveError::UnknownReference)?;
        if entry.caller != caller {
            return Err(ExecutionResolveError::WrongOwner);
        }
        if entry.identity.mode != selection.mode() {
            return Err(ExecutionResolveError::ModeMismatch);
        }
        Ok(&entry.identity)
    }

    /// Explicit disconnect/resource cleanup; does not alter already accepted work.
    pub fn release_connection(&mut self, caller: ExecutionCaller) {
        self.entries.retain(|_, v| v.caller != caller);
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutionResolveError {
    #[error("invalid execution identity: {0}")]
    InvalidIdentity(&'static str),
    #[error("service execution does not use a context reference")]
    ServiceNeedsNoReference,
    #[error("execution context capacity reached")]
    Capacity,
    #[error("unknown execution context; query available contexts again")]
    UnknownReference,
    #[error("execution context belongs to a different device or caller")]
    WrongOwner,
    #[error("execution mode does not match the selected context")]
    ModeMismatch,
    #[error("execution account/session changed; query available contexts again")]
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pab_protocol::{DeviceId, EndpointKey, ExecutionEnvironmentSource, TenantId};
    fn device(id: u128) -> DeviceRef {
        DeviceRef {
            tenant_id: TenantId::from_u128(1),
            device_id: DeviceId::from_u128(id),
        }
    }
    fn actor(id: u8) -> OperatorRef {
        OperatorRef::guest(EndpointKey::new([id; 32]))
    }
    fn caller(device_id: u128, actor_id: u8, connection: u128) -> ExecutionCaller {
        ExecutionCaller {
            device: device(device_id),
            actor: actor(actor_id),
            connection: RequestId::from_u128(connection),
        }
    }
    fn identity() -> ExecutionIdentity {
        ExecutionIdentity {
            mode: ExecutionMode::User,
            account_id: "uid:501".into(),
            account_name: "alice".into(),
            home: "/home/alice".into(),
            primary_group: Some(20),
            session_id: Some("1".into()),
            logon_id: Some("login-1".into()),
            environment_source: ExecutionEnvironmentSource::NativeAccount,
        }
    }
    #[test]
    fn reference_is_bound_to_device_and_caller_not_just_account() {
        let mut r = ExecutionContextRegistry::default();
        let observed = identity();
        let id = r.register(caller(1, 1, 1), observed.clone()).unwrap();
        let selected = ExecutionSelection::User { context_ref: id };
        assert!(r.resolve(caller(1, 1, 1), selected, &observed).is_ok());
        assert_eq!(
            r.resolve(caller(2, 1, 1), selected, &observed),
            Err(ExecutionResolveError::WrongOwner)
        );
        assert_eq!(
            r.resolve(caller(1, 2, 1), selected, &observed),
            Err(ExecutionResolveError::WrongOwner)
        );
    }
    #[test]
    fn stale_login_and_mode_change_cannot_retarget_accepted_identity() {
        let mut r = ExecutionContextRegistry::default();
        let original = identity();
        let id = r.register(caller(1, 1, 1), original.clone()).unwrap();
        let selected = ExecutionSelection::User { context_ref: id };
        let frozen = r.resolve(caller(1, 1, 1), selected, &original).unwrap();
        let mut next = original.clone();
        next.logon_id = Some("login-2".into());
        assert_eq!(
            r.resolve(caller(1, 1, 1), selected, &next),
            Err(ExecutionResolveError::Stale)
        );
        assert_eq!(frozen.identity, original);
        assert_eq!(
            r.resolve(
                caller(1, 1, 1),
                ExecutionSelection::DesktopUser { context_ref: id },
                &original
            ),
            Err(ExecutionResolveError::ModeMismatch)
        );
        r.release_connection(caller(1, 1, 1));
        assert_eq!(
            r.resolve(caller(1, 1, 1), selected, &original),
            Err(ExecutionResolveError::UnknownReference)
        );
        assert_eq!(frozen.identity, original);
    }
    #[test]
    fn repeated_discovery_reuses_reference_and_capacity_never_evicts_silently() {
        let mut r = ExecutionContextRegistry::new(1);
        let id = r.register(caller(1, 1, 1), identity()).unwrap();
        assert_eq!(r.register(caller(1, 1, 1), identity()).unwrap(), id);
        assert_eq!(
            r.register(caller(1, 2, 1), identity()),
            Err(ExecutionResolveError::Capacity)
        );
    }
    #[test]
    fn same_account_two_mcp_connections_cannot_reuse_or_release_each_others_context() {
        let mut registry = ExecutionContextRegistry::default();
        let first = caller(1, 1, 1);
        let second = caller(1, 1, 2);
        let a = registry.register(first, identity()).unwrap();
        let b = registry.register(second, identity()).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            registry.resolve(
                second,
                ExecutionSelection::User { context_ref: a },
                &identity()
            ),
            Err(ExecutionResolveError::WrongOwner)
        );
        registry.release_connection(first);
        assert!(
            registry
                .resolve(
                    second,
                    ExecutionSelection::User { context_ref: b },
                    &identity()
                )
                .is_ok()
        );
    }
    #[test]
    fn missing_selection_is_explicit_service_not_active_desktop() {
        let r = ExecutionContextRegistry::default();
        let mut service = identity();
        service.mode = ExecutionMode::Service;
        service.environment_source = ExecutionEnvironmentSource::ServiceProcess;
        assert_eq!(
            r.resolve(caller(1, 1, 1), ExecutionSelection::default(), &service)
                .unwrap()
                .identity,
            service
        );
        assert_eq!(
            r.resolve(caller(1, 1, 1), ExecutionSelection::default(), &identity()),
            Err(ExecutionResolveError::ModeMismatch)
        );
    }
}
