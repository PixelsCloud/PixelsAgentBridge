use std::time::Duration;

use iroh_base::SecretKey;
use pab_protocol::{
    DEVICE_NETWORK_SCHEMA_VERSION, DeviceNetworkUpdate, DeviceRef, EndpointInstanceId, EndpointKey,
    EndpointProofChallenge, EndpointProofPrincipal, EndpointProofPurpose, EndpointProofResponse,
    EndpointSignature, RelayEndpointOwner, RelayLimitDefaults, TenantId, TrafficScope, UserId,
};
use pab_server::{ControlPlane, PasswordPolicy, PostgresStore, ServiceError, StoreError};
use sqlx::PgPool;
use time::OffsetDateTime;

fn prove_endpoint(
    user_id: UserId,
    tenant_id: TenantId,
    purpose: EndpointProofPurpose,
    secret: &SecretKey,
) -> pab_server::VerifiedEndpointProof {
    prove_endpoint_principal(
        EndpointProofPrincipal::User { user_id },
        tenant_id,
        purpose,
        secret,
    )
}

fn prove_endpoint_principal(
    principal: EndpointProofPrincipal,
    tenant_id: TenantId,
    purpose: EndpointProofPurpose,
    secret: &SecretKey,
) -> pab_server::VerifiedEndpointProof {
    let now = OffsetDateTime::now_utc();
    let mut session = pab_server::EndpointProofSession::new();
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
async fn account_device_and_user_policy_flow(pool: PgPool) {
    let store = PostgresStore::from_pool(pool);
    let control = ControlPlane::new(store, PasswordPolicy::default()).unwrap();
    control
        .initialize_settings(RelayLimitDefaults {
            user_mbps: 5,
            guest_mbps: 1,
        })
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

    let alice_scopes = control.list_traffic_scopes(&alice).await.unwrap();
    assert_eq!(alice_scopes.personal_tenant_id, alice.personal_tenant_id);
    assert_eq!(alice_scopes.personal_mbps, 5);

    let alice_key = SecretKey::generate();
    let bob_key = SecretKey::generate();
    let alice_personal_key = SecretKey::generate();
    let device_key = SecretKey::generate();
    let unauthorized_device_key = SecretKey::generate();
    control
        .register_user_endpoint(prove_endpoint(
            alice.id,
            alice.personal_tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &alice_key,
        ))
        .await
        .unwrap();
    control
        .register_user_endpoint(prove_endpoint(
            bob.id,
            bob.personal_tenant_id,
            EndpointProofPurpose::RegisterUserEndpoint,
            &bob_key,
        ))
        .await
        .unwrap();
    control
        .register_user_endpoint(prove_endpoint(
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
                    bob.id,
                    alice.personal_tenant_id,
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
    assert_eq!(snapshot.defaults.user_mbps, 5);
    assert!(snapshot.user_limits.is_empty());
    assert_eq!(snapshot.endpoints.len(), 4);
    assert!(snapshot.endpoints.iter().any(|endpoint| {
        endpoint.owner
            == RelayEndpointOwner::User {
                scope: TrafficScope::User { user_id: bob.id },
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
            alice.id,
            alice.personal_tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &alice_key,
        ))
        .await
        .unwrap();
    let bob_endpoint = control
        .authenticate_registered_endpoint(prove_endpoint(
            bob.id,
            bob.personal_tenant_id,
            EndpointProofPurpose::AuthenticateRegisteredEndpoint,
            &bob_key,
        ))
        .await
        .unwrap();
    let alice_devices = control.list_my_devices(&alice_endpoint).await.unwrap();
    assert_eq!(alice_devices.len(), 1);
    assert_eq!(alice_devices[0].device_ref, device_ref);
    assert!(
        control
            .list_my_devices(&bob_endpoint,)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(device.code.to_string().len(), 9);
    assert_eq!(
        control
            .resolve_device_code(&alice_endpoint, device.code,)
            .await
            .unwrap(),
        device_ref
    );
    // This test has no live WSS device session: the service reports offline,
    // while store-level discovery validates identity and creates the pair grant.
    assert!(matches!(
        control
            .device_network_snapshot(&alice_endpoint, device_ref)
            .await,
        Err(ServiceError::DeviceOffline)
    ));
    assert_eq!(
        control
            .store()
            .device_network_snapshot(
                alice_endpoint.endpoint_key,
                alice.id,
                alice_endpoint.tenant_id,
                device_ref
            )
            .await
            .unwrap()
            .endpoint_key,
        EndpointKey::new(*device_key.public().as_bytes())
    );
    // A valid account can connect by a saved DeviceRef before resolving its code.
    assert_eq!(
        control
            .store()
            .device_network_snapshot(
                bob_endpoint.endpoint_key,
                bob.id,
                bob_endpoint.tenant_id,
                device_ref
            )
            .await
            .unwrap()
            .device_ref,
        device_ref
    );
    assert_eq!(
        control
            .resolve_device_code(&bob_endpoint, device.code,)
            .await
            .unwrap(),
        device_ref
    );
    assert_eq!(
        control
            .store()
            .device_network_snapshot(
                bob_endpoint.endpoint_key,
                bob.id,
                bob_endpoint.tenant_id,
                device_ref
            )
            .await
            .unwrap()
            .device_ref,
        device_ref
    );
    let invalid_ref = DeviceRef {
        tenant_id: TenantId::new(),
        ..device_ref
    };
    assert!(matches!(
        control
            .store()
            .device_network_snapshot(
                bob_endpoint.endpoint_key,
                bob.id,
                bob_endpoint.tenant_id,
                invalid_ref
            )
            .await,
        Err(StoreError::NotFound)
    ));
    sqlx::query("UPDATE endpoints SET status = 'revoked' WHERE endpoint_key = $1")
        .bind(bob_endpoint.endpoint_key.as_bytes().as_slice())
        .execute(control.store().pool())
        .await
        .unwrap();
    assert!(matches!(
        control
            .store()
            .device_network_snapshot(
                bob_endpoint.endpoint_key,
                bob.id,
                bob_endpoint.tenant_id,
                device_ref
            )
            .await,
        Err(StoreError::NotFound)
    ));
    sqlx::query("DELETE FROM device_network WHERE device_id = $1")
        .bind(device.id.as_uuid())
        .execute(control.store().pool())
        .await
        .unwrap();
    assert_eq!(
        control
            .resolve_device_code(&alice_endpoint, device.code,)
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
