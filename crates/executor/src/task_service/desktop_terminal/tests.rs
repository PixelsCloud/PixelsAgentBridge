use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, ExecutionSelection,
};
use pab_transport::PabConnection;
const TIMEOUT: Duration = Duration::from_secs(15);
fn logged_in(svc: TaskService) -> TaskService {
    svc.with_user_context(pab_protocol::EndpointUserContext {
        revision: 1,
        policy_version: 1,
        user: Some(pab_protocol::UserAttribution {
            user_id: pab_protocol::UserId::from_u128(81),
            username: "terminal-test".into(),
        }),
    })
}

async fn serve_one(
    svc: TaskService,
    server: PabConnection,
    actor: OperatorRef,
) -> tokio::task::JoinHandle<Result<(), TaskServiceError>> {
    tokio::spawn(async move {
        let stream = server.accept_bi(TIMEOUT).await?;
        svc.handle_stream(actor, stream, TIMEOUT).await
    })
}
async fn begin(client: &PabConnection, request: DesktopTerminalRequest) -> PabBiStream {
    let mut stream = client.open_bi(TIMEOUT).await.unwrap();
    stream
        .send_frame_json(
            &DeviceTaskRequest::DesktopTerminal {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request,
            },
            TIMEOUT,
        )
        .await
        .unwrap();
    stream
}
#[tokio::test]
async fn guest_missing_session_and_unsupported_version_fail_without_worker() {
    let temp = tempfile::tempdir().unwrap();
    let svc = service(temp.path()).await.for_ui_connection();
    let (_a, _b, client, server) = pair().await;
    let id = RequestId::new();
    let cases = [
        (
            OperatorRef::guest(actor().endpoint_key()),
            DesktopTerminalRequest::Control { session_id: id },
            DeviceTaskErrorCode::AccessDenied,
        ),
        (
            actor(),
            DesktopTerminalRequest::Close { session_id: id },
            DeviceTaskErrorCode::NotFound,
        ),
        (
            actor(),
            DesktopTerminalRequest::Open {
                version: 500,
                session_id: id,
                execution: Default::default(),
                cols: 100,
                rows: 40,
            },
            DeviceTaskErrorCode::InvalidRequest,
        ),
        (
            actor(),
            DesktopTerminalRequest::Open {
                version: 1,
                session_id: id,
                execution: Default::default(),
                cols: 0,
                rows: 40,
            },
            DeviceTaskErrorCode::InvalidRequest,
        ),
    ];
    for (owner, request, expected) in cases {
        let selected_service = if matches!(owner, OperatorRef::Guest { .. }) {
            svc.clone()
        } else {
            logged_in(svc.clone())
        };
        let serving = serve_one(selected_service, server.clone(), owner).await;
        let mut stream = begin(&client, request).await;
        let reply: DeviceTaskResponse = stream.receive_json(TIMEOUT).await.unwrap();
        assert!(matches!(reply, DeviceTaskResponse::Error { code, .. } if code == expected));
        serving.await.unwrap().unwrap();
    }
    assert!(svc.desktop_terminals.sessions.lock().await.is_empty());
    assert_eq!(svc.desktop_terminals.global.available_permits(), 32);
    assert_eq!(svc.desktop_terminals.connection.available_permits(), 8);
}
#[test]
fn consumption_rejects_bad_frames_backwards_and_ahead_offsets() {
    let mut credit = DesktopTerminalCredit::default();
    credit.reserve(16384).unwrap();
    assert!(apply_ack(&mut credit, &[0; 7]).is_err());
    assert!(apply_ack(&mut credit, &16385_u64.to_be_bytes()).is_err());
    apply_ack(&mut credit, &16384_u64.to_be_bytes()).unwrap();
    apply_ack(&mut credit, &16384_u64.to_be_bytes()).unwrap();
    assert!(apply_ack(&mut credit, &1_u64.to_be_bytes()).is_err());
    assert_eq!(credit.available(), 262144);
}
#[test]
fn reservations_and_quotas_are_independent_of_stable_terminal_resources() {
    let manager = DesktopTerminals::default();
    let second = manager.for_connection();
    let mut permits = Vec::new();
    for _ in 0..8 {
        permits.push(manager.connection.clone().try_acquire_owned().unwrap());
    }
    assert!(manager.connection.clone().try_acquire_owned().is_err());
    assert_eq!(second.connection.available_permits(), 8);
    assert!(Arc::ptr_eq(&manager.global, &second.global));
    let global: Vec<_> = (0..32)
        .map(|_| manager.global.clone().try_acquire_owned().unwrap())
        .collect();
    assert!(second.global.clone().try_acquire_owned().is_err());
    drop(global);
    assert_eq!(second.global.available_permits(), 32);
    let id = RequestId::new();
    manager.reservations.lock().unwrap().insert(id);
    let reserved = Reservation {
        id,
        ids: manager.reservations.clone(),
    };
    drop(reserved);
    assert!(!second.reservations.lock().unwrap().contains(&id));
}

