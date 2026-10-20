use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use crate::task_store::AcceptTaskOutcome;
use pab_protocol::{
    CommandTaskSpec, DeviceTaskRequest, ExpectedEnvironment, RequestId, TaskCompletion,
    TaskEventKind, TaskState,
};
use std::sync::Arc;

const WAIT: Duration = Duration::from_secs(5);

async fn completion_after_sent_snapshot(initial: bool) {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let context = svc.execution_context.clone();
    // Store-only fixture: no OS process or command execution is needed to
    // control exactly when the durable task changes behind a sent snapshot.
    let spec = CommandTaskSpec {
        program: "fixture-not-executed".into(),
        args: vec![],
        cwd: None,
        expected_environment: ExpectedEnvironment {
            os_family: context.os_family,
            environment_revision: context.environment_revision.clone(),
        },
        display_summary: "subscription completion race".into(),
        options: Default::default(),
    };
    let AcceptTaskOutcome::Created(created) = svc
        .store
        .accept_command(svc.device_ref, actor(), RequestId::new(), &spec, context, 1)
        .await
        .unwrap()
    else {
        panic!("new fixture task required")
    };
    let task = created.task_ref;
    svc.store
        .record_event(task, TaskEventKind::Running, 2)
        .await
        .unwrap();
    let gate = Arc::new(CatchUpTestGate::default());
    if initial {
        *svc.subscription_test_gate.lock().await = Some(gate.clone());
    }
    let (_a, _b, client, server) = pair().await;
    let serving = svc.clone();
    let receiving = server.clone();
    let worker = tokio::spawn(async move {
        serving
            .handle_stream(actor(), receiving.accept_bi(WAIT).await.unwrap(), WAIT)
            .await
    });
    let mut stream = client.open_bi(WAIT).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::Subscribe {
                schema_version: pab_protocol::DEVICE_TASK_SCHEMA_VERSION,
                task_ref: task,
                after_event_seq: 0,
                stdout_offset: 0,
                stderr_offset: 0,
            },
            WAIT,
        )
        .await
        .unwrap();
    loop {
        if let DeviceTaskResponse::CaughtUp { snapshot } = stream.receive_json(WAIT).await.unwrap()
        {
            assert_eq!(snapshot.state, TaskState::Running);
            break;
        }
    }
    if !initial {
        *svc.subscription_test_gate.lock().await = Some(gate.clone());
        svc.store
            .append_output(task, OutputStream::Stdout, b"before")
            .await
            .unwrap();
    }
    tokio::time::timeout(WAIT, gate.reached.notified())
        .await
        .unwrap();
    svc.store
        .append_output(task, OutputStream::Stdout, b"after")
        .await
        .unwrap();
    svc.store
        .complete_output(task, OutputStream::Stdout)
        .await
        .unwrap();
    svc.store
        .complete_output(task, OutputStream::Stderr)
        .await
        .unwrap();
    svc.store
        .record_event(
            task,
            TaskEventKind::Succeeded {
                completion: TaskCompletion {
                    summary: "fixture complete".into(),
                    exit_code: Some(0),
                },
            },
            3,
        )
        .await
        .unwrap();
    gate.resume.notify_one();
    let mut output = Vec::new();
    loop {
        let reply = stream
            .receive_json::<DeviceTaskResponse>(WAIT)
            .await
            .expect("subscription must deliver final output and terminal snapshot before EOF");
        match reply {
            DeviceTaskResponse::Output { chunk, .. } if chunk.stream == OutputStream::Stdout => {
                output.extend(chunk.bytes)
            }
            DeviceTaskResponse::CaughtUp { snapshot } if snapshot.state.is_terminal() => {
                assert_eq!(snapshot.state, TaskState::Succeeded);
                assert!(snapshot.output.stdout.complete && snapshot.output.stderr.complete);
                assert_eq!(snapshot.output.stdout.available_to, output.len() as u64);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        output,
        if initial {
            b"after".as_slice()
        } else {
            b"beforeafter".as_slice()
        }
    );
    stream.expect_receive_end(WAIT).await.unwrap();
    worker.await.unwrap().unwrap();
    // Ending the subscription must leave another stream usable on this same
    // connection; a client must not need connection recovery to observe success.
    let serving = svc.clone();
    let worker = tokio::spawn(async move {
        serving
            .handle_stream(actor(), server.accept_bi(WAIT).await.unwrap(), WAIT)
            .await
            .unwrap();
    });
    let mut presence = client.open_bi(WAIT).await.unwrap();
    presence
        .send_frame_json(
            &DeviceTaskRequest::GetPresence {
                schema_version: pab_protocol::DEVICE_TASK_SCHEMA_VERSION,
            },
            WAIT,
        )
        .await
        .unwrap();
    assert!(matches!(
        presence
            .receive_json::<DeviceTaskResponse>(WAIT)
            .await
            .unwrap(),
        DeviceTaskResponse::Presence { .. }
    ));
    worker.await.unwrap();
}

#[tokio::test]
async fn initial_catch_up_completion_delivers_final_snapshot_before_eof() {
    completion_after_sent_snapshot(true).await;
}

#[tokio::test]
async fn live_catch_up_completion_delivers_final_snapshot_before_eof() {
    completion_after_sent_snapshot(false).await;
}
