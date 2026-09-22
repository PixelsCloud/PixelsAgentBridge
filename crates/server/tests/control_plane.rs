use std::time::Duration;

use pab_protocol::{
    DeploymentId, EndpointKey, RelayEndpointOwner, RelayLimitDefaults, TenantId, TrafficScope,
};
use pab_server::{ControlPlane, PasswordPolicy, PostgresStore, ServiceError, StoreError, TeamRole};
use sqlx::PgPool;
use time::OffsetDateTime;

#[sqlx::test(migrations = "./migrations")]
async fn account_team_device_and_policy_flow(pool: PgPool) {
    let store = PostgresStore::from_pool(pool);
    let control = ControlPlane::new(store, PasswordPolicy::default()).unwrap();
    let deployment_id = control
        .initialize_deployment(
            DeploymentId::from_u128(1),
            RelayLimitDefaults {
                team_mbps: 20,
                member_mbps: 4,
                personal_mbps: 5,
            },
        )
        .await
        .unwrap();

    let alice = control
        .register_account("Alice", "correct horse battery staple")
        .await
        .unwrap();
    let bob = control
        .register_account("Bob", "another correct battery staple")
        .await
        .unwrap();
    assert_eq!(
        control
            .authenticate("ＡLICE", "correct horse battery staple")
            .await
            .unwrap(),
        alice
    );
    assert!(matches!(
        control.authenticate("alice", "wrong password").await,
        Err(ServiceError::InvalidCredentials)
    ));

    let team = control.create_team(alice.id, "Engineering").await.unwrap();
    let invitation = control
        .invite_team_member(
            alice.id,
            team.tenant_id,
            bob.id,
            TeamRole::Member,
            OffsetDateTime::now_utc() + time::Duration::hours(1),
        )
        .await
        .unwrap();
    assert_eq!(
        control
            .accept_team_invitation(bob.id, invitation.id)
            .await
            .unwrap(),
        team.tenant_id
    );

    control
        .register_user_endpoint(alice.id, team.tenant_id, EndpointKey::new([1; 32]))
        .await
        .unwrap();
    control
        .register_user_endpoint(bob.id, team.tenant_id, EndpointKey::new([2; 32]))
        .await
        .unwrap();
    control
        .register_user_endpoint(
            alice.id,
            alice.personal_tenant_id,
            EndpointKey::new([3; 32]),
        )
        .await
        .unwrap();
    let device = control
        .register_device(
            alice.id,
            team.tenant_id,
            "build-worker",
            EndpointKey::new([4; 32]),
        )
        .await
        .unwrap();
    assert!(matches!(
        control
            .register_device(
                bob.id,
                team.tenant_id,
                "unauthorized-worker",
                EndpointKey::new([5; 32]),
            )
            .await,
        Err(ServiceError::Store(StoreError::PermissionDenied))
    ));

    let snapshot = control
        .relay_policy_snapshot(Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(snapshot.deployment_id, deployment_id);
    assert_eq!(snapshot.defaults.team_mbps, 20);
    assert_eq!(snapshot.team_limits.len(), 1);
    assert_eq!(snapshot.endpoints.len(), 4);
    assert!(snapshot.endpoints.iter().any(|endpoint| {
        endpoint.owner
            == RelayEndpointOwner::User {
                scope: TrafficScope::Team {
                    tenant_id: team.tenant_id,
                    user_id: bob.id,
                },
            }
    }));
    assert!(snapshot.endpoints.iter().any(|endpoint| {
        endpoint.owner
            == RelayEndpointOwner::Device {
                tenant_id: team.tenant_id,
                device_id: device.id,
            }
    }));
    assert!(
        snapshot
            .endpoints
            .iter()
            .all(|endpoint| endpoint.owner.tenant_id() != TenantId::from_u128(0))
    );
    snapshot.validate().unwrap();
}