// Run against an explicitly built worker, in an isolated test database and QUIC
// pair. On Windows this runs as SYSTEM on 90; it never restarts the product service.
#[tokio::test]
#[ignore = "requires PAB_DESKTOP_TERMINAL_TEST_WORKER; creates real dedicated PTY workers"]
async fn real_worker_streams_preserves_state_drains_exit_and_reclaims_quota() {
    let temp = tempfile::tempdir().unwrap();
    let mut svc = logged_in(service(temp.path()).await.for_ui_connection());
    svc.worker_executable = Some(
        std::env::var_os("PAB_DESKTOP_TERMINAL_TEST_WORKER")
            .expect("explicit test worker")
            .into(),
    );
    let (_a, _b, client, server) = pair().await;
    let id = RequestId::new();
    let handler = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut open = begin(
        &client,
        DesktopTerminalRequest::Open {
            version: 1,
            session_id: id,
            execution: ExecutionSelection::default(),
            cols: 100,
            rows: 40,
        },
    )
    .await;
    let reply: DeviceTaskResponse = open.receive_json(TIMEOUT).await.unwrap();
    assert!(
        matches!(reply, DeviceTaskResponse::DesktopTerminal { reply: DesktopTerminalReply::Opened { opened } } if opened.session_id == id && opened.identity.mode == ExecutionMode::Service)
    );
    handler.await.unwrap().unwrap();
    assert!(matches!(
        svc.for_ui_connection().desktop_entry(actor(), id).await,
        Err(TaskServiceError::AccessDenied)
    ));
    let other = OperatorRef::account(pab_protocol::UserId::from_u128(999), actor().endpoint_key());
    assert!(matches!(
        svc.desktop_entry(other, id).await,
        Err(TaskServiceError::AccessDenied)
    ));
    let control_handle = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut control = begin(&client, DesktopTerminalRequest::Control { session_id: id }).await;
    let _: DeviceTaskResponse = control.receive_json(TIMEOUT).await.unwrap();
    let (mut writer, mut replies) = control.into_framed();
    let output_handle = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut output = begin(&client, DesktopTerminalRequest::Output { session_id: id }).await;
    let _: DeviceTaskResponse = output.receive_json(TIMEOUT).await.unwrap();
    let (mut acknowledgements, mut reader) = output.into_framed();
    let command = if cfg!(windows) {
        "\x1b[1;1R$env:PIXELS_TERMINAL_TEST='kept'; Write-Output 'PIXELS_FIRST'; Write-Output $env:PIXELS_TERMINAL_TEST; Write-Output ('X' * 700000); Write-Output 'PIXELS_LAST'; exit 7\r"
    } else {
        "PIXELS_TERMINAL_TEST=kept; export PIXELS_TERMINAL_TEST; printf 'PIXELS_FIRST\\n%s\\n' \"$PIXELS_TERMINAL_TEST\"; head -c 700000 /dev/zero | tr '\\0' X; printf '\\nPIXELS_LAST\\n'; exit 7\n"
    };
    let bytes = serde_json::to_vec(&DesktopTerminalControl::Input {
        sequence: 1,
        bytes: command.as_bytes().to_vec(),
    })
    .unwrap();
    writer.write(&bytes, 24 * 1024).await.unwrap();
    let mut all = Vec::new();
    let mut end = None;
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let bytes = reader.read(DESKTOP_TERMINAL_CHUNK + 9).await.unwrap();
            assert_eq!(
                u64::from_be_bytes(bytes[1..9].try_into().unwrap()),
                all.len() as u64
            );
            if bytes[0] == 1 {
                end = Some(serde_json::from_slice::<serde_json::Value>(&bytes[9..]).unwrap());
                break;
            }
            all.extend_from_slice(&bytes[9..]);
            acknowledgements
                .write(&(all.len() as u64).to_be_bytes(), 8)
                .await
                .unwrap();
        }
    })
    .await
    .unwrap();
    // A reply may arrive before or after output; separate directions preserve both.
    let reply = replies.read(4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<DesktopTerminalReply>(&reply).unwrap(),
        DesktopTerminalReply::Applied
    );
    let end = end.unwrap();
    assert_eq!(end["exit_code"], 7);
    assert_eq!(end["output_bytes"], all.len() as u64);
    use sha2::Digest;
    assert_eq!(end["sha256"], format!("{:x}", sha2::Sha256::digest(&all)));
    let text = String::from_utf8_lossy(&all);
    assert!(text.contains("PIXELS_FIRST"));
    assert!(text.contains("kept"));
    assert!(text.contains("PIXELS_LAST"));
    assert!(all.len() > 700000);
    output_handle.await.unwrap().unwrap();
    let _ = control_handle.await.unwrap();
    assert!(svc.desktop_terminals.sessions.lock().await.is_empty());
    // Setup timers use Weak and must not retain native worker resources/quota.
    assert_eq!(svc.desktop_terminals.global.available_permits(), 32);
    assert_eq!(svc.desktop_terminals.connection.available_permits(), 8);
}

