use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use iroh_relay::server::{ForwardingControl, ForwardingDecision};
use pab_protocol::{
    DeviceId, EndpointKey, RELAY_POLICY_SCHEMA_VERSION, RelayConnectionIntent, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot, TeamRelayLimits, TenantId,
    TrafficScope, UserId,
};

use crate::{RelayPolicyRuntime, RelayPolicyState};

#[test]
fn forwarding_hook_uses_current_endpoint_scope_and_limits() {
    let tenant_id = TenantId::new();
    let user_id = UserId::new();
    let user_key = SecretKey::generate();
    let device_key = SecretKey::generate();
    let device_id = DeviceId::new();
    let unknown_key = SecretKey::generate();
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let snapshot = RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        policy_version: 1,
        issued_at_unix_ms: now - 1_000,
        expires_at_unix_ms: now + 60_000,
        defaults: RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        },
        team_limits: Vec::new(),
        endpoints: vec![
            RelayEndpointPolicy {
                endpoint_key: EndpointKey::new(*user_key.public().as_bytes()),
                owner: RelayEndpointOwner::User {
                    scope: TrafficScope::Personal { tenant_id, user_id },
                },
            },
            RelayEndpointPolicy {
                endpoint_key: EndpointKey::new(*device_key.public().as_bytes()),
                owner: RelayEndpointOwner::Device {
                    tenant_id,
                    device_id,
                },
            },
        ],
        connection_intents: vec![RelayConnectionIntent {
            operator_endpoint_key: EndpointKey::new(*user_key.public().as_bytes()),
            device_id,
            expires_at_unix_ms: now + 60_000,
        }],
    };
    let runtime =
        RelayPolicyRuntime::new(RelayPolicyState::new(Duration::from_millis(100)).unwrap());
    runtime.apply_snapshot(snapshot).unwrap();

    assert_eq!(
        ForwardingControl::check(&runtime, user_key.public(), device_key.public(), 10_000),
        ForwardingDecision::Allow
    );
    assert_eq!(
        ForwardingControl::check(&runtime, unknown_key.public(), device_key.public(), 1),
        ForwardingDecision::Drop
    );
}

#[test]
fn forwarding_hook_shares_team_budget_across_members() {
    let tenant_id = TenantId::new();
    let device_id = DeviceId::new();
    let device_key = SecretKey::generate();
    let members = [
        SecretKey::generate(),
        SecretKey::generate(),
        SecretKey::generate(),
    ];
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut endpoints = vec![RelayEndpointPolicy {
        endpoint_key: EndpointKey::new(*device_key.public().as_bytes()),
        owner: RelayEndpointOwner::Device {
            tenant_id,
            device_id,
        },
    }];
    let mut connection_intents = Vec::new();
    for member in &members {
        let endpoint_key = EndpointKey::new(*member.public().as_bytes());
        endpoints.push(RelayEndpointPolicy {
            endpoint_key,
            owner: RelayEndpointOwner::User {
                scope: TrafficScope::Team {
                    tenant_id,
                    user_id: UserId::new(),
                },
            },
        });
        connection_intents.push(RelayConnectionIntent {
            operator_endpoint_key: endpoint_key,
            device_id,
            expires_at_unix_ms: now + 60_000,
        });
    }
    let snapshot = RelayPolicySnapshot {
        schema_version: RELAY_POLICY_SCHEMA_VERSION,
        policy_version: 1,
        issued_at_unix_ms: now - 1_000,
        expires_at_unix_ms: now + 60_000,
        defaults: RelayLimitDefaults {
            team_mbps: 20,
            member_mbps: 4,
            personal_mbps: 5,
        },
        team_limits: vec![TeamRelayLimits {
            tenant_id,
            total_mbps: 2,
            member_mbps: 1,
        }],
        endpoints,
        connection_intents,
    };
    let runtime =
        RelayPolicyRuntime::new(RelayPolicyState::new(Duration::from_millis(100)).unwrap());
    runtime.apply_snapshot(snapshot).unwrap();

    assert_eq!(
        ForwardingControl::check(&runtime, members[0].public(), device_key.public(), 12_000),
        ForwardingDecision::Allow
    );
    assert!(matches!(
        ForwardingControl::check(&runtime, members[0].public(), device_key.public(), 1_000),
        ForwardingDecision::Wait(_)
    ));
    assert_eq!(
        ForwardingControl::check(&runtime, members[1].public(), device_key.public(), 12_000),
        ForwardingDecision::Allow
    );
    assert!(matches!(
        ForwardingControl::check(&runtime, members[2].public(), device_key.public(), 2_000),
        ForwardingDecision::Wait(_)
    ));
}
