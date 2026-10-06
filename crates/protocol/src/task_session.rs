use serde::{Deserialize, Serialize};

use crate::{
    DirectoryPage, ExpectedEnvironment, OperatorRef, OutputChunk, OutputRange, OutputStream,
    RequestId, ScreenshotMeta, TargetContext, TaskEvent, TaskRef, TaskSnapshot, WindowList,
};

pub const DEVICE_TASK_SCHEMA_VERSION: u16 = 1;
pub const MAX_COMMAND_PROGRAM_BYTES: usize = 4 * 1024;
pub const MAX_COMMAND_ARGUMENTS: usize = 1024;
pub const MAX_COMMAND_ARGUMENT_BYTES: usize = 16 * 1024;
// JSON encodes bytes as numbers, so leave ample room inside the 64 KiB PAB frame.
pub const MAX_OUTPUT_READ_BYTES: u32 = 8 * 1024;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 4 * 1024;
pub const MAX_TERMINAL_READ_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopInputEvent {
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: DesktopMouseButton,
        down: bool,
    },
    MouseWheel {
        delta: i16,
    },
    Key {
        virtual_key: u16,
        down: bool,
    },
    SecureAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandTaskSpec {
    #[serde(default, skip_serializing_if = "CommandOptions::is_default")]
    pub options: CommandOptions,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub expected_environment: ExpectedEnvironment,
    pub display_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandOptions {
    #[serde(default, skip_serializing_if = "crate::ExecutionSelection::is_service")]
    pub execution: crate::ExecutionSelection,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    pub stdin_text: Option<String>,
    pub timeout_ms: Option<u64>,
}

impl CommandOptions {
    pub fn required_version(&self) -> u16 {
        if !self.execution.is_service() {
            3
        } else if self.is_default() {
            1
        } else {
            2
        }
    }
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if matches!(
            self.execution,
            crate::ExecutionSelection::DesktopUser { .. }
        ) {
            return Err(
                "command requires service or user execution; desktop_user is for desktop operations",
            );
        }
        if self.env.len() > 64
            || self.env.iter().any(|(k, v)| {
                k.is_empty() || k.len() > 256 || k.contains(['=', '\0']) || v.contains('\0')
            })
        {
            return Err("invalid command environment");
        }
        if !self.execution.is_service()
            && self.env.keys().any(|key| {
                [
                    "HOME",
                    "USER",
                    "LOGNAME",
                    "USERNAME",
                    "USERDOMAIN",
                    "USERPROFILE",
                    "HOMEDRIVE",
                    "HOMEPATH",
                    "APPDATA",
                    "LOCALAPPDATA",
                    "UID",
                    "EUID",
                ]
                .iter()
                .any(|reserved| key.eq_ignore_ascii_case(reserved))
            })
        {
            return Err("user execution cannot override native account environment");
        }
        if self
            .env
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 8192
        {
            return Err("command environment exceeds 8192 bytes");
        }
        if self.stdin_text.as_ref().is_some_and(|v| v.len() > 16384) {
            return Err("stdin_text exceeds 16384 UTF-8 bytes");
        }
        if self
            .timeout_ms
            .is_some_and(|v| !(1..=86400000).contains(&v))
        {
            return Err("timeout_ms must be between 1 and 86400000");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_context: Option<crate::ExecutionContext>,
    pub request_id: RequestId,
    pub initiated_by: OperatorRef,
    pub direction: String,
    pub path: String,
    pub state: String,
    pub offset: u64,
    pub size: u64,
    pub sha256: Option<String>,
    pub finished_at_unix_ms: Option<i64>,
    pub message: Option<String>,
    /// None on older peers or while publication cannot yet be proven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceTaskRequest {
    TransferFile {
        schema_version: u16,
        request: crate::FileTransferRequest,
    },
    SystemQuery {
        schema_version: u16,
        request_id: RequestId,
        query: crate::SystemQuery,
    },
    GetSystemQuery {
        schema_version: u16,
        request_id: RequestId,
    },
    CancelSystemQuery {
        schema_version: u16,
        request_id: RequestId,
    },
    FileSystem {
        schema_version: u16,
        request: crate::FileSystemRequest,
    },
    CancelFileSystem {
        schema_version: u16,
        request_id: RequestId,
    },
    GetFileSystem {
        schema_version: u16,
        request_id: RequestId,
    },
    GetEnvironment {
        schema_version: u16,
    },
    GetPresence {
        schema_version: u16,
    },
    SubmitCommand {
        schema_version: u16,
        request_id: RequestId,
        command: CommandTaskSpec,
    },
    GetTask {
        schema_version: u16,
        task_ref: TaskRef,
    },
    GetTransfer {
        schema_version: u16,
        request_id: RequestId,
    },
    ListDirectory {
        schema_version: u16,
        request_id: RequestId,
        path: String,
        after: Option<String>,
        limit: u16,
    },
    ListWindows {
        schema_version: u16,
        request_id: RequestId,
    },
    CaptureScreenshotV2 {
        schema_version: u16,
        request_id: RequestId,
        options: crate::ScreenshotOptions,
    },
    CaptureScreenshot {
        schema_version: u16,
        request_id: RequestId,
    },
    DesktopInput {
        schema_version: u16,
        request_id: RequestId,
        event: DesktopInputEvent,
    },
    OpenTerminal {
        schema_version: u16,
        request_id: RequestId,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "crate::ExecutionSelection::is_service")]
        execution: crate::ExecutionSelection,
    },
    TerminalInput {
        schema_version: u16,
        session_id: RequestId,
        sequence: u64,
        size: u16,
    },
    TerminalRead {
        schema_version: u16,
        session_id: RequestId,
        offset: u64,
        limit: u16,
    },
    TerminalResize {
        schema_version: u16,
        session_id: RequestId,
        sequence: u64,
        cols: u16,
        rows: u16,
    },
    TerminalClose {
        schema_version: u16,
        session_id: RequestId,
        sequence: u64,
    },
    ReadOutput {
        schema_version: u16,
        task_ref: TaskRef,
        stream: OutputStream,
        offset: u64,
        max_bytes: u32,
    },
    Subscribe {
        schema_version: u16,
        task_ref: TaskRef,
        after_event_seq: u64,
        stdout_offset: u64,
        stderr_offset: u64,
    },
    Cancel {
        schema_version: u16,
        task_ref: TaskRef,
        reason: String,
    },
    UploadFile {
        schema_version: u16,
        #[serde(default)]
        request_id: RequestId,
        path: String,
        size: u64,
        sha256: String,
        #[serde(default)]
        overwrite: bool,
    },
    DownloadFile {
        schema_version: u16,
        #[serde(default)]
        request_id: RequestId,
        path: String,
        offset: u64,
        #[serde(default)]
        overwrite: bool,
    },
}

