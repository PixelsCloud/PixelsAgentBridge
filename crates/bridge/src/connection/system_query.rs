use super::{AuthenticatedDeviceConnection, BridgeError, unexpected_task_response};
use pab_protocol::*;

impl AuthenticatedDeviceConnection {
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
                if system_query_schema_version.is_none_or(|v| v < query.required_version()) {
                    return Err(BridgeError::UnsupportedSystemQuery);
                }
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
fn valid(r: &SystemQueryReply, id: RequestId) -> bool {
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
        && matches!(
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
                | "system_info"
                | "disks"
                | "processes"
                | "process"
                | "network_interfaces"
                | "network_connections"
                | "dns"
                | "os_sessions"
                | "process_terminate"
                | "services"
                | "service"
                | "service_control"
        )
        && r.returned_count <= 1000
        && r.warnings.len() <= 8
        && serde_json::to_vec(r).is_ok_and(|b| b.len() <= MAX_SYSTEM_REPLY_BYTES)
        && (r.state != "completed"
            || matches!(
                (r.kind.as_str(), r.data.as_ref()),
                (
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
                        | "monitor_input",
                    Some(SystemQueryData::Desktop { .. })
                ) | (
                    "network_connections",
                    Some(SystemQueryData::Connections { .. })
                ) | ("dns", Some(SystemQueryData::Dns { .. }))
                    | ("os_sessions", Some(SystemQueryData::Sessions { .. }))
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
                    | ("disks", Some(SystemQueryData::Disks { .. }))
                    | ("processes", Some(SystemQueryData::Processes { .. }))
                    | ("process", Some(SystemQueryData::Process { .. }))
                    | ("network_interfaces", Some(SystemQueryData::Networks { .. }))
            ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
