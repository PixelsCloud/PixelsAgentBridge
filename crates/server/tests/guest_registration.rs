use std::time::Duration;

use iroh_base::SecretKey;
use pab_protocol::{
    DeploymentId, DeviceId, EndpointKey, EndpointProofPrincipal, EndpointProofPurpose,
    EndpointProofResponse, EndpointSignature, RelayLimitDefaults, TenantId,
};
use pab_server::{ControlPlane, EndpointProofSession, PasswordPolicy, PostgresStore};
use sqlx::PgPool;
use time::OffsetDateTime;

fn proof(
    secret: &SecretKey,
    principal: EndpointProofPrincipal,
    tenant_id: TenantId,
    purpose: EndpointProofPurpose,
) -> pab_server::VerifiedEndpointProof {
    let now = OffsetDateTime::now_utc();
    let mut session = EndpointProofSession::new(DeploymentId::from_u128(1));
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
        .verify(
            EndpointProofResponse {
                challenge_id: challenge.challenge_id,
                signature: EndpointSignature::from_bytes(
                    secret.sign(&challenge.signing_message()).to_bytes(),
                ),
            },
            now,
        )
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn self_registration_is_idempotent_and_does_not_create_an_account(pool: PgPool) {
    let store = PostgresStore::from_pool(pool);
    let control = ControlPlane::new(store.clone(), PasswordPolicy::default()).unwrap();
    control
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

    let device_secret = SecretKey::generate();
    let first = control
        .register_unclaimed_device(
            proof(
                &device_secret,
                EndpointProofPrincipal::Device {
                    device_id: DeviceId::from_u128(2),
                },
                TenantId::from_u128(3),
                EndpointProofPurpose::RegisterUnclaimedDevice,
            ),
            "unclaimed-linux",
        )
        .await
        .unwrap();
    let repeated = control
        .register_unclaimed_device(
            proof(
                &device_secret,
                EndpointProofPrincipal::Device {
                    device_id: DeviceId::from_u128(4),
                },
                TenantId::from_u128(5),
                EndpointProofPurpose::RegisterUnclaimedDevice,
            ),
            "ignored-name",
        )
        .await
        .unwrap();
    assert_eq!(first.id, repeated.id);
    assert_eq!(first.code, repeated.code);
    assert_eq!(first.tenant_id, repeated.tenant_id);

    let guest_secret = SecretKey::generate();
    let guest_tenant = control
        .register_guest_endpoint(proof(
            &guest_secret,
            EndpointProofPrincipal::Guest,
            TenantId::from_u128(6),
            EndpointProofPurpose::RegisterGuestEndpoint,
        ))
        .await
        .unwrap();
    let guest_again = control
        .register_guest_endpoint(proof(
            &guest_secret,
            EndpointProofPrincipal::Guest,
            TenantId::from_u128(7),
            EndpointProofPurpose::RegisterGuestEndpoint,
        ))
        .await
        .unwrap();
    assert_eq!(guest_tenant, guest_again);
    assert_eq!(
        control
            .registered_endpoint(EndpointKey::new(*guest_secret.public().as_bytes()))
            .await
            .unwrap()
            .principal,
        EndpointProofPrincipal::Guest
    );

    let (users, devices, guests): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM users), \
                (SELECT count(*) FROM devices WHERE owner_tenant_id IS NULL), \
                (SELECT count(*) FROM endpoints WHERE owner_kind = 'guest')",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!((users, devices, guests), (0, 1, 1));
}