async fn live_service(path: &std::path::Path) -> TaskService {
    let mut svc = logged_in(service(path).await.for_ui_connection());
    svc.worker_executable = Some(
        std::env::var_os("PAB_DESKTOP_TERMINAL_TEST_WORKER")
            .expect("explicit test worker")
            .into(),
    );
    svc
}
async fn live_open(svc: &TaskService, client: &PabConnection, server: &PabConnection) -> RequestId {
    let id = RequestId::new();
    let serving = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut stream = begin(
        client,
        DesktopTerminalRequest::Open {
            version: 1,
            session_id: id,
            execution: ExecutionSelection::default(),
            cols: 100,
            rows: 40,
        },
    )
    .await;
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::DesktopTerminal {
            reply: DesktopTerminalReply::Opened { .. }
        }
    ));
    serving.await.unwrap().unwrap();
    id
}
async fn live_close(
    svc: &TaskService,
    client: &PabConnection,
    server: &PabConnection,
    id: RequestId,
) {
    let serving = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut stream = begin(client, DesktopTerminalRequest::Close { session_id: id }).await;
    assert!(matches!(
        stream
            .receive_json::<DeviceTaskResponse>(TIMEOUT)
            .await
            .unwrap(),
        DeviceTaskResponse::DesktopTerminal {
            reply: DesktopTerminalReply::Closed
        }
    ));
    serving.await.unwrap().unwrap();
}

