use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh_base::SecretKey;
use iroh_relay::server::{ForwardingControl, ForwardingDecision};
use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, RELAY_POLICY_SCHEMA_VERSION, RelayEndpointOwner,
    RelayEndpointPolicy, RelayLimitDefaults, RelayPolicySnapshot, TenantId, TrafficScope, UserId,
};

use crate::{RelayPolicyRuntime, RelayPolicyState};

#[test]
fn forwarding_hook_uses_current_endpoint_scope_and_limits() {
    let deployment_id = DeploymentId::new();
    let tenant_id = TenantId::new();
    let user_id = UserId::new();
    let user_key = SecretKey::generate();
    let device_key = SecretKey::generate();
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
        deployment_id,
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
                    device_id: DeviceId::new(),
                },
            },
        ],
        guest_grants: Vec::new(),
    };
    let runtime = RelayPolicyRuntime::new(
        RelayPolicyState::new(deployment_id, Duration::from_millis(100)).unwrap(),
    );
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
