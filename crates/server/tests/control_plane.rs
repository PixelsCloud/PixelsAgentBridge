use std::time::Duration;

use iroh_base::SecretKey;
use pab_protocol::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeploymentId, DeviceNetworkUpdate, DeviceRef,
    EndpointInstanceId, EndpointKey, EndpointProofChallenge, EndpointProofPrincipal,
    EndpointProofPurpose, EndpointProofResponse, EndpointSignature, RelayEndpointOwner,
    RelayLimitDefaults, TenantId, TrafficScope, UserId,
};
use pab_server::{ControlPlane, PasswordPolicy, PostgresStore, ServiceError, StoreError, TeamRole};
use sqlx::PgPool;
use time::OffsetDateTime;

fn prove_endpoint(
    deployment_id: DeploymentId,
    user_id: UserId,
    tenant_id: TenantId,
    purpose: EndpointProofPurpose,
    secret: &SecretKey,
) -> pab_server::VerifiedEndpointProof {
    prove_endpoint_principal(
        deployment_id,
        EndpointProofPrincipal::User { user_id },
        tenant_id,
        purpose,
        secret,
    )
}

fn prove_endpoint_principal(
    deployment_id: DeploymentId,
    principal: EndpointProofPrincipal,
    tenant_id: TenantId,
    purpose: EndpointProofPurpose,
    secret: &SecretKey,
) -> pab_server::VerifiedEndpointProof {
    let now = OffsetDateTime::now_utc();
    let mut session = pab_server::EndpointProofSession::new(deployment_id);
    let challenge = session
        .issue(
            principal,
            tenant_id,
            EndpointKey::new(*secret.public().as_bytes()),
            purpose,
            now,
            Duration::from_secs(30),
        )
        .unwrap();
    session
        .verify(signed_response(secret, &challenge), now)
        .unwrap()
}

