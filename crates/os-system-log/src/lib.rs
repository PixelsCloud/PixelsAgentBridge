//! Native, read-only OS log adapters. Scanning, persistence and caller binding
//! belong to Executor; an adapter never claims completeness on its own.

use pab_protocol::SystemLogSource;
use thiserror::Error;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod pixels;
#[cfg(windows)]
mod windows;

#[derive(Debug, Error)]
pub enum LogError {
    #[error("source unavailable: {0}")]
    Unavailable(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("native log query failed: {0}")]
    Native(String),
    #[error("source continuity cannot be verified: {0}")]
    Unverified(String),
    #[error("source position was lost: {0}")]
    Gap(String),
}

/// An adapter page is bounded by count and raw bytes. `next_native_position`
/// must represent the last *scanned* entry, even if nothing matched a filter.
#[derive(Debug, Clone)]
pub struct NativePage {
    pub events: Vec<NativeRecord>,
    pub next_native_position: Option<String>,
    pub scanned_count: u64,
    pub exhausted: bool,
    pub verifiable: bool,
}

#[derive(Debug, Clone)]
pub struct NativeRecord {
    pub source_id: String,
    pub source_kind: String,
    pub source_locator: String,
    pub native_position: String,
    pub timestamp_unix_ms: i64,
    pub native_level: Option<String>,
    /// Native content, never silently truncated or decoded with replacement.
    pub raw: Vec<u8>,
    /// Human-readable message if the native record has one.
    pub message: Option<String>,
}

pub fn sources(identity: &str) -> Result<Vec<SystemLogSource>, LogError> {
    #[cfg(windows)]
    {
        return windows::sources(identity);
    }
    #[cfg(target_os = "linux")]
    {
        return linux::sources(identity);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::sources(identity);
    }
    #[allow(unreachable_code)]
    Err(LogError::Unavailable("unsupported operating system".into()))
}

pub fn scan(
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
) -> Result<NativePage, LogError> {
    #[cfg(windows)]
    {
        return windows::scan(
            source,
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
        );
    }
    #[cfg(target_os = "linux")]
    {
        return linux::scan(
            source,
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
        );
    }
    #[cfg(target_os = "macos")]
    {
        return macos::scan(
            source,
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
        );
    }
    #[allow(unreachable_code)]
    Err(LogError::Unavailable("unsupported operating system".into()))
}

/// A selective query advances through unrelated native rows without returning
/// each of them to the persistent scan store.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn scan_with_message_filter(
    source: &str,
    after: Option<&str>,
    start_unix_ms: i64,
    end_unix_ms: i64,
    max_records: usize,
    max_bytes: usize,
    message_contains: Option<&str>,
) -> Result<NativePage, LogError> {
    #[cfg(target_os = "macos")]
    {
        macos::scan_filtered(
            source,
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
            message_contains,
        )
    }
    #[cfg(target_os = "linux")]
    {
        linux::scan_filtered(
            source,
            after,
            start_unix_ms,
            end_unix_ms,
            max_records,
            max_bytes,
            message_contains,
        )
    }
}
