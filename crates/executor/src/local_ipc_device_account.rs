use super::*;
use pab_protocol::{DeviceAccountChallenge, DeviceAccountProof, EndpointSignature};

pub(super) async fn sign(
    root: &Path,
    challenge: DeviceAccountChallenge,
) -> Result<DeviceAccountProof, LocalIpcError> {
    let origin = pab_agent_core::account::account_origin(
        &env::var("PAB_CONTROL_URL").map_err(|_| LocalIpcError::Protocol)?,
    )
    .map_err(|_| LocalIpcError::Protocol)?;
    let path = env::var_os("PAB_ENDPOINT_SECRET_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("device-endpoint.key"));
    let secret =
        pab_agent_core::read_endpoint_secret(&path).map_err(|_| LocalIpcError::Authentication)?;
    let status = read_device_status_from_service(root)
        .await
        .map_err(|_| LocalIpcError::Protocol)?;
    validate(&challenge, &origin, &status.device_id, &secret)?;
    Ok(DeviceAccountProof {
        challenge_id: challenge.id,
        signature: EndpointSignature::from_bytes(
            secret.sign(&challenge.signing_message()).to_bytes(),
        ),
    })
}

fn validate(
    challenge: &DeviceAccountChallenge,
    origin: &str,
    device_id: &str,
    secret: &iroh_base::SecretKey,
) -> Result<(), LocalIpcError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| LocalIpcError::Protocol)?
        .as_millis() as i64;
    if challenge.server_origin != origin
        || challenge.device_id.to_string() != device_id
        || challenge.endpoint_key.as_bytes() != secret.public().as_bytes()
        || challenge.expected_revision < 0
        || challenge.expires_at_unix_ms <= now
        || challenge.expires_at_unix_ms > now + 360_000
    {
        return Err(LocalIpcError::Authentication);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signing_is_limited_to_this_device_server_and_current_challenge() {
        let secret = iroh_base::SecretKey::generate();
        let challenge = DeviceAccountChallenge {
            id: pab_protocol::RequestId::new().as_uuid(),
            server_origin: "https://example.test".into(),
            device_id: pab_protocol::DeviceId::new(),
            endpoint_key: pab_protocol::EndpointKey::new(*secret.public().as_bytes()),
            user_id: pab_protocol::UserId::new(),
            action: pab_protocol::DeviceAccountAction::Automatic,
            expected_revision: 0,
            expires_at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
                + 300_000,
        };
        let id = challenge.device_id.to_string();
        assert!(validate(&challenge, "https://example.test", &id, &secret).is_ok());
        assert!(validate(&challenge, "https://other.test", &id, &secret).is_err());
        assert!(
            validate(
                &challenge,
                "https://example.test",
                &pab_protocol::DeviceId::new().to_string(),
                &secret
            )
            .is_err()
        );
        assert!(
            validate(
                &challenge,
                "https://example.test",
                &id,
                &iroh_base::SecretKey::generate()
            )
            .is_err()
        );
        let expired = DeviceAccountChallenge {
            expires_at_unix_ms: 0,
            ..challenge
        };
        assert!(validate(&expired, "https://example.test", &id, &secret).is_err());
    }
}