impl DeviceTaskRequest {
    pub const fn schema_version(&self) -> u16 {
        match self {
            Self::TransferFile { schema_version, .. }
            | Self::SystemQuery { schema_version, .. }
            | Self::GetSystemQuery { schema_version, .. }
            | Self::CancelSystemQuery { schema_version, .. }
            | Self::FileSystem { schema_version, .. }
            | Self::CancelFileSystem { schema_version, .. }
            | Self::GetFileSystem { schema_version, .. }
            | Self::GetEnvironment { schema_version }
            | Self::GetPresence { schema_version }
            | Self::SubmitCommand { schema_version, .. }
            | Self::GetTask { schema_version, .. }
            | Self::GetTransfer { schema_version, .. }
            | Self::ListDirectory { schema_version, .. }
            | Self::ListWindows { schema_version, .. }
            | Self::CaptureScreenshot { schema_version, .. }
            | Self::CaptureScreenshotV2 { schema_version, .. }
            | Self::DesktopInput { schema_version, .. }
            | Self::OpenTerminal { schema_version, .. }
            | Self::TerminalInput { schema_version, .. }
            | Self::TerminalRead { schema_version, .. }
            | Self::TerminalResize { schema_version, .. }
            | Self::TerminalClose { schema_version, .. }
            | Self::ReadOutput { schema_version, .. }
            | Self::Subscribe { schema_version, .. }
            | Self::Cancel { schema_version, .. }
            | Self::UploadFile { schema_version, .. }
            | Self::DownloadFile { schema_version, .. } => *schema_version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTaskErrorCode {
    InvalidRequest,
    EnvironmentChanged,
    NotFound,
    RequestConflict,
    NotCancellable,
    StorageUnavailable,
    Internal,
    AccessDenied,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceTaskResponse {
    TransferAccepted {
        snapshot: TransferSnapshot,
    },
    SystemQuery {
        reply: Box<crate::SystemQueryReply>,
    },
    FileSystem {
        reply: Box<crate::FileSystemReply>,
    },
    Environment {
        context: Box<TargetContext>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_schema_version: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_schema_version: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filesystem_schema_version: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer_schema_version: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_query_schema_version: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screenshot_schema_version: Option<u16>,
    },
    Presence {
        active_operators: u16,
    },
    Submitted {
        snapshot: Box<TaskSnapshot>,
    },
    Snapshot {
        snapshot: Box<TaskSnapshot>,
    },
    Transfer {
        snapshot: TransferSnapshot,
    },
    Directory {
        page: DirectoryPage,
    },
    Windows {
        list: WindowList,
    },
    Screenshot {
        meta: ScreenshotMeta,
    },
    DesktopInputApplied {
        request_id: RequestId,
    },
    TerminalOpened {
        session_id: RequestId,
        shell: String,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<crate::ExecutionIdentity>,
    },
    TerminalAcknowledged {
        session_id: RequestId,
        sequence: u64,
    },
    TerminalOutput {
        session_id: RequestId,
        retained_from: u64,
        offset: u64,
        next_offset: u64,
        size: u16,
        ended: bool,
    },
    TerminalClosed {
        session_id: RequestId,
    },
    Events {
        events: Vec<TaskEvent>,
    },
    Output {
        chunk: OutputChunk,
        range: OutputRange,
    },
    Event {
        event: TaskEvent,
    },
    OutputChanged {
        task_ref: TaskRef,
        stream: OutputStream,
        range: OutputRange,
    },
    CaughtUp {
        snapshot: Box<TaskSnapshot>,
    },
    CancelAccepted {
        snapshot: Box<TaskSnapshot>,
    },
    FileReady {
        size: u64,
        offset: u64,
        sha256: String,
    },
    FileProgress {
        offset: u64,
    },
    FileComplete {
        size: u64,
        sha256: String,
    },
    Error {
        code: DeviceTaskErrorCode,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use crate::{DeviceId, DeviceRef, TaskId, TenantId};

    use super::*;

    #[test]
    fn user_execution_is_explicit_strict_and_requires_command_v3() {
        let legacy: CommandOptions = serde_json::from_value(
            serde_json::json!({"env":{},"stdin_text":null,"timeout_ms":null}),
        )
        .unwrap();
        assert!(legacy.execution.is_service());
        assert_eq!(legacy.required_version(), 1);
        let selected:CommandOptions=serde_json::from_value(serde_json::json!({"execution":{"mode":"user","context_ref":crate::ExecutionContextRef::new()},"env":{},"stdin_text":null,"timeout_ms":null})).unwrap();
        assert_eq!(selected.required_version(), 3);
        assert!(selected.validate().is_ok());
        for invalid in [
            serde_json::json!({"mode":"service","context_ref":crate::ExecutionContextRef::new()}),
            serde_json::json!({"mode":"user","username":"fake"}),
        ] {
            assert!(serde_json::from_value::<CommandOptions>(serde_json::json!({"execution":invalid,"env":{},"stdin_text":null,"timeout_ms":null})).is_err());
        }
    }

    #[test]
    fn selected_account_environment_cannot_be_replaced_by_caller() {
        let mut options = CommandOptions {
            execution: crate::ExecutionSelection::User {
                context_ref: crate::ExecutionContextRef::new(),
            },
            ..Default::default()
        };
        for key in [
            "HOME",
            "user",
            "UserProfile",
            "APPDATA",
            "LOCALAPPDATA",
            "USERDOMAIN",
            "HOMEDRIVE",
            "HOMEPATH",
            "LOGNAME",
            "USERNAME",
            "UID",
            "EUID",
        ] {
            options.env.clear();
            options.env.insert(key.into(), "other-account".into());
            assert!(options.validate().is_err(), "{key}");
        }
        options.env.clear();
        options
            .env
            .insert("PATH".into(), "/custom/bin:/usr/bin".into());
        assert!(options.validate().is_ok());
        options.execution = crate::ExecutionSelection::Service {};
        options.env.insert("HOME".into(), "/legacy".into());
        assert!(options.validate().is_ok());
    }

    #[test]
    fn legacy_commands_default_options_and_enhanced_options_are_byte_bounded() {
        let value = serde_json::json!({"program":"echo", "args":[], "cwd":null,
            "expected_environment":{"os_family":"windows","environment_revision":"test"},
            "display_summary":"echo"});
        let command: CommandTaskSpec = serde_json::from_value(value.clone()).unwrap();
        assert!(command.options.is_default());
        assert_eq!(serde_json::to_value(command).unwrap(), value);
        let mut options = CommandOptions::default();
        options.env.insert("BAD=KEY".into(), "x".into());
        assert!(options.validate().is_err());
        options.env.clear();
        options.stdin_text = Some("中".repeat(5462));
        assert!(options.validate().is_err());
        options.stdin_text = Some("中".repeat(5461));
        assert!(options.validate().is_ok());
        options.timeout_ms = Some(0);
        assert!(options.validate().is_err());
        options.timeout_ms = Some(86400000);
        options.env.insert("KEY".into(), "x".repeat(8192));
        assert!(options.validate().is_err());
        options.env.clear();
        assert!(options.validate().is_ok());
    }

    #[test]
    fn task_requests_are_explicitly_versioned() {
        let request = DeviceTaskRequest::GetTask {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            task_ref: TaskRef {
                device_ref: DeviceRef {
                    tenant_id: TenantId::from_u128(2),
                    device_id: DeviceId::from_u128(3),
                },
                task_id: TaskId::from_u128(4),
            },
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: DeviceTaskRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.schema_version(), DEVICE_TASK_SCHEMA_VERSION);
    }

    #[test]
    fn transfer_lookup_round_trips_with_authenticated_actor() {
        let request_id = RequestId::from_u128(8);
        let request = DeviceTaskRequest::GetTransfer {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id,
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<DeviceTaskRequest>(&encoded).unwrap(),
            request
        );
        let response = DeviceTaskResponse::Transfer {
            snapshot: TransferSnapshot {
                execution_context: None,
                request_id,
                initiated_by: crate::OperatorRef::account(
                    crate::UserId::from_u128(9),
                    crate::EndpointKey::new([9; 32]),
                ),
                direction: "receive".to_owned(),
                path: "/tmp/file.bin".to_owned(),
                state: "completed".to_owned(),
                offset: 4,
                size: 4,
                sha256: Some("a".repeat(64)),
                finished_at_unix_ms: Some(1_000),
                message: None,
                published: Some(true),
            },
        };
        let encoded = serde_json::to_vec(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<DeviceTaskResponse>(&encoded).unwrap(),
            response
        );
    }
}
