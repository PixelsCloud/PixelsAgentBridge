//! Read-only native account preparation. Registry references are assigned by the
//! authenticated Executor connection, never by this account inventory layer.
use super::system_query::{bound_reply, now, push_bounded};
use pab_os_control::execution::{PreparedUser, UserIdentity, current_identity};
use pab_protocol::*;

fn observed(identity: UserIdentity, mode: ExecutionMode) -> ExecutionContextEntry {
    let mut row = ExecutionContextEntry {
        mode,
        account_name: identity.account_name.clone(),
        account_id: Some(identity.account_id.clone()),
        session_id: identity.session_id.map(|v| v.to_string()),
        identity: None,
        selection: None,
        unavailable_reason: None,
    };
    let source = if mode == ExecutionMode::Service {
        ExecutionEnvironmentSource::ServiceProcess
    } else {
        ExecutionEnvironmentSource::NativeAccount
    };
    match identity.observation(mode, source) {
        Ok(identity) => row.identity = Some(identity),
        Err(_) => row.unavailable_reason = Some("account_fields_unavailable".into()),
    }
    row
}

fn unavailable(
    mode: ExecutionMode,
    name: String,
    account_id: Option<String>,
    session_id: Option<String>,
    code: &str,
) -> ExecutionContextEntry {
    ExecutionContextEntry {
        mode,
        account_name: name,
        account_id,
        session_id,
        identity: None,
        selection: None,
        unavailable_reason: Some(code.into()),
    }
}

fn matches_user(name: &str, user: Option<&str>) -> bool {
    user.is_none_or(|wanted| name.eq_ignore_ascii_case(wanted))
}

/// The service row is always included. User filter is exact; include_system
/// applies to Unix account enumeration. No Linux logind or GUI call is made.
pub async fn collect_execution_contexts(id: RequestId, query: &SystemQuery) -> SystemQueryReply {
    let mut reply = SystemQueryReply::pending(id, query);
    let SystemQuery::ExecutionContexts {
        user,
        include_system,
        limit,
    } = query
    else {
        reply.state = "failed".into();
        reply.error = Some("wrong execution context query".into());
        return reply;
    };
    if let Err(error) = query.validate() {
        reply.state = "failed".into();
        reply.error = Some(error.into());
        return reply;
    }
    reply.sampled_from_unix_ms = Some(now());
    #[cfg(windows)]
    let sessions = pab_os_sessions::collect().await;
    let user = user.clone();
    let include_system = *include_system;
    let limit = *limit as usize;
    let result=tokio::task::spawn_blocking(move || {
        let mut rows=Vec::new();
        rows.push(match current_identity() {
            Ok(identity)=>observed(identity,ExecutionMode::Service),
            Err(_)=>unavailable(ExecutionMode::Service,"service".into(),None,None,"service_identity_unavailable"),
        });
        let mut warnings=vec![];
        let mut truncated=false;
        #[cfg(windows)]
        {
            let _=include_system;
            match sessions {
                Ok(batch)=>{
                    truncated |= batch.truncated;
                    for session in batch.entries {
                        let Some(name)=session.user_name else {continue;};
                        let qualified=session.domain.filter(|v|!v.is_empty()).map_or_else(||name.clone(),|d|format!("{d}\\{name}"));
                        if !matches_user(&name,user.as_deref()) && !matches_user(&qualified,user.as_deref()) {continue;}
                        if rows.len()>=limit {truncated=true;break;}
                        let prepared=session.id.parse::<u32>().ok().and_then(|id|PreparedUser::for_session(id).ok());
                        rows.push(match prepared {
                            Some(value)=>observed(value.identity().clone(),ExecutionMode::User),
                            None=>unavailable(ExecutionMode::User,qualified,None,Some(session.id),"user_token_unavailable"),
                        });
                    }
                }
                Err(_)=>warnings.push("session_inventory_unavailable: Windows user contexts could not be collected".into()),
            }
        }
        #[cfg(unix)]
        {
            let users=sysinfo::Users::new_with_refreshed_list();
            let mut users=users.list().iter().collect::<Vec<_>>();
            users.sort_by_key(|u|u.id().to_string());
            for account in users {
                let Ok(uid)=account.id().to_string().parse::<u32>() else {continue;};
                if !matches_user(account.name(),user.as_deref()) {continue;}
                // An explicit name may select a system account. Otherwise hide
                // conventional low-numbered service users and nobody by default.
                let threshold=if cfg!(target_os="macos") {500}else{1000};
                if !include_system && user.is_none() && (uid<threshold || uid==65534 || uid==u32::MAX-1) {continue;}
                if rows.len()>=limit {truncated=true;break;}
                rows.push(match PreparedUser::for_uid(uid) {
                    Ok(value)=>observed(value.identity().clone(),ExecutionMode::User),
                    Err(error)=>unavailable(ExecutionMode::User,account.name().into(),Some(format!("uid:{uid}")),None,
                        if error.kind()==std::io::ErrorKind::PermissionDenied {"identity_switch_denied"}else{"account_unavailable"}),
                });
            }
            if rows.len()==1 { warnings.push("No matching user accounts were observed. The service identity remains explicit.".into()); }
        }
        (rows,truncated,warnings)
    }).await;
    match result {
        Ok((rows, truncated, warnings)) => {
            reply.warnings = warnings;
            reply.truncated = truncated;
            if truncated {
                reply.stop_reason = Some("entry_limit".into());
            }
            let mut entries = vec![];
            let mut bytes = 0;
            for row in rows {
                if !push_bounded(&mut entries, row, &mut bytes, &mut reply) {
                    break;
                }
            }
            reply.data = Some(SystemQueryData::ExecutionContexts {
                backend: if cfg!(windows) {
                    "windows_wts_token"
                } else {
                    "sysinfo_native_accounts"
                }
                .into(),
                entries,
            });
            reply.state = "completed".into();
        }
        Err(_) => {
            reply.state = "failed".into();
            reply.error = Some("execution context collector stopped unexpectedly".into());
        }
    }
    reply.sampled_at_unix_ms = Some(now());
    bound_reply(&mut reply);
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn service_identity_is_native_and_inventory_does_not_assign_remote_references() {
        let result = collect_execution_contexts(
            RequestId::new(),
            &SystemQuery::ExecutionContexts {
                user: None,
                include_system: false,
                limit: 1,
            },
        )
        .await;
        assert_eq!(result.state, "completed");
        let Some(SystemQueryData::ExecutionContexts { entries, .. }) = result.data else {
            panic!("missing context inventory")
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, ExecutionMode::Service);
        assert_eq!(
            entries[0].account_id.as_deref(),
            Some(current_identity().unwrap().account_id.as_str())
        );
        assert!(entries[0].selection.is_none());
    }
}
