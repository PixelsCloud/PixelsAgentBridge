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
        let worker_pid = worker.process_id();
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
                    window_ref: ticket.window_ref.clone(),
                },
                selector: UiSelector::default(),
                limits: UiQueryLimits::default(),
            },
        )?;
        if let Some(code) = &tree.error_code {
            return Err(code.clone().into());
        }
        if std::env::args().nth(3).as_deref() == Some("--stress") {
            let rss = || -> Result<u64, Box<dyn std::error::Error>> {
                let output = Command::new("/bin/ps")
                    .args(["-o", "rss=", "-p", &worker_pid.to_string()])
                    .output()?;
                Ok(String::from_utf8(output.stdout)?.trim().parse()?)
            };
            let baseline = rss()?;
            let mut max_ms = 0;
            let mut min_visited = u32::MAX;
            let mut truncated = 0;
            for _ in 0..50 {
                let start = std::time::Instant::now();
                let result = call(
                    Some(ticket.clone()),
                    UiRequest::Query {
                        scope: UiScope::Window {
                            window_ref: ticket.window_ref.clone(),
                        },
                        selector: UiSelector {
                            name: Some("nonexistent budget selector".into()),
                            ..Default::default()
                        },
                        limits: UiQueryLimits::default(),
                    },
                )?;
                if result.error_code.is_some() {
                    return Err("stress provider failed".into());
                }
                min_visited = min_visited.min(result.visited_count);
                max_ms = max_ms.max(start.elapsed().as_millis());
                truncated += usize::from(result.truncated);
            }
            let final_rss = rss()?;
            if final_rss > baseline + 8192 {
                return Err("worker RSS grew beyond 8 MiB budget".into());
            }
            std::fs::write(
                dir.join("stress-report.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"rounds":50,"baseline_rss_kib":baseline,"final_rss_kib":final_rss,"max_ms":max_ms,"min_visited":min_visited,"truncated_rounds":truncated,"initial_truncated":tree.truncated,"initial_stop_reason":tree.stop_reason,"initial_bytes":serde_json::to_vec(&tree)?.len()}),
                )?,
            )?;
            return Ok(());
        }
        std::fs::write(
            dir.join("backend-tree.json"),
            serde_json::to_vec_pretty(&tree)?,
        )?;
        if tree.truncated {
            return Err("unexpected truncated fixture".into());
        }
        // Cell-based AppKit tables expose unnamed AXRows. Resolve the row by
        // explicitly reading its child cell, exactly as an agent can do.
        let mut second_row = None;
        for cell in tree
            .elements
            .iter()
            .filter(|e| e.role == UiRole::TextField && e.name.is_none())
        {
            let read = call(
                None,
                UiRequest::Get {
                    element_ref: cell.element_ref.clone(),
                    include_value: true,
                },
            )?;
            if read.elements.first().and_then(|e| e.value.as_deref()) == Some("Fixture second") {
                second_row = cell.parent_ref.clone();
            }
        }
        let reference = |name: &str| -> Result<String, Box<dyn std::error::Error>> {
            if name == "Fixture second row" {
                return second_row
                    .clone()
                    .ok_or_else(|| "second row missing".into());
            }
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
            ("Fixture second row", UiAction::Select, true),
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
            || result["selected"] != 1
        {
            return Err("actual fixture effect mismatch".into());
        }
        Ok(())
    };
    run().map_err(|e| e.to_string())
}
