use serde::{Deserialize, Serialize};

/// A log query is bound to the authenticated device and caller by Executor.
/// Cursors are opaque and must never be interpreted by Bridge or an Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemLogQuery {
    Sources {
        /// Optional scan cursor to list per-source progress for an active scan.
        scan_cursor: Option<String>,
        after: Option<String>,
        limit: u16,
    },
    Read {
        /// Omit on the first call; subsequent calls use the returned opaque cursor.
        cursor: Option<String>,
        /// Native source IDs. None discovers all sources; an empty list is invalid.
        sources: Option<Vec<String>>,
        start_unix_ms: Option<i64>,
        end_unix_ms: Option<i64>,
        mode: SystemLogMode,
        filter: Option<SystemLogFilter>,
        limit: u16,
    },
    EventRead {
        event_ref: String,
        offset: u64,
        max_bytes: u32,
    },
}

impl SystemLogQuery {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Sources { .. } => "system_log_sources",
            Self::Read { .. } => "system_log_read",
            Self::EventRead { .. } => "system_log_event_read",
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Sources {
                scan_cursor,
                after,
                limit,
            } => {
                if !(1..=128).contains(limit)
                    || scan_cursor
                        .as_ref()
                        .is_some_and(|s| s.is_empty() || s.len() > 512 || !s.is_ascii())
                    || after.as_ref().is_some_and(|s| {
                        s.is_empty() || s.len() > 512 || s.chars().any(char::is_control)
                    })
                {
                    Err("invalid log source page")
                } else {
                    Ok(())
                }
            }
            Self::Read {
                cursor,
                sources,
                start_unix_ms,
                end_unix_ms,
                filter,
                limit,
                ..
            } => {
                if !(1..=128).contains(limit) {
                    return Err("log page limit must be 1..128");
                }
                if let Some(cursor) = cursor {
                    if cursor.is_empty() || cursor.len() > 512 || !cursor.is_ascii() {
                        return Err("invalid log cursor");
                    }
                    if sources.is_some()
                        || start_unix_ms.is_some()
                        || end_unix_ms.is_some()
                        || filter.is_some()
                    {
                        return Err("cursor cannot be combined with a new log selection");
                    }
                    return Ok(());
                }
                if (*start_unix_ms)
                    .zip(*end_unix_ms)
                    .is_some_and(|(start, end)| start >= end)
                {
                    return Err("log start must precede end");
                }
                if sources.as_ref().is_some_and(|ids| {
                    ids.is_empty()
                        || ids.len() > 256
                        || ids.iter().any(|id| {
                            id.is_empty() || id.len() > 512 || id.chars().any(char::is_control)
                        })
                }) {
                    return Err("invalid log source selection");
                }
                if filter.as_ref().is_some_and(|f| !f.validate()) {
                    return Err("invalid log filter");
                }
                Ok(())
            }
            Self::EventRead {
                event_ref,
                max_bytes,
                ..
            } => {
                if event_ref.is_empty()
                    || event_ref.len() > 512
                    || !event_ref.is_ascii()
                    || !(1..=8192).contains(max_bytes)
                {
                    Err("invalid log event reference or byte limit")
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemLogMode {
    #[default]
    Range,
    Follow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemLogFilter {
    /// Literal case-sensitive substring, applied after the native record is read.
    pub message_contains: Option<String>,
    /// Exact native level/priority, with no lossy cross-platform mapping.
    pub native_level: Option<String>,
}

impl SystemLogFilter {
    pub fn validate(&self) -> bool {
        [self.message_contains.as_ref(), self.native_level.as_ref()]
            .into_iter()
            .flatten()
            .all(|value| {
                !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemLogIntegrity {
    Scanning,
    Complete,
    Partial,
    Gap,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLogSource {
    pub source_id: String,
    pub source_kind: String,
    pub source_locator: String,
    pub availability: String,
    pub reason: Option<String>,
    pub execution_identity: String,
    pub coverage_limit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLogSourceProgress {
    pub source_id: String,
    pub integrity: SystemLogIntegrity,
    pub native_position: Option<String>,
    pub scanned_count: u64,
    pub matched_count: u64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLogEvent {
    pub device_ref: crate::DeviceRef,
    pub event_ref: String,
    pub source_id: String,
    pub source_kind: String,
    pub source_locator: String,
    pub execution_identity: String,
    pub native_position: Option<String>,
    pub timestamp_unix_ms: i64,
    pub native_level: Option<String>,
    pub message_preview: String,
    pub total_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemLogData {
    Sources {
        sources: Vec<SystemLogSource>,
        progress: Option<Vec<SystemLogSourceProgress>>,
        next_source: Option<String>,
        total: u32,
    },
    Page {
        events: Vec<SystemLogEvent>,
        sources: Vec<SystemLogSourceProgress>,
        source_total: u32,
        source_progress_truncated: bool,
        integrity: SystemLogIntegrity,
        /// Follow only: following, catching_up, lagging, or stopped.
        capture_state: Option<String>,
        cursor: Option<String>,
        has_more: bool,
        /// Age of the oldest event committed beyond this page's cursor.
        /// None does not mean a native source has no unscanned backlog.
        oldest_unread_age_ms: Option<u64>,
    },
    EventChunk {
        event_ref: String,
        offset: u64,
        next_offset: u64,
        total_bytes: u64,
        data_base64: String,
        sha256: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_is_the_only_selection_on_resume() {
        let query = SystemLogQuery::Read {
            cursor: Some("opaque".into()),
            sources: Some(vec!["eventlog:System".into()]),
            start_unix_ms: None,
            end_unix_ms: None,
            mode: SystemLogMode::Range,
            filter: None,
            limit: 32,
        };
        assert!(query.validate().is_err());
    }

    #[test]
    fn invalid_ranges_and_unbounded_reads_are_rejected() {
        let query = SystemLogQuery::Read {
            cursor: None,
            sources: None,
            start_unix_ms: Some(10),
            end_unix_ms: Some(10),
            mode: SystemLogMode::Range,
            filter: None,
            limit: 32,
        };
        assert!(query.validate().is_err());
        assert!(
            SystemLogQuery::EventRead {
                event_ref: "a".into(),
                offset: 0,
                max_bytes: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn log_filter_text_is_not_saved_in_task_history() {
        let query = crate::SystemQuery::Logs {
            query: SystemLogQuery::Read {
                cursor: None,
                sources: Some(vec!["journal:local".into()]),
                start_unix_ms: Some(1),
                end_unix_ms: Some(2),
                mode: SystemLogMode::Range,
                filter: Some(SystemLogFilter {
                    message_contains: Some("private-token-123".into()),
                    native_level: None,
                }),
                limit: 1,
            },
        };
        let saved = serde_json::to_string(&query.persistence_form()).unwrap();
        assert!(!saved.contains("private-token-123"));
        assert!(saved.contains("blake3:"));
        assert_eq!(query.persistence_form(), query.persistence_form());
    }
}
