//! Personal display associations do not grant endpoint or task authorization.
use super::{WebError, WebState, session};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
};
use iroh_base::{PublicKey, Signature};
use pab_protocol::{
    DeviceAccountAction, DeviceAccountChallenge, DeviceAccountChallengeRequest, DeviceAccountProof,
    DeviceAccountState, DeviceAccountStatus, DeviceAccountUnlink, DeviceId, EndpointKey, UserId,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

fn conflict() -> WebError {
    WebError::new(StatusCode::CONFLICT, "association_changed")
}

fn status(owner: Option<Uuid>, revision: i64, user: Uuid) -> DeviceAccountState {
    DeviceAccountState {
        status: match owner {
            Some(id) if id == user => DeviceAccountStatus::Associated,
            Some(_) => DeviceAccountStatus::OtherAccount,
            None if revision > 0 => DeviceAccountStatus::Unlinked,
            None => DeviceAccountStatus::Available,
        },
        revision,
        challenge: None,
    }
}

async fn lock_session(
    tx: &mut Transaction<'_, Postgres>,
    hash: &str,
    user: Uuid,
) -> Result<(), WebError> {
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT u.id FROM web_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND u.id=$2 AND u.status='active' FOR SHARE OF s,u")
        .bind(hash).bind(user).fetch_optional(&mut **tx).await?;
    exists.ok_or_else(WebError::unauthorized)?;
    Ok(())
}

async fn lock_device(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<(), WebError> {
    sqlx::query("SELECT id FROM devices WHERE id=$1 AND status='active' FOR UPDATE")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn challenge(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<DeviceAccountChallengeRequest>,
) -> Result<Json<DeviceAccountState>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    let hash = session::native_token_hash(&headers).ok_or_else(WebError::unauthorized)?;
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
    let mut tx = state.control.store().pool().begin().await?;
    lock_session(&mut tx, &hash, me.id).await?;
    lock_device(&mut tx, id).await?;
    let key: Vec<u8> = sqlx::query_scalar("SELECT endpoint_key FROM endpoints WHERE device_id=$1 AND owner_kind='device' AND status='active' ORDER BY created_at DESC LIMIT 1")
        .bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO device_accounts(device_id) VALUES($1) ON CONFLICT DO NOTHING")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let row =
        sqlx::query("SELECT user_id,revision,updated_at FROM device_accounts WHERE device_id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    let mut result = status(row.try_get("user_id")?, row.try_get("revision")?, me.id);
    let allowed = match input.action {
        DeviceAccountAction::Automatic => match result.status {
            DeviceAccountStatus::Associated => false,
            DeviceAccountStatus::Available | DeviceAccountStatus::OtherAccount => true,
            DeviceAccountStatus::Unlinked => {
                // Keep a Web unlink effective for existing sessions/background retries.
                // A subsequent explicit login creates a new session and associates again.
                sqlx::query_scalar::<_, bool>(
                    "SELECT created_at > $1 FROM web_sessions WHERE token_hash=$2",
                )
                .bind(row.try_get::<time::OffsetDateTime, _>("updated_at")?)
                .bind(&hash)
                .fetch_one(&mut *tx)
                .await?
            }
        },
        DeviceAccountAction::Associate => {
            if input.expected_revision != Some(result.revision) {
                return Err(conflict());
            }
            matches!(
                result.status,
                DeviceAccountStatus::Available | DeviceAccountStatus::Unlinked
            )
        }
        DeviceAccountAction::Replace => {
            if input.expected_revision != Some(result.revision) {
                return Err(conflict());
            }
            result.status != DeviceAccountStatus::Associated
        }
    };
    if allowed {
        let challenge = DeviceAccountChallenge {
            id: Uuid::new_v4(),
            server_origin: origin,
            device_id: DeviceId::from_uuid(id),
            endpoint_key: EndpointKey::new(key.try_into().map_err(|_| WebError::invalid())?),
            user_id: UserId::from_uuid(me.id),
            action: input.action,
            expected_revision: result.revision,
            expires_at_unix_ms: (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                as i64
                + 300_000,
        };
        // At most one outstanding challenge for this session/device. Other sessions cannot invalidate it.
        sqlx::query("DELETE FROM device_account_challenges WHERE expires_at<now() OR (device_id=$1 AND session_hash=$2)")
            .bind(id).bind(&hash).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO device_account_challenges(id,device_id,session_hash,payload,expires_at) VALUES($1,$2,$3,$4,to_timestamp($5::double precision/1000))")
            .bind(challenge.id).bind(id).bind(hash).bind(serde_json::to_value(&challenge).map_err(|_|WebError::invalid())?)
            .bind(challenge.expires_at_unix_ms).execute(&mut *tx).await?;
        result.challenge = Some(challenge);
    }
    tx.commit().await?;
    Ok(Json(result))
}

pub(crate) async fn associate(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<DeviceAccountProof>,
) -> Result<Json<DeviceAccountState>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    let hash = session::native_token_hash(&headers).ok_or_else(WebError::unauthorized)?;
    let mut tx = state.control.store().pool().begin().await?;
    lock_session(&mut tx, &hash, me.id).await?;
    lock_device(&mut tx, id).await?;
    let row = sqlx::query("SELECT payload,applied_revision FROM device_account_challenges WHERE id=$1 AND device_id=$2 AND session_hash=$3 AND expires_at>now() FOR UPDATE")
        .bind(input.challenge_id).bind(id).bind(&hash).fetch_one(&mut *tx).await?;
    let challenge: DeviceAccountChallenge =
        serde_json::from_value(row.try_get("payload")?).map_err(|_| WebError::invalid())?;
    if challenge.user_id.as_uuid() != me.id {
        return Err(WebError::unauthorized());
    }
    let public = PublicKey::from_bytes(challenge.endpoint_key.as_bytes())
        .map_err(|_| WebError::invalid())?;
    public
        .verify(
            &challenge.signing_message(),
            &Signature::from_bytes(&input.signature.to_bytes()),
        )
        .map_err(|_| WebError::new(StatusCode::FORBIDDEN, "invalid_device_proof"))?;
    sqlx::query("SELECT endpoint_key FROM endpoints WHERE device_id=$1 AND endpoint_key=$2 AND owner_kind='device' AND status='active' FOR SHARE")
        .bind(id).bind(challenge.endpoint_key.as_bytes().as_slice()).fetch_one(&mut *tx).await?;
    let current = sqlx::query("SELECT user_id,revision FROM device_accounts WHERE device_id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let owner: Option<Uuid> = current.try_get("user_id")?;
    let revision: i64 = current.try_get("revision")?;
    if let Some(applied) = row.try_get::<Option<i64>, _>("applied_revision")? {
        if revision != applied || owner != Some(me.id) {
            return Err(conflict());
        }
        return Ok(Json(status(owner, revision, me.id)));
    }
    if revision != challenge.expected_revision {
        return Err(conflict());
    }
    let next = revision.checked_add(1).ok_or_else(WebError::invalid)?;
    sqlx::query(
        "UPDATE device_accounts SET user_id=$1,revision=$2,updated_at=now() WHERE device_id=$3",
    )
    .bind(me.id)
    .bind(next)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE device_account_challenges SET applied_revision=$1 WHERE id=$2")
        .bind(next)
        .bind(input.challenge_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'device.associated',$2)")
        .bind(me.id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(status(Some(me.id), next, me.id)))
}

async fn unlink(
    state: WebState,
    user: Uuid,
    id: Uuid,
    expected: i64,
) -> Result<Json<DeviceAccountState>, WebError> {
    let mut tx = state.control.store().pool().begin().await?;
    lock_device(&mut tx, id).await?;
    let row = sqlx::query("SELECT revision FROM device_accounts WHERE device_id=$1 AND user_id=$2")
        .bind(id)
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
    let revision: i64 = row.try_get("revision")?;
    if expected != revision {
        return Err(conflict());
    }
    let next = revision.checked_add(1).ok_or_else(WebError::invalid)?;
    sqlx::query(
        "UPDATE device_accounts SET user_id=NULL,revision=$1,updated_at=now() WHERE device_id=$2",
    )
    .bind(next)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'device.disassociated',$2)")
        .bind(user).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(status(None, next, user)))
}

pub(crate) async fn web_unlink(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<DeviceAccountUnlink>,
) -> Result<Json<DeviceAccountState>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    unlink(state, me.id, id, input.revision).await
}

pub(crate) async fn native_unlink(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<DeviceAccountUnlink>,
) -> Result<Json<DeviceAccountState>, WebError> {
    let me = session::native_viewer(&state, &headers).await?;
    unlink(state, me.id, id, input.revision).await
}
