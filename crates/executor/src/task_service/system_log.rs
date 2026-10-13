use super::TaskService;
use pab_os_system_log::LogError;
use pab_protocol::{
    OperatorRef, RequestId, SystemLogData, SystemLogIntegrity, SystemLogMode, SystemLogQuery,
    SystemLogSource, SystemQuery, SystemQueryData, SystemQueryReply,
};
use pab_system_log_store::{LogStore, NewScan};
use std::path::Path;

impl TaskService {
    pub(super) async fn log_query(
        &self,
        actor: OperatorRef,
        id: RequestId,
        query: &SystemLogQuery,
    ) -> SystemQueryReply {
        let mut reply = SystemQueryReply::pending(
            id,
            &SystemQuery::Logs {
                query: query.clone(),
            },
        );
        match self.log_query_data(actor, query).await {
            Ok(data) => {
                reply.returned_count = match &data {
                    SystemLogData::Sources { sources, .. } => sources.len() as u32,
                    SystemLogData::Page { events, .. } => events.len() as u32,
                    SystemLogData::EventChunk { .. } => 1,
                };
                reply.data = Some(SystemQueryData::Logs {
                    snapshot: Box::new(data),
                });
                reply.state = "completed".into();
                reply.sampled_at_unix_ms = super::unix_millis().ok();
            }
            Err(error) => {
                reply.state = "failed".into();
                reply.error = Some(error);
            }
        }
        pab_platform::bound_system_reply(&mut reply);
        reply
    }

