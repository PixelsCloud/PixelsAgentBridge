use super::{
    WebError, WebState,
    session::{Viewer, viewer},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use pab_protocol::{TenantId, UserId};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn administrator(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Viewer, WebError> {
    let me = viewer(state, headers).await?;
    if !me.server_admin {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(me)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    q: Option<String>,
}
impl PageQuery {
    fn values(&self) -> Result<(i64, i64, &str), WebError> {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(20);
        let search = self.q.as_deref().unwrap_or("").trim();
        if page == 0 || page > 1_000_000 || size == 0 || size > 100 || search.chars().count() > 128
        {
            return Err(WebError::invalid());
        }
        Ok((page.into(), size.into(), search))
    }
}

async fn paged(
    state: &WebState,
    me: &Viewer,
    query: PageQuery,
    sql: &str,
) -> Result<Json<Value>, WebError> {
    let (page, size, search) = query.values()?;
    let mut tx = state.control.store().pool().begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ({sql}) visible"))
        .bind(me.server_admin)
        .bind(me.id)
        .bind(search)
        .fetch_one(&mut *tx)
        .await?;
    let rows = sqlx::query(&format!("{sql} LIMIT $4 OFFSET $5"))
        .bind(me.server_admin)
        .bind(me.id)
        .bind(search)
        .bind(size)
        .bind((page - 1) * size)
        .fetch_all(&mut *tx)
        .await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let text: String = row.try_get("payload")?;
            serde_json::from_str::<Value>(&text)
                .map_err(|_| WebError::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"))
        })
        .collect::<Result<Vec<_>, WebError>>()?;
    tx.commit().await?;
    Ok(Json(
        json!({"items":items,"total":total,"page":page,"page_size":size}),
    ))
}

pub(crate) async fn accounts(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    paged(&state,&me,query,r#"SELECT jsonb_build_object('id',u.id,'username',u.username,'status',u.status,'server_admin',u.server_admin,'revision',u.auth_revision,'default_team_id',u.default_traffic_team_id,'default_team_name',t.name,'created_at',(extract(epoch from u.created_at)*1000)::bigint)::text payload
      FROM users u LEFT JOIN teams t ON t.tenant_id=u.default_traffic_team_id
      WHERE $1 AND $2::uuid IS NOT NULL AND position(lower($3) in lower(u.username))>0 ORDER BY lower(u.username),u.id"#).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountUpdate {
    status: String,
    server_admin: bool,
    revision: i64,
}
pub(crate) async fn update_account(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AccountUpdate>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    if !matches!(input.status.as_str(), "active" | "disabled") {
        return Err(WebError::invalid());
    }
    let mut tx = state.control.store().pool().begin().await?;
    // Same deployment-wide lock is shared by bootstrap and all administrator changes.
    sqlx::query("SELECT pg_advisory_xact_lock(26035001)")
        .execute(&mut *tx)
        .await?;
    let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND server_admin AND status='active' AND auth_revision=$2)")
        .bind(me.id).bind(me.auth_revision).fetch_one(&mut *tx).await?;
    if !allowed {
        return Err(WebError::new(StatusCode::FORBIDDEN, "forbidden"));
    }
    let row =
        sqlx::query("SELECT auth_revision,server_admin,status FROM users WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if row.try_get::<i64, _>("auth_revision")? != input.revision {
        return Err(WebError::new(StatusCode::CONFLICT, "conflict"));
    }
    if row.try_get::<bool, _>("server_admin")?
        && row.try_get::<String, _>("status")? == "active"
        && (!input.server_admin || input.status != "active")
    {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE server_admin AND status='active'")
                .fetch_one(&mut *tx)
                .await?;
        if count <= 1 {
            return Err(WebError::new(StatusCode::CONFLICT, "last_admin"));
        }
    }
    sqlx::query("UPDATE users SET status=$1,server_admin=$2,auth_revision=auth_revision+1,updated_at=now() WHERE id=$3").bind(input.status).bind(input.server_admin).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM web_sessions WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE deployments SET policy_revision=policy_revision+1 WHERE singleton")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'account.updated',$2)",
    )
    .bind(me.id)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.control.web_changed();
    Ok(Json(json!({"revision":input.revision+1})))
}

pub(crate) async fn teams(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    paged(&state,&me,query,r#"SELECT jsonb_build_object('id',t.tenant_id,'name',t.name,'total_mbps',COALESCE(t.relay_total_mbps,d.default_team_mbps),'member_mbps',COALESCE(t.relay_member_mbps,d.default_member_mbps),'role',m.role,'member_count',(SELECT count(*) FROM memberships x WHERE x.tenant_id=t.tenant_id AND x.status='active'))::text payload
      FROM teams t JOIN tenants scope ON scope.id=t.tenant_id AND scope.status='active'
      CROSS JOIN deployments d LEFT JOIN memberships m ON m.tenant_id=t.tenant_id AND m.user_id=$2 AND m.status='active'
      WHERE ($1 OR m.user_id IS NOT NULL) AND position(lower($3) in lower(t.name))>0 ORDER BY lower(t.name),t.tenant_id"#).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TeamCreate {
    name: String,
    owner_id: Uuid,
}
pub(crate) async fn create_team(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(input): Json<TeamCreate>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let team = state
        .control
        .admin_create_team(
            UserId::from_uuid(input.owner_id),
            &input.name,
            &me.id.to_string(),
        )
        .await?;
    state.control.web_changed();
    Ok(Json(json!({"id":team.tenant_id,"name":team.name})))
}

pub(crate) async fn members(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    // UUID is parsed, then bound by the fixed UUID literal in this internal query.
    let sql = format!(
        r#"SELECT jsonb_build_object('id',u.id,'username',u.username,'role',m.role,'status',u.status)::text payload FROM memberships m JOIN users u ON u.id=m.user_id WHERE $1 AND $2::uuid IS NOT NULL AND m.tenant_id='{id}'::uuid AND m.status='active' AND position(lower($3) in lower(u.username))>0 ORDER BY lower(u.username),u.id"#
    );
    paged(&state, &me, query, &sql).await
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TeamAction {
    Rename { name: String },
    SetLimits { total_mbps: u32, member_mbps: u32 },
    AddMember { user_id: Uuid, role: String },
    RemoveMember { user_id: Uuid },
}
pub(crate) async fn team_action(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<TeamAction>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let team = TenantId::from_uuid(id);
    let actor = me.id.to_string();
    match input {
        TeamAction::Rename { name } => {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
                return Err(WebError::invalid());
            }
            let mut tx = state.control.store().pool().begin().await?;
            let result = sqlx::query("UPDATE teams SET name=$1 WHERE tenant_id=$2")
                .bind(name)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            if result.rows_affected() != 1 {
                return Err(WebError::new(StatusCode::NOT_FOUND, "not_found"));
            }
            sqlx::query("INSERT INTO web_admin_events(actor_id,action,resource_id) VALUES($1,'team.rename',$2)").bind(me.id).bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        TeamAction::SetLimits {
            total_mbps,
            member_mbps,
        } => {
            state
                .control
                .admin_set_team_limits(team, total_mbps, member_mbps, &actor)
                .await?;
        }
        TeamAction::AddMember { user_id, role } => {
            let role = match role.as_str() {
                "admin" => crate::TeamRole::Admin,
                "member" => crate::TeamRole::Member,
                _ => return Err(WebError::invalid()),
            };
            state
                .control
                .admin_add_team_member(team, UserId::from_uuid(user_id), role, &actor)
                .await?;
        }
        TeamAction::RemoveMember { user_id } => {
            state
                .control
                .admin_remove_team_member(team, UserId::from_uuid(user_id), &actor)
                .await?;
        }
    }
    state.control.web_changed();
    Ok(Json(json!({"success":true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Assignment {
    team_id: Option<Uuid>,
}
pub(crate) async fn assign_team(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Assignment>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let changed = state
        .control
        .admin_set_default_traffic_team(
            UserId::from_uuid(id),
            input.team_id.map(TenantId::from_uuid),
            &me.id.to_string(),
        )
        .await?;
    state.control.web_changed();
    Ok(Json(json!({"changed":changed})))
}

pub(crate) async fn eligible_teams(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let sql = format!(
        "SELECT jsonb_build_object('id',t.tenant_id,'name',t.name)::text payload FROM teams t JOIN tenants scope ON scope.id=t.tenant_id AND scope.status='active' JOIN memberships m ON m.tenant_id=t.tenant_id AND m.user_id='{id}'::uuid AND m.status='active' WHERE $1 AND $2::uuid IS NOT NULL AND position(lower($3) in lower(t.name))>0 ORDER BY lower(t.name),t.tenant_id"
    );
    paged(&state, &me, query, &sql).await
}

pub(crate) async fn service_config(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    administrator(&state, &headers).await?;
    let row = sqlx::query("SELECT default_team_mbps,default_member_mbps,default_personal_mbps,policy_revision FROM deployments WHERE singleton").fetch_one(state.control.store().pool()).await?;
    Ok(Json(
        json!({"registration_enabled":state.registration_enabled,"version":env!("CARGO_PKG_VERSION"),"default_team_mbps":row.try_get::<i32,_>("default_team_mbps")?,"default_member_mbps":row.try_get::<i32,_>("default_member_mbps")?,"default_personal_mbps":row.try_get::<i32,_>("default_personal_mbps")?,"policy_revision":row.try_get::<i64,_>("policy_revision")?,"session_hours":12}),
    ))
}

pub(crate) async fn audit(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    paged(&state,&me,query,r#"SELECT jsonb_build_object('id',e.id,'actor',COALESCE(u.username,e.actor),'action',e.action,'resource_id',e.resource,'resource_name',COALESCE((SELECT name FROM devices WHERE id=e.resource),(SELECT name FROM teams WHERE tenant_id=e.resource),(SELECT username FROM users WHERE id=e.resource)),'created_at',(extract(epoch from e.at)*1000)::bigint)::text payload FROM (
      SELECT 'web:'||id::text id, actor_id::text actor,action,resource_id resource,created_at at FROM web_admin_events
      UNION ALL SELECT 'team:'||id::text,operator_label,action,tenant_id,created_at FROM team_admin_events
      UNION ALL SELECT 'traffic:'||id::text,operator_label,'traffic.assignment',user_id,created_at FROM account_traffic_assignment_events
      ) e LEFT JOIN users u ON u.id::text=e.actor WHERE $1 AND $2::uuid IS NOT NULL AND (position(lower($3) in lower(e.action))>0 OR position(lower($3) in lower(COALESCE(u.username,e.actor,'')))>0) ORDER BY e.at DESC,e.id DESC"#).await
}

pub(crate) async fn traffic(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, WebError> {
    let me = viewer(&state, &headers).await?;
    let scopes = state
        .control
        .list_traffic_scopes(&crate::Account {
            id: UserId::from_uuid(me.id),
            username: me.username,
            personal_tenant_id: TenantId::from_uuid(me.personal_tenant_id),
        })
        .await?;
    Ok(Json(json!({"scopes":scopes})))
}

pub(crate) async fn relays(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Result<Json<Value>, WebError> {
    let me = administrator(&state, &headers).await?;
    let instance = state.server_instance;
    let sql = format!(
        "SELECT jsonb_build_object('id',node_id,'agent_version',agent_version,'applied_policy_version',applied_policy_version,'offered_policy_version',offered_policy_version,'last_seen',(extract(epoch from last_seen_at)*1000)::bigint,'online',server_instance='{instance}'::uuid AND last_seen_at>now()-interval '120 seconds')::text payload FROM relay_nodes WHERE $1 AND $2::uuid IS NOT NULL AND position(lower($3) in lower(node_id))>0 ORDER BY node_id"
    );
    paged(&state, &me, query, &sql).await
}
