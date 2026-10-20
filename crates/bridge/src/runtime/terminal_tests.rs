use super::*;
use crate::runtime::test_support::runtime;
use pab_protocol::{DeviceTaskRequest, DeviceTaskResponse};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn close_archives_large_tail_serializes_polling_and_records_missing_output() {
    const TIMEOUT: Duration = Duration::from_secs(5);
    for missing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = runtime(dir.path()).await;
        let (_a, _b, client, server) = crate::connection::test_connection_pair().await;
        let device = client.device_ref();
        runtime
            .inner
            .device(device)
            .await
            .set_test_connection(client)
            .await;
        let id = RequestId::new();
        runtime
            .inner
            .store
            .start_terminal_operation(id, device, None, "fixture", "shell", "fixture")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join(format!("{id}.bin")), [])
            .await
            .unwrap();
        runtime.inner.terminals.lock().await.insert(
            id,
            Arc::new(Mutex::new(TerminalRuntimeSession {
                device_ref: device,
                next_sequence: 1,
                offset: 0,
                ended: false,
            })),
        );
        // More than the old twenty-read cap, with binary data and a partial last frame.
        let payload: Vec<u8> = (0..700_003).map(|n| (n % 251) as u8).collect();
        let expected = payload.clone();
        let (closing, observed) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
            assert!(
                matches!(stream.receive_json::<DeviceTaskRequest>(TIMEOUT).await.unwrap(),
                DeviceTaskRequest::TerminalClose { session_id, sequence: 1, .. } if session_id == id)
            );
            closing.send(()).unwrap();
            released.await.unwrap();
            stream
                .send_json(
                    &DeviceTaskResponse::TerminalClosed { session_id: id },
                    TIMEOUT,
                )
                .await
                .unwrap();
            let mut position = 0;
            loop {
                let mut stream = server.accept_bi(TIMEOUT).await.unwrap();
                let DeviceTaskRequest::TerminalRead { offset, limit, .. } =
                    stream.receive_json(TIMEOUT).await.unwrap()
                else {
                    panic!("expected read");
                };
                if missing {
                    stream
                        .send_json(
                            &DeviceTaskResponse::Error {
                                code: pab_protocol::DeviceTaskErrorCode::NotFound,
                                message: "fixture tail unavailable".into(),
                            },
                            TIMEOUT,
                        )
                        .await
                        .unwrap();
                    break;
                }
                assert_eq!(offset as usize, position);
                let end = (position + usize::from(limit)).min(payload.len());
                stream
                    .send_frame_json(
                        &DeviceTaskResponse::TerminalOutput {
                            session_id: id,
                            retained_from: 0,
                            offset,
                            next_offset: end as u64,
                            size: (end - position) as u16,
                            ended: end == payload.len(),
                        },
                        TIMEOUT,
                    )
                    .await
                    .unwrap();
                stream
                    .send_binary_frame(&payload[position..end], TIMEOUT)
                    .await
                    .unwrap();
                stream.finish_send(TIMEOUT).await.unwrap();
                position = end;
                if position == payload.len() {
                    break;
                }
            }
        });
        let closer = runtime.clone();
        let closing = tokio::spawn(async move { closer.terminal_close(id).await });
        observed.await.unwrap();
        if missing {
            release.send(()).unwrap();
            assert!(closing.await.unwrap().is_err());
        } else {
            // Desktop's periodic read is already waiting on this same session.
            let poll = runtime.terminal_read(id);
            tokio::pin!(poll);
            assert!(futures_util::poll!(&mut poll).is_pending());
            release.send(()).unwrap();
            closing.await.unwrap().unwrap();
            let final_read = poll.await.unwrap();
            assert!(final_read.ended && final_read.bytes.is_empty());
            assert_eq!(
                tokio::fs::read(dir.path().join(format!("{id}.bin")))
                    .await
                    .unwrap(),
                expected
            );
        }
        peer.await.unwrap();
        assert!(runtime.inner.terminals.lock().await.is_empty());
        let records = runtime.inner.store.operations().await.unwrap();
        assert_eq!(
            records[0].state,
            if missing { "failed" } else { "completed" }
        );
        runtime.inner.devices.lock().await.clear(); // release test connection/runtime references
        clear_account(&runtime);
    }
}

fn clear_account(runtime: &BridgeRuntime) {
    let state = runtime.inner.account_store.logout().unwrap();
    for slot in state.pending_logouts {
        runtime.inner.account_store.finish_logout(&slot).unwrap();
    }
}

#[tokio::test]
async fn account_readiness_requires_current_confirmed_login() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = runtime(dir.path()).await;
    assert!(runtime.inner.wait_account_ready().await.is_ok());
    let status = runtime.inner.presence.lock().unwrap().account.take();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            runtime.inner.wait_account_ready()
        )
        .await
        .is_err()
    );
    runtime.inner.presence.lock().unwrap().account = status;
    runtime
        .inner
        .presence
        .lock()
        .unwrap()
        .account
        .as_mut()
        .unwrap()
        .user = None;
    assert!(
        runtime
            .inner
            .wait_account_ready()
            .await
            .unwrap_err()
            .to_string()
            .contains("sign in again")
    );
    clear_account(&runtime);
    assert!(
        runtime
            .inner
            .wait_account_ready()
            .await
            .unwrap_err()
            .to_string()
            .contains("login required")
    );
}