fn signed_response(
    secret: &SecretKey,
    challenge: &EndpointProofChallenge,
) -> EndpointProofResponse {
    EndpointProofResponse {
        challenge_id: challenge.challenge_id,
        signature: EndpointSignature::from_bytes(
            secret.sign(&challenge.signing_message()).to_bytes(),
        ),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn team_limits_and_member_removal_are_audited_and_revoked(pool: PgPool) {
    let store = PostgresStore::from_pool(pool);
    let control = ControlPlane::new(store, PasswordPolicy::default()).unwrap();
    let deployment_id = control
        .initialize_deployment(
            DeploymentId::from_u128(90),
            RelayLimitDefaults {
                team_mbps: 20,
                member_mbps: 4,
                personal_mbps: 5,
            },
        )
        .await
        .unwrap();
    let owner = control
        .register_account("scope-owner", "correct horse battery staple")
        .await
        .unwrap();
    let member = control
        .register_account("scope-member", "another correct battery staple")
        .await
        .unwrap();
    let team = control
        .admin_create_team(owner.id, "Test Team", "integration-admin")
        .await
        .unwrap();
    control
        .admin_add_team_member(
            team.tenant_id,
            member.id,
            TeamRole::Member,
            "integration-admin",
        )
        .await
        .unwrap();
    let secret = SecretKey::generate();
    let endpoint_key = EndpointKey::new(*secret.public().as_bytes());
    control
        .register_user_endpoint(prove_endpoint(
            deployment_id,
            member.id,
            team.tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &secret,
        ))
        .await
        .unwrap();

    assert!(
        control
            .admin_set_team_limits(team.tenant_id, 2, 1, "integration-admin")
            .await
            .unwrap()
    );
    assert!(
        !control
            .admin_set_team_limits(team.tenant_id, 2, 1, "integration-admin")
            .await
            .unwrap()
    );
    assert!(
        control
            .admin_set_team_limits(team.tenant_id, 1, 2, "integration-admin")
            .await
            .is_err()
    );
    let options = control.list_traffic_scopes(&member).await.unwrap();
    assert_eq!(options.default_tenant_id, member.personal_tenant_id);
    assert_eq!(options.teams[0].total_mbps, 2);
    assert_eq!(options.teams[0].member_mbps, 1);
    assert!(
        control
            .admin_set_default_traffic_team(member.id, Some(team.tenant_id), "integration-admin",)
            .await
            .unwrap()
    );
    assert_eq!(
        control
            .list_traffic_scopes(&member)
            .await
            .unwrap()
            .default_tenant_id,
        team.tenant_id
    );
    let policy = control
        .relay_policy_snapshot(Duration::from_secs(60))
        .await
        .unwrap();
    let limits = policy
        .team_limits
        .iter()
        .find(|limits| limits.tenant_id == team.tenant_id)
        .unwrap();
    assert_eq!((limits.total_mbps, limits.member_mbps), (2, 1));

    assert!(
        control
            .admin_remove_team_member(team.tenant_id, member.id, "integration-admin")
            .await
            .unwrap()
    );
    assert!(
        !control
            .admin_remove_team_member(team.tenant_id, member.id, "integration-admin")
            .await
            .unwrap()
    );
    assert!(
        control
            .admin_remove_team_member(team.tenant_id, owner.id, "integration-admin")
            .await
            .is_err()
    );
    assert!(
        control
            .list_traffic_scopes(&member)
            .await
            .unwrap()
            .teams
            .is_empty()
    );
    assert_eq!(
        control
            .list_traffic_scopes(&member)
            .await
            .unwrap()
            .default_tenant_id,
        member.personal_tenant_id
    );
    assert!(control.registered_endpoint(endpoint_key).await.is_err());
    let policy = control
        .relay_policy_snapshot(Duration::from_secs(60))
        .await
        .unwrap();
    assert!(
        !policy
            .endpoints
            .iter()
            .any(|entry| entry.endpoint_key == endpoint_key)
    );

    assert!(
        control
            .admin_add_team_member(
                team.tenant_id,
                member.id,
                TeamRole::Member,
                "integration-admin"
            )
            .await
            .unwrap()
    );
    assert_eq!(
        control
            .list_traffic_scopes(&member)
            .await
            .unwrap()
            .teams
            .len(),
        1
    );
    assert!(control.registered_endpoint(endpoint_key).await.is_ok());

    let actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM team_admin_events WHERE tenant_id = $1 ORDER BY created_at, id",
    )
    .bind(team.tenant_id.as_uuid())
    .fetch_all(control.store().pool())
    .await
    .unwrap();
    assert_eq!(actions.len(), 5);
    assert!(actions.contains(&"limits_changed".to_owned()));
    assert!(actions.contains(&"member_removed".to_owned()));
    let old_and_new: (i32, i32, i32, i32) = sqlx::query_as(
        "SELECT old_total_mbps, old_member_mbps, new_total_mbps, new_member_mbps FROM team_admin_events WHERE tenant_id = $1 AND action = 'limits_changed'",
    )
    .bind(team.tenant_id.as_uuid())
    .fetch_one(control.store().pool())
    .await
    .unwrap();
    assert_eq!(old_and_new, (20, 4, 2, 1));
    let assignments: Vec<(Option<uuid::Uuid>, Option<uuid::Uuid>)> = sqlx::query_as(
        "SELECT old_team_id, new_team_id FROM account_traffic_assignment_events WHERE user_id = $1 ORDER BY created_at, id",
    )
    .bind(member.id.as_uuid())
    .fetch_all(control.store().pool())
    .await
    .unwrap();
    assert_eq!(assignments.len(), 2);
    assert!(assignments.contains(&(None, Some(team.tenant_id.as_uuid()))));
    assert!(assignments.contains(&(Some(team.tenant_id.as_uuid()), None)));
}

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

    let team = control
        .admin_create_team(alice.id, "Engineering", "integration-test-admin")
        .await
        .unwrap();
    assert!(
        control
            .admin_add_team_member(
                team.tenant_id,
                bob.id,
                TeamRole::Member,
                "integration-test-admin",
            )
            .await
            .unwrap()
    );
    assert!(
        !control
            .admin_add_team_member(
                team.tenant_id,
                bob.id,
                TeamRole::Member,
                "integration-test-admin",
            )
            .await
            .unwrap()
    );
    let admin_events =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM team_admin_events WHERE tenant_id = $1")
            .bind(team.tenant_id.as_uuid())
            .fetch_one(control.store().pool())
            .await
            .unwrap();
    assert_eq!(admin_events, 2);

    let alice_scopes = control.list_traffic_scopes(&alice).await.unwrap();
    assert_eq!(alice_scopes.personal_tenant_id, alice.personal_tenant_id);
    assert_eq!(alice_scopes.personal_mbps, 5);
    assert_eq!(alice_scopes.teams.len(), 1);
    assert_eq!(alice_scopes.teams[0].tenant_id, team.tenant_id);
    assert_eq!(alice_scopes.teams[0].total_mbps, 20);
    assert_eq!(alice_scopes.teams[0].member_mbps, 4);
    let bob_scopes = control.list_traffic_scopes(&bob).await.unwrap();
    assert_eq!(bob_scopes.teams, alice_scopes.teams);

    let alice_team_key = SecretKey::generate();
    let bob_team_key = SecretKey::generate();
    let alice_personal_key = SecretKey::generate();
    let device_key = SecretKey::generate();
    let unauthorized_device_key = SecretKey::generate();
    control
        .register_user_endpoint(prove_endpoint(
            deployment_id,
            alice.id,
            team.tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &alice_team_key,
        ))
        .await
        .unwrap();
    control
        .register_user_endpoint(prove_endpoint(
            deployment_id,
            bob.id,
            team.tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &bob_team_key,
        ))
        .await
        .unwrap();
    control
        .register_user_endpoint(prove_endpoint(
            deployment_id,
            alice.id,
            alice.personal_tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &alice_personal_key,
        ))
        .await
        .unwrap();
    let device = control
        .register_device(
            prove_endpoint(
                deployment_id,
                alice.id,
                alice.personal_tenant_id,
                EndpointProofPurpose::RegisterDevice,
                &device_key,
            ),
            "build-worker",
        )
        .await
        .unwrap();
    assert!(matches!(
        control
            .register_device(
                prove_endpoint(
                    deployment_id,
                    bob.id,
                    team.tenant_id,
                    EndpointProofPurpose::RegisterDevice,
                    &unauthorized_device_key,
                ),
                "unauthorized-worker",
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
                tenant_id: alice.personal_tenant_id,
                device_id: device.id,
            }
    }));
    assert!(
        snapshot
            .endpoints
            .iter()
            .all(|endpoint| endpoint.owner.tenant_id() != Some(TenantId::from_u128(0)))
    );
    snapshot.validate().unwrap();

    let authenticated_device = control
        .authenticate_registered_endpoint(prove_endpoint_principal(
            deployment_id,
            EndpointProofPrincipal::Device {
                device_id: device.id,
            },
            alice.personal_tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &device_key,
        ))
        .await
        .unwrap();
    assert_eq!(authenticated_device.tenant_id, alice.personal_tenant_id);
    assert_eq!(
        authenticated_device.principal,
        EndpointProofPrincipal::Device {
            device_id: device.id,
        }
    );

    let device_ref = DeviceRef {
        deployment_id,
        tenant_id: alice.personal_tenant_id,
        device_id: device.id,
    };
    control
        .publish_device_network(
            &authenticated_device,
            &DeviceNetworkUpdate {
                schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
                device_ref,
                endpoint_key: EndpointKey::new(*device_key.public().as_bytes()),
                endpoint_instance_id: EndpointInstanceId::new(),
                address_revision: 1,
                relay_urls: vec!["https://relay.example/".to_owned()],
                direct_addresses: vec!["192.0.2.10:7842".parse().unwrap()],
                observed_at_unix_ms: 1_795_000_000_000,
            },
        )
        .await
        .unwrap();
    let alice_endpoint = control
        .authenticate_registered_endpoint(prove_endpoint(
            deployment_id,
            alice.id,
            team.tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &alice_team_key,
        ))
        .await
        .unwrap();
    let bob_endpoint = control
        .authenticate_registered_endpoint(prove_endpoint(
            deployment_id,
            bob.id,
            team.tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &bob_team_key,
        ))
        .await
        .unwrap();
    let alice_devices = control
        .list_my_devices(&alice_endpoint, deployment_id)
        .await
        .unwrap();
    assert_eq!(alice_devices.len(), 1);
    assert_eq!(alice_devices[0].device_ref, device_ref);
    assert!(
        control
            .list_my_devices(&bob_endpoint, deployment_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(device.code.to_string().len(), 9);
    assert_eq!(
        control
            .resolve_device_code(&alice_endpoint, device.code, deployment_id)
            .await
            .unwrap(),
        device_ref
    );
    assert_eq!(
        control
            .device_network_snapshot(&alice_endpoint, device_ref)
            .await
            .unwrap()
            .endpoint_key,
        EndpointKey::new(*device_key.public().as_bytes())
    );
    assert!(matches!(
        control
            .device_network_snapshot(&bob_endpoint, device_ref)
            .await,
        Err(ServiceError::Store(StoreError::NotFound))
    ));
    assert_eq!(
        control
            .resolve_device_code(&bob_endpoint, device.code, deployment_id)
            .await
            .unwrap(),
        device_ref
    );
    assert_eq!(
        control
            .device_network_snapshot(&bob_endpoint, device_ref)
            .await
            .unwrap()
            .device_ref,
        device_ref
    );
    sqlx::query("DELETE FROM device_network WHERE device_id = $1")
        .bind(device.id.as_uuid())
        .execute(control.store().pool())
        .await
        .unwrap();
    assert_eq!(
        control
            .resolve_device_code(&alice_endpoint, device.code, deployment_id)
            .await
            .unwrap(),
        device_ref
    );

    sqlx::query("UPDATE devices SET status = 'disabled' WHERE id = $1")
        .bind(device.id.as_uuid())
        .execute(control.store().pool())
        .await
        .unwrap();
    assert!(matches!(
        control
            .authenticate_registered_endpoint(prove_endpoint_principal(
                deployment_id,
                EndpointProofPrincipal::Device {
                    device_id: device.id,
                },
                alice.personal_tenant_id,
                EndpointProofPurpose::AuthenticateRegisteredEndpoint,
                &device_key,
            ))
            .await,
        Err(ServiceError::Store(StoreError::NotFound))
    ));
}
