use super::{WebError, WebState};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use iroh_base::{PublicKey, Signature};
use pab_protocol::{EndpointUserContext, EndpointUserContextUpdate, UserAttribution, UserId};
use sqlx::Row;

pub(crate) async fn update(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<EndpointUserContextUpdate>,
) -> Result<Json<pab_protocol::EndpointUserContextReceipt>, WebError> {
    let revision = i64::try_from(input.revision).map_err(|_| WebError::invalid())?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    if (now - i128::from(input.issued_at_unix_ms)).abs() > 300_000 {
        return Err(WebError::new(StatusCode::BAD_REQUEST, "stale_proof"));
    }
    let digest = if headers.contains_key(header::AUTHORIZATION) {
        Some(super::session::native_token_hash(&headers).ok_or_else(WebError::unauthorized)?)
    } else {
        None
    };
    let origin = std::env::var("PAB_WEB_ORIGIN").unwrap_or_else(|_| {
        format!(
            "https://{}",
            headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        )
    });
    let origin = url::Url::parse(&origin)
        .map_err(|_| WebError::invalid())?
        .origin()
        .ascii_serialization();
    let public =
        PublicKey::from_bytes(input.endpoint_key.as_bytes()).map_err(|_| WebError::invalid())?;
    public
        .verify(
            &input.signing_message(&origin, digest.as_deref().unwrap_or("")),
            &Signature::from_bytes(&input.signature.to_bytes()),
        )
        .map_err(|_| WebError::new(StatusCode::FORBIDDEN, "invalid_endpoint_proof"))?;
    let mut tx = state.control.store().pool().begin().await?;
    // Endpoint lock serializes concurrent first-bind requests and account switches.
    let endpoint: Option<Vec<u8>> = sqlx::query_scalar("SELECT endpoint_key FROM endpoints WHERE endpoint_key=$1 AND owner_kind IN ('guest','user') AND status='active' FOR UPDATE")
        .bind(input.endpoint_key.as_bytes().as_slice()).fetch_optional(&mut *tx).await?;
    if endpoint.is_none() {
        return Err(WebError::new(StatusCode::FORBIDDEN, "unknown_endpoint"));
    }
    let user = if let Some(hash) = &digest {
        // Lock the session against concurrent logout until the binding commits.
        let row = sqlx::query("SELECT u.id,u.username,u.status FROM web_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 FOR SHARE OF s,u")
            .bind(hash).fetch_optional(&mut *tx).await?.ok_or_else(WebError::unauthorized)?;
        if row.try_get::<String, _>("status")? == "active" {
            Some(UserAttribution {
                user_id: UserId::from_uuid(row.try_get("id")?),
                username: row.try_get("username")?,
            })
        } else {
            None
        }
    } else {
        None
    };
    let previous =
        sqlx::query("SELECT revision,token_hash FROM endpoint_user_contexts WHERE endpoint_key=$1")
            .bind(input.endpoint_key.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await?;
    let mut changed = true;
    if let Some(previous) = previous {
        let old_revision: i64 = previous.try_get("revision")?;
        let old_hash: Option<String> = previous.try_get("token_hash")?;
        if old_revision > revision || (old_revision == revision && old_hash != digest) {
            return Err(WebError::new(
                StatusCode::CONFLICT,
                "stale_account_revision",
            ));
        }
        changed = old_revision != revision || old_hash != digest;
    }
    if changed {
        sqlx::query("INSERT INTO endpoint_user_contexts(endpoint_key,token_hash,revision) VALUES($1,$2,$3) ON CONFLICT(endpoint_key) DO UPDATE SET token_hash=excluded.token_hash,revision=excluded.revision,updated_at=now()")
            .bind(input.endpoint_key.as_bytes().as_slice()).bind(digest).bind(revision).execute(&mut *tx).await?;
        sqlx::query("UPDATE server_settings SET policy_revision=policy_revision+1 WHERE singleton")
            .execute(&mut *tx)
            .await?;
    }
    let policy_version: i64 =
        sqlx::query_scalar("SELECT policy_revision FROM server_settings WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    if changed {
        state.control.web_changed();
    }
    let relay_nodes = sqlx::query("SELECT node_id,applied_policy_version,server_instance=$1 AND last_seen_at>now()-interval '120 seconds' AS online FROM relay_nodes ORDER BY node_id")
        .bind(state.server_instance).fetch_all(state.control.store().pool()).await?.into_iter().map(|row| {
            Ok::<_,WebError>(pab_protocol::RelayUserContextReceipt { node_id:row.try_get("node_id")?, applied_policy_version:row.try_get::<Option<i64>,_>("applied_policy_version")?.map(|v|v as u64), online:row.try_get("online")? })
        }).collect::<Result<Vec<_>,_>>()?;
    Ok(Json(pab_protocol::EndpointUserContextReceipt {
        context: EndpointUserContext {
            revision: input.revision,
            user,
            policy_version: policy_version as u64,
        },
        relay_nodes,
    }))
}
