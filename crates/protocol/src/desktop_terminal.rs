//! Human Desktop terminals are deliberately separate from the MCP terminal protocol.
use crate::{ExecutionIdentity, ExecutionSelection, RequestId};
use serde::{Deserialize, Serialize};

pub const DESKTOP_TERMINAL_SCHEMA_VERSION: u16 = 1;
pub const DESKTOP_TERMINAL_CHUNK: usize = 16 * 1024;
pub const DESKTOP_TERMINAL_CREDIT: u64 = 256 * 1024;
pub const DESKTOP_TERMINAL_INPUT: usize = 4096;

pub fn validate_desktop_terminal_size(cols: u16, rows: u16) -> Result<(), &'static str> {
    if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
        return Err("terminal size must be 20..500 columns and 5..200 rows");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopTerminalRequest {
    Open {
        version: u16,
        session_id: RequestId,
        execution: ExecutionSelection,
        cols: u16,
        rows: u16,
    },
    Output {
        session_id: RequestId,
    },
    Control {
        session_id: RequestId,
    },
    Close {
        session_id: RequestId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopTerminalControl {
    Input { sequence: u64, bytes: Vec<u8> },
    Resize { cols: u16, rows: u16 },
}
impl DesktopTerminalControl {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Input { sequence, bytes }
                if *sequence == 0 || bytes.is_empty() || bytes.len() > DESKTOP_TERMINAL_INPUT =>
            {
                Err("invalid terminal input sequence or size")
            }
            Self::Resize { cols, rows } => validate_desktop_terminal_size(*cols, *rows),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopTerminalOpened {
    pub session_id: RequestId,
    pub shell: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    pub identity: ExecutionIdentity,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopTerminalReply {
    Opened { opened: DesktopTerminalOpened },
    Ready,
    Applied,
    Closed,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopTerminalEvent {
    Output {
        offset: u64,
        bytes: Vec<u8>,
    },
    Ended {
        offset: u64,
        exit_code: Option<u32>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
}

/// Cumulative renderer consumption; duplicates cannot create extra credit.
#[derive(Debug, Default)]
pub struct DesktopTerminalCredit {
    sent: u64,
    consumed: u64,
}
impl DesktopTerminalCredit {
    pub fn available(&self) -> u64 {
        DESKTOP_TERMINAL_CREDIT - (self.sent - self.consumed)
    }
    pub fn sent(&self) -> u64 {
        self.sent
    }
    pub fn reserve(&mut self, bytes: usize) -> Result<u64, &'static str> {
        if bytes == 0 || bytes > DESKTOP_TERMINAL_CHUNK || bytes as u64 > self.available() {
            return Err("output exceeds terminal credit");
        }
        let offset = self.sent;
        self.sent = self
            .sent
            .checked_add(bytes as u64)
            .ok_or("terminal offset overflow")?;
        Ok(offset)
    }
    pub fn consume(&mut self, offset: u64) -> Result<(), &'static str> {
        if offset < self.consumed || offset > self.sent {
            return Err("invalid terminal consumption offset");
        }
        self.consumed = offset;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credit_is_bounded_and_never_silently_discards_output() {
        let mut c = DesktopTerminalCredit::default();
        for i in 0..16 {
            assert_eq!(c.reserve(16384), Ok(i * 16384));
        }
        assert_eq!(c.available(), 0);
        assert!(c.reserve(1).is_err());
        assert!(c.consume(262145).is_err());
        c.consume(16384).unwrap();
        c.consume(16384).unwrap();
        assert_eq!(c.available(), 16384);
        assert!(c.consume(0).is_err());
        assert_eq!(c.reserve(16384), Ok(262144));
    }
    #[test]
    fn control_rejects_invalid_sizes_and_sequences() {
        for n in [0, 4097] {
            assert!(
                DesktopTerminalControl::Input {
                    sequence: 1,
                    bytes: vec![0; n]
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            DesktopTerminalControl::Input {
                sequence: 0,
                bytes: vec![1]
            }
            .validate()
            .is_err()
        );
        assert!(validate_desktop_terminal_size(0, 40).is_err());
        assert!(validate_desktop_terminal_size(100, 201).is_err());
        assert!(validate_desktop_terminal_size(100, 40).is_ok());
        assert!(
            serde_json::from_str::<DesktopTerminalControl>(
                r#"{"action":"resize","cols":100,"rows":40,"unknown":true}"#
            )
            .is_err()
        );
    }
    #[test]
    fn old_environment_wire_roundtrips_without_new_field() {
        let value = serde_json::json!({"type":"environment","context":{"device_ref":{"tenant_id":"00000000-0000-0000-0000-000000000002","device_id":"00000000-0000-0000-0000-000000000003"},"execution":{"os_family":"windows","os_name":"Windows","os_version":"test","architecture":"x86_64","execution_scope":"native","path_style":"windows","interpreter":null,"cwd":"C:\\test","environment_revision":"fixture"},"source":"executor_verified","observed_at_unix_ms":1,"freshness":"current"},"terminal_schema_version":2});
        let response: crate::DeviceTaskResponse = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(
            response,
            crate::DeviceTaskResponse::Environment {
                desktop_terminal_schema_version: None,
                remote_desktop_schema_version: None,
                remote_desktop_cursor_schema_version: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(response).unwrap(), value);
    }
}
