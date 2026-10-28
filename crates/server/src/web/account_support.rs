use super::{
    WebError, WebState,
    management::{PageQuery, administrator},
    session::{self, Viewer},
};
use crate::{
    StoreError,
    postgres::{SupportActor, SupportMutation},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

fn store_error(error: StoreError) -> WebError {
    match error {
        StoreError::InvalidState(ref code) if code == "password_login_required" => {
            WebError::new(StatusCode::CONFLICT, "password_login_required")
        }
        StoreError::InvalidState(ref code) if code == "use_self_password_change" => {
            WebError::new(StatusCode::CONFLICT, "use_self_password_change")
        }
        other => crate::ServiceError::from(other).into(),
    }
}

fn actor<'a>(me: &Viewer, hash: &'a str) -> SupportActor<'a> {
    SupportActor {
        id: me.id,
        revision: me.auth_revision,
        session_hash: hash,
    }
}

pub(crate) async fn detail(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let sql = format!(
        r#"WITH bounds AS (SELECT (floor(extract(epoch from statement_timestamp())/3600)*3600000+3600000)::bigint until_ms),
      recent AS (SELECT h.* FROM usage_hourly h CROSS JOIN bounds b WHERE h.subject=($1::uuid)::text AND h.hour_unix_ms>=b.until_ms-604800000 AND h.hour_unix_ms<b.until_ms),
      sums AS (SELECT key,sum(value::numeric)::text total FROM recent CROSS JOIN LATERAL jsonb_each_text(counters) WHERE key IN ({metrics}) GROUP BY key)
      SELECT jsonb_build_object('id',u.id,'username',u.username,'status',u.status,'server_admin',u.server_admin,'revision',u.auth_revision,
        'created_at',(extract(epoch from u.created_at)*1000)::bigint,'relay_limit_mbps',u.relay_limit_mbps,
        'password_enabled',u.password_hash IS NOT NULL,'github_enabled',EXISTS(SELECT 1 FROM external_identities e WHERE e.user_id=u.id AND e.provider='github'),
        'device_count',(SELECT count(*) FROM device_accounts d WHERE d.user_id=u.id),
        'usage',jsonb_build_object('from',b.until_ms-604800000,'until',b.until_ms,'interval','hour',
          'counters',COALESCE((SELECT jsonb_object_agg(key,total) FROM sums),'{{}}'::jsonb),
          'incomplete',COALESCE((SELECT bool_or(COALESCE((counters->>'incomplete')::boolean,false)) FROM recent),false)
            OR EXISTS(SELECT 1 FROM usage_hourly h WHERE h.source_kind='relay' AND h.subject='guest' AND h.hour_unix_ms>=b.until_ms-604800000 AND h.hour_unix_ms<b.until_ms AND COALESCE((h.counters->>'incomplete')::boolean,false)),
          'updated_at',(SELECT (extract(epoch from max(updated_at))*1000)::bigint FROM recent)))::text
      FROM users u CROSS JOIN bounds b WHERE u.id=$1"#,
        metrics = super::usage::counter_metric_list()
    );
    let payload: String = sqlx::query_scalar(&sql)
        .bind(id)
        .fetch_one(state.control.store().pool())
        .await?;
    Ok(Json(serde_json::from_str(&payload).map_err(|_| {
        WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error")
    })?))
}

async fn list_sessions(
    state: &WebState,
    me: &Viewer,
    headers: &HeaderMap,
    target: Uuid,
    query: PageQuery,
) -> Result<Json<Value>, WebError> {
    let (page, size, search) = query.values()?;
    let hash = session::token_hash(headers).ok_or_else(WebError::unauthorized)?;
    let mut tx = state.control.store().begin_snapshot(true).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(WebError::new(StatusCode::NOT_FOUND, "not_found"));
    }
    let total: i64=sqlx::query_scalar("SELECT count(*) FROM web_sessions WHERE user_id=$1 AND (position(lower($2) in channel)>0 OR position(lower($2) in id::text)>0)").bind(target).bind(search).fetch_one(&mut *tx).await?;
    let rows=sqlx::query("SELECT id,channel,(extract(epoch from created_at)*1000)::bigint created_at,(extract(epoch from last_active_at)*1000)::bigint last_active_at,token_hash=$2 AND user_id=$3 AS is_current FROM web_sessions WHERE user_id=$1 AND (position(lower($6) in channel)>0 OR position(lower($6) in id::text)>0) ORDER BY created_at DESC,id DESC LIMIT $4 OFFSET $5")
        .bind(target).bind(hash).bind(me.id).bind(size).bind((page-1)*size).bind(search).fetch_all(&mut *tx).await?;
    let items: Vec<Value>=rows.into_iter().map(|row| Ok(json!({"id":row.try_get::<Uuid,_>("id")?,"channel":row.try_get::<String,_>("channel")?,"created_at":row.try_get::<i64,_>("created_at")?,"last_active_at":row.try_get::<Option<i64>,_>("last_active_at")?,"current":row.try_get::<bool,_>("is_current")?}))).collect::<Result<_,sqlx::Error>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}

