use std::time::{Duration, Instant};

use pab_protocol::{
    DeviceId, EndpointKey, RELAY_POLICY_SCHEMA_VERSION, RelayConnectionIntent, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot, TenantId, TrafficScope, UserId,
    UserRelayLimit,
};

use crate::{PolicyStateError, RelayPolicyState};

fn snapshot(version: u64, endpoints: Vec<RelayEndpointPolicy>) -> RelayPolicySnapshot {
    RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        policy_version: version,
        issued_at_unix_ms: 1_000,
        expires_at_unix_ms: 10_000,
        defaults: RelayLimitDefaults { user_mbps: 5 },
        user_limits: vec![UserRelayLimit {
            user_id: UserId::from_u128(2),
            mbps: 4,
        }],
        endpoints,
        connection_intents: Vec::new(),
    }
}

#[test]
fn newer_snapshot_revokes_missing_endpoints_and_preserves_scope() {
    let user_id = UserId::from_u128(2);
    let user_key = EndpointKey::new([1; 32]);
    let device_key = EndpointKey::new([2; 32]);
    let device_id = DeviceId::from_u128(3);
    let user = RelayEndpointPolicy {
        endpoint_key: user_key,
        owner: RelayEndpointOwner::User {
            scope: TrafficScope::User { user_id },
        },
    };
    let device = RelayEndpointPolicy {
        endpoint_key: device_key,
        owner: RelayEndpointOwner::Device {
            tenant_id: TenantId::from_u128(4),
            device_id,
        },
    };
    let now = Instant::now();
    let mut state = RelayPolicyState::new(Duration::from_millis(100)).unwrap();
    let mut initial = snapshot(1, vec![user, device]);
    initial.connection_intents.push(RelayConnectionIntent {
        operator_endpoint_key: user_key,
        device_id,
        expires_at_unix_ms: 5_000,
    });
    state.apply_snapshot(initial, 2_000, now).unwrap();
    assert_eq!(
        state.traffic_scope(user_key, device_key, 2_000),
        Some(TrafficScope::User { user_id })
    );
    assert_eq!(state.traffic_scope(user_key, device_key, 5_000), None);

    state
        .apply_snapshot(snapshot(2, vec![device]), 3_000, now)
        .unwrap();
    assert_eq!(state.endpoint_owner(user_key, 3_000), None);
    assert_eq!(state.traffic_scope(user_key, device_key, 3_000), None);
    assert_eq!(state.endpoint_owner(device_key, 10_000), None);
}

#[test]
fn unchanged_refresh_extends_only_the_matching_policy() {
    let now = Instant::now();
    let mut state = RelayPolicyState::new(Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot(7, Vec::new()), 2_000, now)
        .unwrap();
    state.refresh_expiry(7, 20_000, 3_000).unwrap();
    assert!(state.is_current(15_000));
    assert_eq!(
        state.refresh_expiry(6, 30_000, 3_000),
        Err(PolicyStateError::UnexpectedPolicyVersion)
    );
}

#[test]
fn logout_and_login_do_not_reset_user_burst() {
    let user_id = UserId::from_u128(2);
    let endpoint_key = EndpointKey::new([24; 32]);
    let user = RelayEndpointPolicy {
        endpoint_key,
        owner: RelayEndpointOwner::User {
            scope: TrafficScope::User { user_id },
        },
    };
    let guest = RelayEndpointPolicy {
        endpoint_key,
        owner: RelayEndpointOwner::Guest,
    };
    let now = Instant::now();
    let mut state = RelayPolicyState::new(Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot(1, vec![user]), 2_000, now)
        .unwrap();
    assert_eq!(
        state.acquire(TrafficScope::User { user_id }, 50_000, now),
        crate::Acquire::Ready
    );
    state
        .apply_snapshot(snapshot(2, vec![guest]), 2_001, now)
        .unwrap();
    state
        .apply_snapshot(snapshot(3, vec![user]), 2_002, now)
        .unwrap();
    assert!(matches!(
        state.acquire(TrafficScope::User { user_id }, 1, now),
        crate::Acquire::Wait(_)
    ));
}

#[test]
fn anonymous_relay_is_denied_even_with_an_unexpired_grant() {
    let guest_key = EndpointKey::new([10; 32]);
    let first_key = EndpointKey::new([11; 32]);
    let second_key = EndpointKey::new([12; 32]);
    let first_id = DeviceId::from_u128(11);
    let endpoints = vec![
        RelayEndpointPolicy {
            endpoint_key: guest_key,
            owner: RelayEndpointOwner::Guest,
        },
        RelayEndpointPolicy {
            endpoint_key: first_key,
            owner: RelayEndpointOwner::Device {
                tenant_id: TenantId::from_u128(11),
                device_id: first_id,
            },
        },
        RelayEndpointPolicy {
            endpoint_key: second_key,
            owner: RelayEndpointOwner::Device {
                tenant_id: TenantId::from_u128(12),
                device_id: DeviceId::from_u128(12),
            },
        },
    ];
    let mut snapshot = snapshot(1, endpoints);
    snapshot.connection_intents.push(RelayConnectionIntent {
        operator_endpoint_key: guest_key,
        device_id: first_id,
        expires_at_unix_ms: 5_000,
    });
    let mut state = RelayPolicyState::new(Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot, 2_000, Instant::now())
        .unwrap();
    assert_eq!(state.traffic_scope(guest_key, first_key, 2_000), None);
    assert_eq!(state.endpoint_owner(guest_key, 2_000), None);
    assert!(state.endpoint_owner(first_key, 2_000).is_some());
    assert_eq!(state.traffic_scope(guest_key, second_key, 2_000), None);
    assert_eq!(state.traffic_scope(guest_key, first_key, 5_000), None);
}
