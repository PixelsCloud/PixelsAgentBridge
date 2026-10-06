//! Same internal worker entry as desktop, without rebuilding the GUI for backend tests.
fn main() {
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--accept-fixture") {
        let dir = std::path::PathBuf::from(std::env::args().nth(2).expect("fixture directory"));
        let result = accept_fixture(&dir);
        let report = serde_json::json!({"success":result.is_ok(),"error":result.err()});
        std::fs::write(
            dir.join("backend-report.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        return;
    }
    if pab_desktop_control::ui_worker_entry::run().is_err() {
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn accept_fixture(dir: &std::path::Path) -> Result<(), String> {
    use pab_desktop_control::{
        ui_engine::UiWindow,
        ui_registry::UiOwner,
        ui_worker::WorkerProcess,
        ui_worker_entry::{WorkerCommand, WorkerMessage, WorkerReply},
    };
    use pab_protocol::*;
    use std::{process::Command, time::Duration};
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let ready: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("ready.json"))?)?;
        let pid = ready["pid"].as_u64().ok_or("fixture pid missing")? as u32;
        let id = ready["window_number"]
            .as_u64()
            .ok_or("fixture window missing")? as u32;
        let ticket = UiWindow {
            window_ref: RequestId::new().to_string(),
            id,
            pid,
            process_identity: pab_os_control::process_identity(pid)?,
            marker_key: "fixture".into(),
            marker: 1,
        };
        let owner = UiOwner {
            connection: RequestId::new(),
            helper_instance: RequestId::new().to_string(),
        };
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("--ui-worker");
        let mut worker = WorkerProcess::spawn(&mut command).map_err(|e| format!("worker {e:?}"))?;
        let mut call = |ticket: Option<UiWindow>,
                        request: UiRequest|
         -> Result<UiSnapshot, Box<dyn std::error::Error>> {
            let id = RequestId::new();
            let value = worker
                .exchange(
                    &serde_json::to_value(WorkerMessage {
                        request_id: id,
                        owner: owner.clone(),
                        command: WorkerCommand::Query { ticket, request },
                    })?,
                    Duration::from_secs(10),
                )
                .map_err(|e| format!("exchange {e:?}"))?;
            let reply: WorkerReply = serde_json::from_value(value)?;
            if reply.request_id != id {
                return Err("reply identity mismatch".into());
            }
            let result = reply.snapshot.ok_or("no snapshot")?;
            Ok(result)
        };
        let tree = call(
            Some(ticket.clone()),
            UiRequest::Query {
                scope: UiScope::Window {
                    window_ref: ticket.window_ref,
                },
                selector: UiSelector::default(),
                limits: UiQueryLimits::default(),
            },
        )?;
        if let Some(code) = &tree.error_code {
            return Err(code.clone().into());
        }
        std::fs::write(
            dir.join("backend-tree.json"),
            serde_json::to_vec_pretty(&tree)?,
        )?;
        if tree.truncated {
            return Err("unexpected truncated fixture".into());
        }
        let reference = |name: &str| -> Result<String, Box<dyn std::error::Error>> {
            let matches: Vec<_> = tree
                .elements
                .iter()
                .filter(|e| e.name.as_deref() == Some(name))
                .collect();
            if matches.len() != 1 {
                return Err(format!("fixture target ambiguous: {name}").into());
            }
            Ok(matches[0].element_ref.clone())
        };
        if tree
            .elements
            .iter()
            .filter(|e| e.name.as_deref() == Some("Fixture duplicate"))
            .count()
            != 2
        {
            return Err("duplicate controls missing".into());
        }
        let secure = call(
            None,
            UiRequest::Get {
                element_ref: reference("Fixture secure")?,
                include_value: true,
            },
        )?;
        if secure.elements.len() != 1
            || !secure.elements[0].protected
            || secure.elements[0].value.is_some()
        {
            return Err("secure value leaked".into());
        }
        for (name, action) in [
            (
                "Fixture readonly",
                UiAction::SetValue {
                    value: "must-not-write".into(),
                },
            ),
            (
                "Fixture secure",
                UiAction::SetValue {
                    value: "must-not-write".into(),
                },
            ),
            ("Fixture disabled", UiAction::Invoke),
        ] {
            let result = call(
                None,
                UiRequest::Action {
                    element_ref: reference(name)?,
                    action,
                    expected: UiExpected::default(),
                    timeout_ms: 5000,
                },
            )?;
            if result.outcome != UiOutcome::Rejected || result.action_dispatched != Some(false) {
                return Err(format!("unsafe action accepted: {name}").into());
            }
        }
        for (name, action, verify) in [
            (
                "Fixture input",
                UiAction::SetValue {
                    value: String::new(),
                },
                true,
            ),
            ("Fixture radio", UiAction::Select, true),
            (
                "Fixture input",
                UiAction::SetValue {
                    value: "PAB 中文🙂".into(),
                },
                true,
            ),
            (
                "Fixture option",
                UiAction::SetChecked { checked: true },
                true,
            ),
            ("Apply fixture", UiAction::Invoke, false),
        ] {
            let result = call(
                None,
                UiRequest::Action {
                    element_ref: reference(name)?,
                    action,
                    expected: UiExpected::default(),
                    timeout_ms: 5000,
                },
            )?;
            if let Some(code) = &result.error_code {
                return Err(format!("{name}: {code}").into());
            }
            if verify && result.verification != UiVerification::Matched {
                return Err(format!("verification failed: {name}").into());
            }
        }
        for _ in 0..30 {
            if dir.join("result.json").exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("result.json"))?)?;
        if result["clicks"] != 1
            || result["value"] != "PAB 中文🙂"
            || !(result["checked"] == true || result["checked"] == 1)
            || !(result["radio"] == true || result["radio"] == 1)
        {
            return Err("actual fixture effect mismatch".into());
        }
        Ok(())
    };
    run().map_err(|e| e.to_string())
}