pub(crate) async fn sessions(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    list_sessions(&state, &me, &headers, id, query).await
}

pub(crate) async fn own_sessions(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    list_sessions(&state, &me, &headers, me.id, query).await
}

pub(crate) async fn operation(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    let result = state
        .control
        .store()
        .account_support_operation(me.id, id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| WebError::new(StatusCode::NOT_FOUND, "not_found"))?;
    if result.target_id != me.id && !me.server_admin {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(Json(
        serde_json::to_value(result).map_err(|_| WebError::invalid())?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PasswordReset {
    operation_id: Uuid,
    revision: i64,
    #[serde(default)]
    new_password: String,
    #[serde(default = "default_revoke")]
    revoke_sessions: bool,
}
fn default_revoke() -> bool {
    true
}

pub(crate) async fn reset_password(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<PasswordReset>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if id == me.id {
        return Err(WebError::new(
            StatusCode::CONFLICT,
            "use_self_password_change",
        ));
    }
    let mutation = SupportMutation {
        id: input.operation_id,
        request_id: super::audit_context::request_id(&headers),
        target: id,
        kind: "password_reset",
        scope: None,
        session_id: None,
        revoke_sessions: input.revoke_sessions,
        revision: Some(input.revision),
    };
    if let Some(previous) = state
        .control
        .store()
        .account_support_operation(me.id, mutation.id)
        .await
        .map_err(store_error)?
    {
        if !previous.matches(me.id, &mutation) {
            return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
        }
        return Ok(Json(
            serde_json::to_value(previous).map_err(|_| WebError::invalid())?,
        ));
    }
    let _permit = session::login_permit(&state).await?;
    let hash = tokio::task::spawn_blocking(move || {
        crate::auth::PasswordEngine::new(crate::PasswordPolicy::default()).hash(&input.new_password)
    })
    .await
    .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))?
    .map_err(|_| WebError::invalid())?;
    let session = session::token_hash(&headers).ok_or_else(WebError::unauthorized)?;
    let result = state
        .control
        .store()
        .apply_account_support(actor(&me, &session), &mutation, Some(&hash))
        .await
        .map_err(store_error)?;
    state.control.web_changed();
    Ok(Json(
        serde_json::to_value(result).map_err(|_| WebError::invalid())?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RevokeScope {
    All,
    Others,
    Session,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeSessions {
    operation_id: Uuid,
    scope: RevokeScope,
    session_id: Option<Uuid>,
}

async fn revoke(
    state: &WebState,
    headers: &HeaderMap,
    me: &Viewer,
    target: Uuid,
    input: RevokeSessions,
) -> Result<Json<Value>, WebError> {
    let scope = match input.scope {
        RevokeScope::All => "all",
        RevokeScope::Others => "others",
        RevokeScope::Session => "session",
    };
    if (scope == "session") != input.session_id.is_some() || (scope == "others" && target != me.id)
    {
        return Err(WebError::invalid());
    }
    let mutation = SupportMutation {
        id: input.operation_id,
        request_id: super::audit_context::request_id(headers),
        target,
        kind: "session_revoke",
        scope: Some(scope),
        session_id: input.session_id,
        revoke_sessions: true,
        revision: None,
    };
    let hash = session::token_hash(headers).ok_or_else(WebError::unauthorized)?;
    let result = state
        .control
        .store()
        .apply_account_support(actor(me, &hash), &mutation, None)
        .await
        .map_err(store_error)?;
    state.control.web_changed();
    Ok(Json(
        serde_json::to_value(result).map_err(|_| WebError::invalid())?,
    ))
}
pub(crate) async fn revoke_sessions(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RevokeSessions>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    revoke(&state, &headers, &me, id, input).await
}
pub(crate) async fn revoke_own_sessions(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<RevokeSessions>,
) -> Result<Json<Value>, WebError> {
    let me = session::viewer(&state, &headers).await?;
    revoke(&state, &headers, &me, me.id, input).await
}
