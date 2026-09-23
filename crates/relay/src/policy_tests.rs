use std::time::{Duration, Instant};

use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, GuestRelayGrant, RELAY_POLICY_SCHEMA_VERSION,
    RelayEndpointOwner, RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot,
    TeamRelayLimits, TenantId, TrafficScope, UserId,
};

use crate::{PolicyStateError, RelayPolicyState};

fn snapshot(
    deployment_id: DeploymentId,
    version: u64,
    endpoints: Vec<RelayEndpointPolicy>,
) -> RelayPolicySnapshot {
    RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        deployment_id,
        policy_version: version,
        issued_at_unix_ms: 1_000,
        expires_at_unix_ms: 10_000,
        defaults: RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        },
        team_limits: vec![TeamRelayLimits {
            tenant_id: TenantId::from_u128(1),
            total_mbps: 20,
            member_mbps: 4,
        }],
        endpoints,
        guest_grants: Vec::new(),
    }
}

#[test]
fn newer_snapshot_revokes_missing_endpoints_and_preserves_scope() {
    let deployment_id = DeploymentId::from_u128(9);
    let tenant_id = TenantId::from_u128(1);
    let user_id = UserId::from_u128(2);
    let user_key = EndpointKey::new([1; 32]);
    let device_key = EndpointKey::new([2; 32]);
    let user = RelayEndpointPolicy {
        endpoint_key: user_key,
        owner: RelayEndpointOwner::User {
            scope: TrafficScope::Team { tenant_id, user_id },
        },
    };
    let device = RelayEndpointPolicy {
        endpoint_key: device_key,
        owner: RelayEndpointOwner::Device {
            tenant_id,
            device_id: DeviceId::from_u128(3),
        },
    };
    let now = Instant::now();
    let mut state = RelayPolicyState::new(deployment_id, Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot(deployment_id, 1, vec![user, device]), 2_000, now)
        .unwrap();
    assert_eq!(
        state.traffic_scope(user_key, device_key, 2_000),
        Some(TrafficScope::Team { tenant_id, user_id })
    );

    state
        .apply_snapshot(snapshot(deployment_id, 2, vec![device]), 3_000, now)
        .unwrap();
    assert_eq!(state.endpoint_owner(user_key, 3_000), None);
    assert_eq!(state.traffic_scope(user_key, device_key, 3_000), None);
    assert_eq!(state.endpoint_owner(device_key, 10_000), None);
}

#[test]
fn unchanged_refresh_extends_only_the_matching_policy() {
    let deployment_id = DeploymentId::from_u128(9);
    let now = Instant::now();
    let mut state = RelayPolicyState::new(deployment_id, Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot(deployment_id, 7, Vec::new()), 2_000, now)
        .unwrap();
    state
        .refresh_expiry(deployment_id, 7, 20_000, 3_000)
        .unwrap();
    assert!(state.is_current(15_000));
    assert_eq!(
        state.refresh_expiry(deployment_id, 6, 30_000, 3_000),
        Err(PolicyStateError::UnexpectedPolicyVersion)
    );
}

#[test]
fn guest_relay_requires_an_unexpired_grant_for_that_device() {
    let deployment_id = DeploymentId::from_u128(9);
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
    let mut snapshot = snapshot(deployment_id, 1, endpoints);
    snapshot.guest_grants.push(GuestRelayGrant {
        guest_endpoint_key: guest_key,
        device_id: first_id,
        expires_at_unix_ms: 5_000,
    });
    let mut state = RelayPolicyState::new(deployment_id, Duration::from_millis(100)).unwrap();
    state
        .apply_snapshot(snapshot, 2_000, Instant::now())
        .unwrap();
    assert_eq!(
        state.traffic_scope(guest_key, first_key, 2_000),
        Some(TrafficScope::Guest {
            endpoint_key: guest_key
        })
    );
    assert_eq!(state.traffic_scope(guest_key, second_key, 2_000), None);
    assert_eq!(state.traffic_scope(guest_key, first_key, 5_000), None);
}
