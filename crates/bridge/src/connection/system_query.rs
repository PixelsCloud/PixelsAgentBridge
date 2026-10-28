use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};
use pab_protocol::*;

impl AuthenticatedDeviceConnection {
    /// References belong to one authenticated connection. Re-discover a frozen
    /// native identity after reconnect; never substitute another account/mode.
    pub async fn resolve_file_execution(&self, expected:&FileExecutionIdentity)->Result<ExecutionSelection,BridgeError> {
        expected.validate().map_err(|message|BridgeError::FileTransfer(message.into()))?;
        if expected.identity.mode==ExecutionMode::Service {return Ok(ExecutionSelection::Service {});}
        let mut cache=self.file_execution_selections.lock().await;
        if let Some((_,selection))=cache.iter().find(|(identity,_)|identity==expected){return Ok(*selection);}
        let id=RequestId::new();
        let query=SystemQuery::ExecutionContexts {user:Some(expected.identity.account_name.clone()),include_system:false,limit:1000};
        let mut reply=match self.system_query(id,query).await {
            Ok(reply)=>reply,
            Err(original)=>self.get_system_query(id).await.map_err(|_|original)?,
        };
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(30);
        while matches!(reply.state.as_str(),"running"|"queued") && tokio::time::Instant::now()<deadline {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            reply=self.get_system_query(id).await?;
        }
        let Some(SystemQueryData::ExecutionContexts {entries,..})=reply.data.filter(|_|reply.state=="completed") else {
            return Err(BridgeError::FileTransfer("Pinned file account discovery did not complete; the original query remains observable".into()));
        };
        let selection=select_file_execution(expected,&entries)?;
        if cache.len()==32 {cache.remove(0);}
        cache.push((expected.clone(),selection));Ok(selection)
    }
    pub async fn system_query(
        &self,
        id: RequestId,
        query: SystemQuery,
    ) -> Result<SystemQueryReply, BridgeError> {
        query
            .validate()
            .map_err(|e| BridgeError::UnexpectedTaskResponse(e.into()))?;
        match self
            .task_request(DeviceTaskRequest::GetEnvironment {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
            })
            .await?
        {
            DeviceTaskResponse::Environment {
                context,
                system_query_schema_version,
                ..
            } if context.device_ref == self.device_ref => {
                require_capability(system_query_schema_version, &query)?;
            }
            response => return Err(unexpected_task_response(response)),
        }
        self.system_query_request(
            DeviceTaskRequest::SystemQuery {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
                query: query.clone(),
            },
            id,
            Some(query.kind()),
        )
        .await
    }
    pub async fn get_system_query(&self, id: RequestId) -> Result<SystemQueryReply, BridgeError> {
        self.system_query_request(
            DeviceTaskRequest::GetSystemQuery {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
            },
            id,
            None,
        )
        .await
    }
    pub async fn cancel_system_query(
        &self,
        id: RequestId,
    ) -> Result<SystemQueryReply, BridgeError> {
        self.system_query_request(
            DeviceTaskRequest::CancelSystemQuery {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
            },
            id,
            None,
        )
        .await
    }
    async fn system_query_request(
        &self,
        request: DeviceTaskRequest,
        id: RequestId,
        kind: Option<&str>,
    ) -> Result<SystemQueryReply, BridgeError> {
        match self.task_request(request).await? {
            DeviceTaskResponse::SystemQuery { reply }
                if valid(&reply, id) && kind.is_none_or(|k| k == reply.kind) =>
            {
                Ok(*reply)
            }
            response => Err(unexpected_task_response(response)),
        }
    }
}
fn select_file_execution(expected:&FileExecutionIdentity,entries:&[ExecutionContextEntry])->Result<ExecutionSelection,BridgeError> {
    let mut matches=entries.iter().filter(|entry|entry.validate().is_ok() && entry.identity.as_ref()==Some(&expected.identity) && entry.unavailable_reason.is_none()).filter_map(|entry|entry.selection.filter(|selection|selection.mode()==expected.identity.mode));
    let first=matches.next().ok_or_else(||BridgeError::FileTransfer("The original file account/session is unavailable; service fallback is forbidden".into()))?;
    if matches.next().is_some(){return Err(BridgeError::FileTransfer("Pinned file account discovery returned ambiguous identities".into()));}
    Ok(first)
}
fn require_capability(version: Option<u16>, query: &SystemQuery) -> Result<(), BridgeError> {
    if version.is_none_or(|v| v < query.required_version()) {
        return Err(BridgeError::UnsupportedSystemQuery);
    }
    Ok(())
}
fn valid(r: &SystemQueryReply, id: RequestId) -> bool {
    let notification_kind = matches!(
        r.kind.as_str(),
        "notification_status"
            | "notification_list"
            | "notification_wait"
            | "notification_open"
            | "notification_dismiss"
    );
    if notification_kind {
        match &r.data {
            Some(SystemQueryData::Notifications { snapshot }) if snapshot.validate().is_ok() => {}
            None if r.state != "completed" => {}
            _ => return false,
        }
    }
    let browser_kind = BrowserAction::ALL
        .iter()
        .any(|action| action.name() == r.kind);
    if ([
        "app_list",
        "app_launch",
        "app_open_file",
        "app_package_inspect",
        "app_install",
        "app_uninstall",
        "app_change_status",
        "app_change_resume",
    ]
    .contains(&r.kind.as_str())
        || browser_kind)
        && !valid_application(r)
    {
        return false;
    }
    if let Some(SystemQueryData::ExecutionContexts { entries, .. }) = &r.data {
        if entries.iter().any(|row| row.validate().is_err())
            || r.returned_count as usize != entries.len()
        {
            return false;
        }
    }
    if ["ui_query", "ui_get", "ui_action", "ui_wait"].contains(&r.kind.as_str()) {
        match &r.data {
            Some(SystemQueryData::Desktop { snapshot }) => {
                if snapshot.ui.as_ref().is_none_or(|ui| ui.validate().is_err()) {
                    return false;
                }
            }
            None if r.state != "completed" => {}
            _ => return false,
        }
    }
    if r.state == "completed" && r.kind.starts_with("system_log_") {
        let valid_log = match (r.kind.as_str(), r.data.as_ref()) {
            ("system_log_sources", Some(SystemQueryData::Logs { snapshot })) => {
                matches!(snapshot.as_ref(), SystemLogData::Sources { sources, .. } if r.returned_count as usize == sources.len())
            }
            ("system_log_read", Some(SystemQueryData::Logs { snapshot })) => {
                matches!(snapshot.as_ref(), SystemLogData::Page { events, .. } if r.returned_count as usize == events.len())
            }
            ("system_log_event_read", Some(SystemQueryData::Logs { snapshot })) => {
                matches!(snapshot.as_ref(), SystemLogData::EventChunk { .. })
                    && r.returned_count == 1
            }
            _ => false,
        };
        if !valid_log {
            return false;
        }
    }
    r.request_id == id
        && matches!(
            r.state.as_str(),
            "running"
                | "completed"
                | "failed"
                | "unconfirmed"
                | "interrupted"
                | "cancel_requested"
                | "cancelled"
        )
        && (notification_kind
            || browser_kind
            || matches!(
                r.kind.as_str(),
                "containers"
                    | "container"
                    | "container_logs"
                    | "container_control"
                    | "git_status"
                    | "git_diff"
                    | "git_log"
                    | "git_commit"
                    | "git_checkout"
                    | "git_fetch"
                    | "git_pull"
                    | "git_push"
                    | "monitors"
                    | "desktop_windows"
                    | "window_focus"
                    | "window_control"
                    | "type_text"
                    | "desktop_batch"
                    | "monitor_input"
                    | "ui_query"
                    | "ui_get"
                    | "ui_action"
                    | "ui_wait"
                    | "system_info"
                    | "disks"
                    | "processes"
                    | "process"
                    | "network_interfaces"
                    | "network_connections"
                    | "dns"
                    | "os_sessions"
                    | "execution_contexts"
                    | "app_list"
                    | "app_launch"
                    | "app_open_file"
                    | "app_package_inspect"
                    | "app_install"
                    | "app_uninstall"
                    | "app_change_status"
                    | "app_change_resume"
                    | "process_terminate"
                    | "services"
                    | "service"
                    | "service_control"
                    | "system_log_sources"
                    | "system_log_read"
                    | "system_log_event_read"
            ))
        && r.returned_count <= 1000
        && r.warnings.len() <= 8
        && serde_json::to_vec(r).is_ok_and(|b| b.len() <= MAX_SYSTEM_REPLY_BYTES)
        && (r.state != "completed"
            || notification_kind
            || (browser_kind
                && matches!(
                    r.data,
                    Some(SystemQueryData::Applications {
                        snapshot: AppSnapshot::Browser { .. }
                    })
                ))
            || matches!(
                (r.kind.as_str(), r.data.as_ref()),
                (
                    "app_list"
                        | "app_launch"
                        | "app_open_file"
                        | "app_package_inspect"
                        | "app_install"
                        | "app_uninstall"
                        | "app_change_status"
                        | "app_change_resume",
                    Some(SystemQueryData::Applications { .. })
                ) | (
                    "containers" | "container" | "container_logs" | "container_control",
                    Some(SystemQueryData::Container { .. })
                ) | (
                    "git_status"
                        | "git_diff"
                        | "git_log"
                        | "git_commit"
                        | "git_checkout"
                        | "git_fetch"
                        | "git_pull"
                        | "git_push",
                    Some(SystemQueryData::Git { .. })
                ) | (
                    "monitors"
                        | "desktop_windows"
                        | "window_focus"
                        | "window_control"
                        | "type_text"
                        | "desktop_batch"
                        | "monitor_input"
                        | "ui_query"
                        | "ui_get"
                        | "ui_action"
                        | "ui_wait",
                    Some(SystemQueryData::Desktop { .. })
                ) | (
                    "network_connections",
                    Some(SystemQueryData::Connections { .. })
                ) | ("dns", Some(SystemQueryData::Dns { .. }))
                    | ("os_sessions", Some(SystemQueryData::Sessions { .. }))
                    | (
                        "execution_contexts",
                        Some(SystemQueryData::ExecutionContexts { .. })
                    )
                    | ("services", Some(SystemQueryData::Services { .. }))
                    | ("service", Some(SystemQueryData::Service { .. }))
                    | (
                        "process_terminate",
                        Some(SystemQueryData::ProcessTermination { .. })
                    )
                    | (
                        "service_control",
                        Some(SystemQueryData::ServiceControl { .. })
                    )
                    | ("system_info", Some(SystemQueryData::Info { .. }))
                    | (
                        "system_log_sources" | "system_log_read" | "system_log_event_read",
                        Some(SystemQueryData::Logs { .. })
                    )
                    | ("disks", Some(SystemQueryData::Disks { .. }))
                    | ("processes", Some(SystemQueryData::Processes { .. }))
                    | ("process", Some(SystemQueryData::Process { .. }))
                    | ("network_interfaces", Some(SystemQueryData::Networks { .. }))
            ))
}