#[tokio::test]
#[ignore = "requires an explicit disposable real PTY worker"]
async fn real_worker_backpressure_cannot_block_independent_close_or_other_session() {
    let temp = tempfile::tempdir().unwrap();
    let svc = live_service(temp.path()).await;
    let (_a, _b, client, server) = pair().await;
    let id = live_open(&svc, &client, &server).await;
    let unaffected = live_open(&svc, &client, &server).await;
    let control_task = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut control = begin(&client, DesktopTerminalRequest::Control { session_id: id }).await;
    let _: DeviceTaskResponse = control.receive_json(TIMEOUT).await.unwrap();
    let (mut input, mut applied) = control.into_framed();
    let output_task = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut output = begin(&client, DesktopTerminalRequest::Output { session_id: id }).await;
    let _: DeviceTaskResponse = output.receive_json(TIMEOUT).await.unwrap();
    let (_ack, mut output) = output.into_framed();
    let command = if cfg!(windows) {
        "\x1b[1;1R[Console]::Write(('X'*10000000)); Start-Sleep -Seconds 60\r"
    } else {
        "head -c 10000000 /dev/zero | tr '\\000' X; sleep 60\n"
    };
    input
        .write(
            &serde_json::to_vec(&DesktopTerminalControl::Input {
                sequence: 1,
                bytes: command.as_bytes().to_vec(),
            })
            .unwrap(),
            24 * 1024,
        )
        .await
        .unwrap();
    let _: DesktopTerminalReply =
        serde_json::from_slice(&applied.read(4096).await.unwrap()).unwrap();
    let mut received = 0;
    tokio::time::timeout(Duration::from_secs(15), async {
        while received <= (pab_protocol::DESKTOP_TERMINAL_CREDIT as usize - DESKTOP_TERMINAL_CHUNK)
        {
            let frame = output.read(DESKTOP_TERMINAL_CHUNK + 9).await.unwrap();
            assert_eq!(frame[0], 0);
            received += frame.len() - 9;
        }
    })
    .await
    .unwrap();
    assert!(received <= pab_protocol::DESKTOP_TERMINAL_CREDIT as usize);
    // Abandon the timed-out frame reader; partial reads are never reused.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(200),
            output.read(DESKTOP_TERMINAL_CHUNK + 9)
        )
        .await
        .is_err()
    );
    let started = tokio::time::Instant::now();
    live_close(&svc, &client, &server, id).await;
    assert!(started.elapsed() < Duration::from_secs(9));
    let _ = output_task.await;
    let _ = control_task.await;
    assert!(svc.desktop_entry(actor(), unaffected).await.is_ok());
    live_close(&svc, &client, &server, unaffected).await;
    assert!(svc.desktop_terminals.sessions.lock().await.is_empty());
    assert_eq!(svc.desktop_terminals.global.available_permits(), 32);
    assert_eq!(svc.desktop_terminals.connection.available_permits(), 8);
}