    async fn log_query_data(
        &self,
        actor: OperatorRef,
        query: &SystemLogQuery,
    ) -> Result<SystemLogData, String> {
        match query {
            SystemLogQuery::Sources {
                scan_cursor,
                after,
                limit,
            } => {
                let (sources, progress, next_source, total) = if let Some(cursor) = scan_cursor {
                    let (sources, progress, next, total) = self
                        .log_store
                        .source_progress_page(actor, cursor, after.as_deref(), *limit)
                        .await
                        .map_err(|e| e.to_string())?;
                    (sources, Some(progress), next, total)
                } else {
                    let mut all = native_sources(
                        &self.log_root,
                        self.execution_context
                            .identity
                            .as_ref()
                            .map(|i| i.account_name.as_str())
                            .unwrap_or("Executor service"),
                    )
                    .await?;
                    all.sort_by(|a, b| a.source_id.cmp(&b.source_id));
                    let total = all.len() as u32;
                    let filtered = all
                        .into_iter()
                        .filter(|source| {
                            after
                                .as_ref()
                                .is_none_or(|previous| source.source_id > *previous)
                        })
                        .collect::<Vec<_>>();
                    let mut sources = Vec::new();
                    let mut used = 0_usize;
                    for source in filtered.iter().take(usize::from(*limit)) {
                        let size = serde_json::to_vec(source).map_err(|e| e.to_string())?.len();
                        if !sources.is_empty() && used + size > 20 * 1024 {
                            break;
                        }
                        used += size;
                        sources.push(source.clone());
                    }
                    let has_more = filtered.len() > sources.len();
                    let next = if has_more {
                        sources.last().map(|s| s.source_id.clone())
                    } else {
                        None
                    };
                    (sources, None, next, total)
                };
                Ok(SystemLogData::Sources {
                    sources,
                    progress,
                    next_source,
                    total,
                })
            }
            SystemLogQuery::EventRead {
                event_ref,
                offset,
                max_bytes,
            } => self
                .log_store
                .read_event(actor, event_ref, *offset, *max_bytes)
                .await
                .map_err(|e| e.to_string()),
            SystemLogQuery::Read {
                cursor,
                sources,
                start_unix_ms,
                end_unix_ms,
                mode,
                filter,
                limit,
            } => {
                let _guard = self.log_scan_lock.lock().await;
                let (scan_id, page_cursor) = if let Some(cursor) = cursor {
                    (
                        self.log_store
                            .scan_id_for_cursor(actor, cursor)
                            .await
                            .map_err(|e| e.to_string())?,
                        cursor.clone(),
                    )
                } else {
                    let now = super::unix_millis().map_err(|e| e.to_string())?;
                    let start = start_unix_ms.unwrap_or(now.saturating_sub(24 * 60 * 60 * 1_000));
                    let end = end_unix_ms.unwrap_or(if *mode == SystemLogMode::Follow {
                        i64::MAX
                    } else {
                        now
                    });
                    if start >= end {
                        return Err("log start must precede end".into());
                    }
                    let identity = self
                        .execution_context
                        .identity
                        .as_ref()
                        .map(|identity| identity.account_name.as_str())
                        .unwrap_or("Executor service");
                    let inventory = native_sources(&self.log_root, identity).await?;
                    let selected = select_sources(inventory, sources.as_deref())?;
                    self.log_store
                        .create_scan(NewScan {
                            actor,
                            device: self.device_ref,
                            identity,
                            sources: &selected,
                            start_unix_ms: start,
                            end_unix_ms: end,
                            mode: *mode,
                            filter: filter.as_ref(),
                        })
                        .await
                        .map_err(|e| e.to_string())?
                };
                scan_one_round(&self.log_store, &self.log_root, actor, &scan_id).await?;
                self.log_store
                    .read_page(actor, &page_cursor, *limit)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }
}

fn select_sources(
    inventory: Vec<SystemLogSource>,
    requested: Option<&[String]>,
) -> Result<Vec<SystemLogSource>, String> {
    if let Some(ids) = requested {
        let mut selected = Vec::new();
        for id in ids {
            if selected
                .iter()
                .any(|source: &SystemLogSource| &source.source_id == id)
            {
                continue;
            }
            let source = inventory
                .iter()
                .find(|source| &source.source_id == id)
                .ok_or_else(|| format!("unknown log source: {id}; call pab_system_log_sources"))?;
            selected.push(source.clone());
        }
        return Ok(selected);
    }
    if inventory.len() > 256 {
        return Err(
            "too many log sources for an implicit selection; list sources and choose IDs".into(),
        );
    }
    // Both macOS readers describe the same unified log. Keep the CLI reader
    // addressable for existing cursors and exact log-show NDJSON, but avoid
    // returning duplicate events when the caller selects sources implicitly.
    let native_readable = inventory.iter().any(|source| {
        source.source_id == "unified:oslogstore" && source.availability == "readable"
    });
    let cli_readable = inventory.iter().any(|source| {
        source.source_id == "unified:local" && source.availability == "readable"
    });
    Ok(inventory
        .into_iter()
        .filter(|source| {
            !(native_readable && source.source_id == "unified:local"
                || !native_readable && cli_readable && source.source_id == "unified:oslogstore")
        })
        .collect())
}

async fn native_sources(root: &Path, identity: &str) -> Result<Vec<SystemLogSource>, String> {
    let identity = identity.to_owned();
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut sources = match pab_os_system_log::sources(&identity) {
            Ok(sources) => sources,
            Err(error) => vec![unavailable_source(
                "system:unavailable",
                "native_system_log",
                &identity,
                &error,
            )],
        };
        match pab_os_system_log::pixels::sources(&root, &identity) {
            Ok(files) => sources.extend(files),
            Err(error) => sources.push(unavailable_source(
                "pixels-file:directory",
                "pixels_rotating_file",
                &identity,
                &error,
            )),
        }
        sources
    })
    .await
    .map_err(|e| format!("log source enumerator stopped: {e}"))
}

