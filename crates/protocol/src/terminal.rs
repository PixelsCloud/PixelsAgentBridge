use serde::{Deserialize, Serialize};

/// Observed launch settings, not a request to change an existing PTY.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartup {
    pub arguments: Vec<String>,
    pub mode: TerminalStartupMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStartupMode {
    Interactive,
    InteractiveLogin,
    InteractiveNoProfile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceTaskResponse, RequestId};

    #[test]
    fn terminal_startup_roundtrip_and_older_peer_is_unknown() {
        let id = RequestId::new();
        let old = serde_json::json!({"type":"terminal_opened", "session_id":id,
            "shell":"/bin/sh", "cols":80, "rows":24});
        let response: DeviceTaskResponse = serde_json::from_value(old).unwrap();
        assert!(matches!(
            response,
            DeviceTaskResponse::TerminalOpened { startup: None, .. }
        ));
        let response = DeviceTaskResponse::TerminalOpened {
            session_id: id,
            shell: "/bin/zsh".into(),
            cols: 80,
            rows: 24,
            identity: None,
            startup: Some(TerminalStartup {
                arguments: vec!["-l".into(), "-i".into()],
                mode: TerminalStartupMode::InteractiveLogin,
            }),
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["startup"]["mode"], "interactive_login");
        assert_eq!(
            value["startup"]["arguments"],
            serde_json::json!(["-l", "-i"])
        );
        assert!(matches!(
            serde_json::from_value::<DeviceTaskResponse>(value).unwrap(),
            DeviceTaskResponse::TerminalOpened {
                startup: Some(TerminalStartup {
                    mode: TerminalStartupMode::InteractiveLogin,
                    ..
                }),
                ..
            }
        ));
    }
}