#[tokio::test]
#[ignore = "requires an explicit disposable real PTY worker"]
async fn real_worker_invalid_control_account_switch_and_connection_loss_reclaim_only_owned_processes()
 {
    let temp = tempfile::tempdir().unwrap();
    let svc = live_service(temp.path()).await;
    let (_a, _b, client, server) = pair().await;
    let id = live_open(&svc, &client, &server).await;
    let serving = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut control = begin(&client, DesktopTerminalRequest::Control { session_id: id }).await;
    let _: DeviceTaskResponse = control.receive_json(TIMEOUT).await.unwrap();
    let (mut input, _reply) = control.into_framed();
    input
        .write(br#"{"action":"resize","cols":0,"rows":0}"#, 24 * 1024)
        .await
        .unwrap();
    assert!(serving.await.unwrap().is_err());
    assert!(svc.desktop_terminals.sessions.lock().await.is_empty());
    let id = live_open(&svc, &client, &server).await;
    let signed_out = svc
        .clone()
        .with_user_context(pab_protocol::EndpointUserContext {
            revision: 2,
            policy_version: 1,
            user: None,
        });
    assert!(matches!(
        signed_out.desktop_entry(actor(), id).await,
        Err(TaskServiceError::AccessDenied)
    ));
    // Cleanup is still permitted for the original connection and endpoint.
    live_close(&signed_out, &client, &server, id).await;
    // Simulate loss of only this owned worker; the service and shared QUIC
    // connection remain available and a new Shell can still be opened.
    let crashed = live_open(&svc, &client, &server).await;
    let process = svc.desktop_entry(actor(), crashed).await.unwrap();
    process.child.lock().await.terminate().unwrap();
    let serving = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut output = begin(
        &client,
        DesktopTerminalRequest::Output {
            session_id: crashed,
        },
    )
    .await;
    let _: DeviceTaskResponse = output.receive_json(TIMEOUT).await.unwrap();
    assert!(serving.await.unwrap().is_err());
    assert!(
        !svc.desktop_terminals
            .sessions
            .lock()
            .await
            .contains_key(&crashed)
    );
    drop(process);
    let id = live_open(&svc, &client, &server).await;
    let own = svc.desktop_entry(actor(), id).await.unwrap();
    let unrelated_service = svc.for_ui_connection();
    let unrelated = live_open(&unrelated_service, &client, &server).await;
    drop(svc.desktop_terminal_connection_guard());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !svc
                .desktop_terminals
                .sessions
                .lock()
                .await
                .contains_key(&id)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(own.child.lock().await.try_wait().unwrap().is_some());
    assert!(
        unrelated_service
            .desktop_entry(actor(), unrelated)
            .await
            .is_ok()
    );
    drop(own);
    live_close(&unrelated_service, &client, &server, unrelated).await;
    assert_eq!(svc.desktop_terminals.global.available_permits(), 32);
}

#[tokio::test]
#[ignore = "explicit disposable PTY worker; streams over 256 MiB without retaining it"]
async fn real_worker_streams_256_mib_with_bounded_receiver_and_source_digest() {
    let temp = tempfile::tempdir().unwrap();
    let svc = live_service(temp.path()).await;
    let (_a, _b, client, server) = pair().await;
    let id = live_open(&svc, &client, &server).await;
    let control_task = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut control = begin(&client, DesktopTerminalRequest::Control { session_id: id }).await;
    let _: DeviceTaskResponse = control.receive_json(TIMEOUT).await.unwrap();
    let (mut input, mut applied) = control.into_framed();
    let output_task = serve_one(svc.clone(), server.clone(), actor()).await;
    let mut output = begin(&client, DesktopTerminalRequest::Output { session_id: id }).await;
    let _: DeviceTaskResponse = output.receive_json(TIMEOUT).await.unwrap();
    let (mut ack, mut output) = output.into_framed();
    let command = if cfg!(windows) {
        "\x1b[1;1R$s='X'*16384; for($i=0;$i-lt16900;$i++){[Console]::Write($s)}; [Console]::WriteLine(('VOLUME_'+'TAIL')); exit 11\r"
    } else {
        "head -c 276889600 /dev/zero | tr '\\000' X; printf '\\nVOLUME_TAIL\\n'; exit 11\n"
    };
    input
        .write(
            &serde_json::to_vec(&DesktopTerminalControl::Input {
                sequence: 1,
                bytes: command.as_bytes().to_vec(),
            })
            .unwrap(),
            24 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<DesktopTerminalReply>(&applied.read(4096).await.unwrap()).unwrap(),
        DesktopTerminalReply::Applied
    );
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    let mut count = 0_u64;
    let mut tail = Vec::new();
    let end = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let frame = output.read(DESKTOP_TERMINAL_CHUNK + 9).await.unwrap();
            assert_eq!(u64::from_be_bytes(frame[1..9].try_into().unwrap()), count);
            if frame[0] == 1 {
                break serde_json::from_slice::<serde_json::Value>(&frame[9..]).unwrap();
            }
            assert_eq!(frame[0], 0);
            digest.update(&frame[9..]);
            count += (frame.len() - 9) as u64;
            tail.extend_from_slice(&frame[9..]);
            if tail.len() > 65536 {
                tail.drain(..tail.len() - 65536);
            }
            ack.write(&count.to_be_bytes(), 8).await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(count > 256 * 1024 * 1024);
    assert!(String::from_utf8_lossy(&tail).contains("VOLUME_TAIL"));
    assert_eq!(end["exit_code"], 11);
    assert_eq!(end["output_bytes"], count);
    assert_eq!(end["sha256"], format!("{:x}", digest.finalize()));
    output_task.await.unwrap().unwrap();
    let _ = control_task.await;
    assert_eq!(svc.desktop_terminals.global.available_permits(), 32);
}