fn unavailable_source(id: &str, kind: &str, identity: &str, error: &LogError) -> SystemLogSource {
    SystemLogSource {
        source_id: id.into(), source_kind: kind.into(), source_locator: id.into(),
        availability: if matches!(error, LogError::PermissionDenied(_)) { "permission_denied" } else { "unavailable" }.into(),
        reason: Some(error.to_string()), execution_identity: identity.into(),
        coverage_limit: Some("This source could not be enumerated; use a native command or restore access before relying on its coverage".into()),
    }
}

async fn scan_one_round(
    store: &LogStore,
    root: &Path,
    actor: OperatorRef,
    scan_id: &str,
) -> Result<(), String> {
    let (start, end, _mode, _filter) = store
        .scan_spec(actor, scan_id)
        .await
        .map_err(|e| e.to_string())?;
    for (source, after, state) in store
        .scan_sources(actor, scan_id)
        .await
        .map_err(|e| e.to_string())?
    {
        if state != SystemLogIntegrity::Scanning || source.availability != "readable" {
            continue;
        }
        let source_id = source.source_id.clone();
        let previous = after.clone();
        let root = root.to_owned();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let message_contains = _filter
            .as_ref()
            .and_then(|filter| filter.message_contains.clone());
        let scan_result = tokio::task::spawn_blocking(move || {
            if source_id.starts_with("pixels-file:") {
                pab_os_system_log::pixels::scan(
                    &root,
                    &source_id,
                    previous.as_deref(),
                    start,
                    end,
                    32,
                    256 * 1024,
                )
            } else {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                if source_id == "unified:local"
                    || source_id == "unified:oslogstore"
                    || source_id == "journal:local"
                {
                    return pab_os_system_log::scan_with_message_filter(
                        &source_id,
                        previous.as_deref(),
                        start,
                        end,
                        32,
                        256 * 1024,
                        message_contains.as_deref(),
                    );
                }
                pab_os_system_log::scan(&source_id, previous.as_deref(), start, end, 32, 256 * 1024)
            }
        })
        .await
        .map_err(|e| format!("native log worker stopped: {e}"))?;
        match scan_result {
            Ok(page) => {
                if let Err(error) = store
                    .append_page(actor, scan_id, &source.source_id, after.as_deref(), &page)
                    .await
                {
                    let reason = error.to_string();
                    store
                        .mark_source(
                            actor,
                            scan_id,
                            &source.source_id,
                            SystemLogIntegrity::Partial,
                            &reason,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            Err(error) => {
                let state = match error {
                    LogError::Gap(_) => SystemLogIntegrity::Gap,
                    LogError::Unverified(_) => SystemLogIntegrity::Unverified,
                    _ => SystemLogIntegrity::Partial,
                };
                store
                    .mark_source(actor, scan_id, &source.source_id, state, &error.to_string())
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod source_selection_tests {
    use super::*;

    fn source(id: &str, availability: &str) -> SystemLogSource {
        SystemLogSource {
            source_id: id.into(),
            source_kind: "macos_unified_log".into(),
            source_locator: id.into(),
            availability: availability.into(),
            reason: None,
            execution_identity: "root".into(),
            coverage_limit: None,
        }
    }

    #[test]
    fn implicit_macos_selection_avoids_duplicate_unified_log_sources() {
        let inventory = vec![
            source("unified:local", "readable"),
            source("unified:oslogstore", "readable"),
            source("pixels-file:executor", "readable"),
        ];
        let selected = select_sources(inventory.clone(), None).unwrap();
        assert_eq!(
            selected.iter().map(|item| item.source_id.as_str()).collect::<Vec<_>>(),
            ["unified:oslogstore", "pixels-file:executor"]
        );
        let explicit = select_sources(inventory, Some(&["unified:local".into()])).unwrap();
        assert_eq!(explicit[0].source_id, "unified:local");
    }

    #[test]
    fn implicit_macos_selection_uses_cli_when_native_is_unavailable() {
        let selected = select_sources(
            vec![
                source("unified:local", "readable"),
                source("unified:oslogstore", "unavailable"),
            ],
            None,
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source_id, "unified:local");
    }
}