fn valid_application(reply: &SystemQueryReply) -> bool {
    let Some(data) = &reply.data else {
        return reply.state != "completed";
    };
    let (identity, instances, count) = match (reply.kind.as_str(), data) {
        (
            "app_package_inspect"
            | "app_install"
            | "app_uninstall"
            | "app_change_status"
            | "app_change_resume",
            SystemQueryData::Applications {
                snapshot: AppSnapshot::Package { result },
            },
        ) if result.validate().is_ok() => (&result.execution_identity, Vec::new(), 1),
        (
            "app_list",
            SystemQueryData::Applications {
                snapshot: AppSnapshot::List { snapshot },
            },
        ) => {
            if snapshot.apps.len() > 200 {
                return false;
            }
            (
                &snapshot.execution_identity,
                snapshot
                    .apps
                    .iter()
                    .filter_map(|app| app.instance.as_ref())
                    .collect::<Vec<_>>(),
                snapshot.apps.len() as u32,
            )
        }
        (
            "app_launch" | "app_open_file",
            SystemQueryData::Applications {
                snapshot: AppSnapshot::Action { result },
            },
        ) if result.request_accepted => (
            &result.execution_identity,
            result.instance.iter().collect(),
            1,
        ),
        (
            kind,
            SystemQueryData::Applications {
                snapshot: AppSnapshot::Browser { result },
            },
        ) if BrowserAction::ALL
            .iter()
            .any(|action| action.name() == kind) =>
        {
            (&result.execution_identity, Vec::new(), 1)
        }
        _ => return false,
    };
    identity.mode == ExecutionMode::DesktopUser
        && identity.validate().is_ok()
        && reply
            .execution_context
            .as_ref()
            .and_then(|context| context.identity.as_ref())
            == Some(identity)
        && reply.returned_count == count
        && instances.into_iter().all(|instance| {
            instance.process_id > 0
                && !instance.process_identity.is_empty()
                && instance
                    .account_id
                    .as_ref()
                    .is_none_or(|id| id == &identity.account_id)
                && instance
                    .session_id
                    .as_ref()
                    .is_none_or(|id| Some(id) == identity.session_id.as_ref())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_user_resolution_requires_the_same_account_session_and_one_native_reference() {
        let identity=ExecutionIdentity {mode:ExecutionMode::User,account_id:"S-1-5-21-123-456-789-1001".into(),account_name:"fixture\\user".into(),home:"C:\\Users\\fixture".into(),primary_group:None,session_id:Some("1".into()),logon_id:Some("login-1".into()),environment_source:ExecutionEnvironmentSource::NativeAccount};
        let expected=FileExecutionIdentity {os_family:OsFamily::Windows,path_style:PathStyle::Windows,identity:identity.clone()};
        let selection=ExecutionSelection::User {context_ref:ExecutionContextRef::new()};
        let entry=ExecutionContextEntry {mode:ExecutionMode::User,account_name:identity.account_name.clone(),account_id:Some(identity.account_id.clone()),session_id:identity.session_id.clone(),identity:Some(identity),selection:Some(selection),unavailable_reason:None};
        assert_eq!(select_file_execution(&expected,&[entry.clone()]).unwrap(),selection);
        assert!(select_file_execution(&expected,&[]).is_err());
        let mut changed=entry.clone();changed.identity.as_mut().unwrap().logon_id=Some("login-2".into());
        assert!(select_file_execution(&expected,&[changed]).is_err());
        let mut unavailable=entry.clone();unavailable.selection=None;unavailable.unavailable_reason=Some("session closed".into());
        assert!(select_file_execution(&expected,&[unavailable]).is_err());
        let mut duplicate=entry.clone();duplicate.selection=Some(ExecutionSelection::User {context_ref:ExecutionContextRef::new()});
        assert!(select_file_execution(&expected,&[entry,duplicate]).is_err());
    }
    #[test]
    fn notification_replies_accept_all_five_kinds_and_require_matching_bounded_data() {
        let mut reply = SystemQueryReply::pending(
            RequestId::new(),
            &SystemQuery::Notifications {
                query: NotificationRequest::Status {},
            },
        );
        for kind in [
            "notification_status",
            "notification_list",
            "notification_wait",
            "notification_open",
            "notification_dismiss",
        ] {
            reply.kind = kind.into();
            reply.state = "failed".into();
            reply.error = Some("control_paused".into());
            reply.data = None;
            assert!(valid(&reply, reply.request_id));
            reply.state = "completed".into();
            assert!(!valid(&reply, reply.request_id));
            reply.error = None;
            reply.data = Some(SystemQueryData::Notifications {
                snapshot: NotificationSnapshot {
                    access_granted: true,
                    listener_connected: true,
                    revision: 1,
                    entries: vec![],
                    outcome: "sampled".into(),
                    action_dispatched: false,
                    truncated: false,
                },
            });
            assert!(valid(&reply, reply.request_id));
            assert!(!valid(&reply, RequestId::new()));
            if let Some(SystemQueryData::Notifications { snapshot }) = &mut reply.data {
                snapshot.outcome = "unknown".into();
            }
            assert!(!valid(&reply, reply.request_id));
            reply.data = Some(SystemQueryData::Disks { entries: vec![] });
            assert!(!valid(&reply, reply.request_id));
        }
        reply.kind = "notification_unknown".into();
        reply.state = "failed".into();
        reply.data = None;
        assert!(!valid(&reply, reply.request_id));
    }
    #[test]
    fn applications_require_v12_and_reply_identity_matches_persisted_context() {
        let query = SystemQuery::Applications {
            execution: ExecutionSelection::DesktopUser {
                context_ref: ExecutionContextRef::new(),
            },
            query: AppQuery::Execute {
                request: AppActionRequest::Launch {
                    new_instance: false,
                    application: AppTarget::Id {
                        id: "fixture.app".into(),
                    },
                },
            },
        };
        for version in [None, Some(1), Some(11)] {
            assert!(require_capability(version, &query).is_err());
        }
        assert!(require_capability(Some(12), &query).is_ok());
        let context: ExecutionContext = serde_json::from_value(serde_json::json!({"os_family":"macos","os_name":"Mac OS","os_version":"fixture","architecture":"aarch64","execution_scope":"native","path_style":"posix","interpreter":null,"cwd":null,"environment_revision":"interactive-session-v1","identity":{"mode":"desktop_user","account_id":"uid:501","account_name":"fixture","home":"/Users/fixture","primary_group":20,"session_id":"100002","logon_id":null,"environment_source":"interactive_session"}})).unwrap();
        let identity = context.identity.clone().unwrap();
        let mut reply = SystemQueryReply::pending(RequestId::new(), &query);
        assert!(valid(&reply, reply.request_id));
        reply.state = "completed".into();
        reply.returned_count = 1;
        reply.execution_context = Some(context.clone());
        reply.data = Some(SystemQueryData::Applications {
            snapshot: AppSnapshot::Action {
                result: AppActionResult {
                    request_accepted: true,
                    instance: Some(AppInstance {
                        process_id: 42,
                        process_identity: "fixture:42".into(),
                        account_id: Some(identity.account_id.clone()),
                        session_id: identity.session_id.clone(),
                    }),
                    reused_instance: None,
                    execution_identity: identity.clone(),
                    window_ready: None,
                    notes: vec![],
                },
            },
        });
        assert!(valid(&reply, reply.request_id));
        reply.kind = "app_list".into();
        assert!(!valid(&reply, reply.request_id));
        reply.kind = "app_launch".into();
        reply.returned_count = 0;
        assert!(!valid(&reply, reply.request_id));
        reply.returned_count = 1;
        reply
            .execution_context
            .as_mut()
            .unwrap()
            .identity
            .as_mut()
            .unwrap()
            .account_id = "uid:502".into();
        assert!(!valid(&reply, reply.request_id));
        reply.execution_context = Some(context);
        if let Some(SystemQueryData::Applications {
            snapshot: AppSnapshot::Action { result },
        }) = &mut reply.data
        {
            result.instance.as_mut().unwrap().account_id = Some("uid:502".into());
        }
        assert!(!valid(&reply, reply.request_id));
        reply.state = "unconfirmed".into();
        reply.data = None;
        assert!(valid(&reply, reply.request_id));
    }
    #[test]
    fn execution_context_discovery_rejects_old_peers_without_blocking_existing_queries() {
        let query = SystemQuery::ExecutionContexts {
            user: None,
            include_system: false,
            limit: 10,
        };
        for version in [None, Some(1), Some(9)] {
            assert!(matches!(
                require_capability(version, &query),
                Err(BridgeError::UnsupportedSystemQuery)
            ));
        }
        assert!(require_capability(Some(10), &query).is_ok());
        assert!(require_capability(Some(9), &SystemQuery::Disks { limit: 10 }).is_ok());
    }
    #[test]
    fn capability_is_additive_and_reply_must_match_request_kind_identity_and_budget() {
        let json = serde_json::json!({"type":"environment","filesystem_schema_version":3,"context":{"device_ref":{"tenant_id":TenantId::from_u128(2),"device_id":DeviceId::from_u128(3)},"execution":{"os_family":"windows","os_name":"Windows","os_version":"fixture","architecture":"x86_64","execution_scope":"native","path_style":"windows","interpreter":null,"cwd":null,"environment_revision":"fixture"},"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"}});
        let r: DeviceTaskResponse = serde_json::from_value(json).unwrap();
        assert!(matches!(
            r,
            DeviceTaskResponse::Environment {
                system_query_schema_version: None,
                ..
            }
        ));
        let mut r = SystemQueryReply::pending(RequestId::new(), &SystemQuery::Disks { limit: 1 });
        assert!(valid(&r, r.request_id));
        assert!(!valid(&r, RequestId::new()));
        r.state = "completed".into();
        assert!(!valid(&r, r.request_id));
        r.data = Some(SystemQueryData::Disks { entries: vec![] });
        assert!(valid(&r, r.request_id));
        r.kind = "process".into();
        assert!(!valid(&r, r.request_id));
        r.kind = "disks".into();
        r.warnings = vec!["x".repeat(33 * 1024)];
        assert!(!valid(&r, r.request_id));
    }
    #[test]
    fn batch_reply_is_valid_with_desktop_data_and_rejects_wrong_kind_data() {
        let q = SystemQuery::Desktop {
            query: DesktopQuery::Batch {
                window_ref: RequestId::new().to_string(),
                actions: vec![DesktopAction::Focus {}],
                timeout_ms: 5000,
            },
        };
        let mut r = SystemQueryReply::pending(RequestId::new(), &q);
        assert!(valid(&r, r.request_id));
        r.state = "completed".into();
        r.data = Some(SystemQueryData::Desktop {
            snapshot: DesktopSnapshot::new("test".into(), "fake"),
        });
        assert!(valid(&r, r.request_id));
        r.kind = "monitor_input".into();
        assert!(valid(&r, r.request_id));
        r.data = Some(SystemQueryData::Disks { entries: vec![] });
        assert!(!valid(&r, r.request_id));
    }

    #[test]
    fn log_reply_accepts_matching_shape_and_rejects_mismatched_kind() {
        let query = SystemQuery::Logs {
            query: SystemLogQuery::Sources {
                scan_cursor: None,
                after: None,
                limit: 1,
            },
        };
        let mut reply = SystemQueryReply::pending(RequestId::new(), &query);
        assert!(valid(&reply, reply.request_id));
        reply.state = "completed".into();
        reply.data = Some(SystemQueryData::Logs {
            snapshot: Box::new(SystemLogData::Sources {
                sources: vec![],
                progress: None,
                next_source: None,
                total: 0,
            }),
        });
        assert!(valid(&reply, reply.request_id));
        reply.kind = "system_log_read".into();
        assert!(!valid(&reply, reply.request_id));
        reply.kind = "system_log_sources".into();
        reply.returned_count = 1;
        assert!(!valid(&reply, reply.request_id));
    }
}
