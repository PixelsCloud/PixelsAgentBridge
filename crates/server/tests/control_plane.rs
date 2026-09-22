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
                team.tenant_id,
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

    let authenticated_device = control
        .authenticate_registered_endpoint(prove_endpoint_principal(
            deployment_id,
            EndpointProofPrincipal::Device {
                device_id: device.id,
            },
            team.tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &device_key,
        ))
        .await
        .unwrap();
    assert_eq!(authenticated_device.tenant_id, team.tenant_id);
    assert_eq!(
        authenticated_device.principal,
        EndpointProofPrincipal::Device {
            device_id: device.id,
        }
    );

    let device_ref = DeviceRef {
        deployment_id,
        tenant_id: team.tenant_id,
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
    assert!(matches!(
        control
            .set_device_connect_grant(bob.id, team.tenant_id, device.id, bob.id, true)
            .await,
        Err(ServiceError::Store(StoreError::PermissionDenied))
    ));
    control
        .set_device_connect_grant(alice.id, team.tenant_id, device.id, bob.id, true)
        .await
        .unwrap();
    assert_eq!(
        control
            .device_network_snapshot(&bob_endpoint, device_ref)
            .await
            .unwrap()
            .device_ref,
        device_ref
    );
    control
        .set_device_connect_grant(alice.id, team.tenant_id, device.id, bob.id, false)
        .await
        .unwrap();
    assert!(matches!(
        control
            .device_network_snapshot(&bob_endpoint, device_ref)
            .await,
        Err(ServiceError::Store(StoreError::NotFound))
    ));

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
                team.tenant_id,
                EndpointProofPurpose::AuthenticateRegisteredEndpoint,
                &device_key,
            ))
            .await,
        Err(ServiceError::Store(StoreError::NotFound))
    ));
}
