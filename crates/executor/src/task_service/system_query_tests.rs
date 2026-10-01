use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, RequestId, SystemQuery,
    SystemQueryData,
};
use std::time::Duration;

#[tokio::test]
async fn restart_keeps_lifecycle_result_unconfirmed_and_never_replays_request() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let q = SystemQuery::ServiceControl {
        name: "pab-missing-owned-test-service".into(),
        control: pab_protocol::ServiceControlAction::Start,
        timeout_ms: 100,
    };
    let id = RequestId::new();
    svc.store
        .accept_system_query(actor(), id, &q)
        .await
        .unwrap();
    svc.store.interrupt_read_operations().await.unwrap();
    let reopened = service(dir.path()).await;
    let result = reopened.system_query(actor(), id, q).await.unwrap();
    assert_eq!(result.state, "unconfirmed");
    assert!(result.data.is_none());
    assert!(result.error.unwrap().contains("never replayed"));
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "explicit native control over QUIC; requires PAB_PROCESS_TEST_FIXTURE"]
async fn lifecycle_returns_running_survives_stream_drop_and_deduplicates_side_effects() {
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let path =
        std::env::var_os("PAB_PROCESS_TEST_FIXTURE").expect("build the process_fixture example");
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let mut child = OwnedChild(
        Command::new(path)
            .arg("ignore-close")
            .arg(&ready)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid = child.0.id();
    let svc = service(dir.path()).await;
    let q_process = SystemQuery::Process {
        pid,
        sample_cpu: false,
    };
    let r = svc
        .system_query(actor(), RequestId::new(), q_process)
        .await
        .unwrap();
    let Some(SystemQueryData::Process { process }) = r.data else {
        panic!("{r:?}");
    };
    let identity = process.termination_identity.unwrap();
    let q = SystemQuery::TerminateProcess {
        pid,
        identity: identity.clone(),
        timeout_ms: 1500,
        force: false,
    };
    let id = RequestId::new();
    let (a, b, client, server) = pair().await;
    let worker = {
        let svc = svc.clone();
        let server = server.clone();
        tokio::spawn(async move {
            svc.handle_stream(
                actor(),
                server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        })
    };
    let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
    stream
        .send_json(
            &DeviceTaskRequest::SystemQuery {
                schema_version: DEVICE_TASK_SCHEMA_VERSION,
                request_id: id,
                query: q.clone(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let response: DeviceTaskResponse = stream.receive_json(Duration::from_secs(5)).await.unwrap();
    let DeviceTaskResponse::SystemQuery { reply } = response else {
        panic!("{response:?}");
    };
    assert_eq!(reply.state, "running");
    drop(stream);
    worker.await.unwrap();
    assert_eq!(
        svc.system_query(actor(), id, q.clone())
            .await
            .unwrap()
            .state,
        "running"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let terminal = loop {
        let service = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            service
                .handle_stream(
                    actor(),
                    server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(
                &DeviceTaskRequest::GetSystemQuery {
                    schema_version: DEVICE_TASK_SCHEMA_VERSION,
                    request_id: id,
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        let response: DeviceTaskResponse =
            stream.receive_json(Duration::from_secs(5)).await.unwrap();
        worker.await.unwrap();
        let DeviceTaskResponse::SystemQuery { reply } = response else {
            panic!("{response:?}");
        };
        if reply.state != "running" {
            break *reply;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(terminal.state, "failed");
    assert!(child.0.try_wait().unwrap().is_none());
    assert_eq!(svc.system_query(actor(), id, q).await.unwrap(), terminal);
    let forced = SystemQuery::TerminateProcess {
        pid,
        identity,
        timeout_ms: 100,
        force: true,
    };
    let forced_id = RequestId::new();
    let _ = svc
        .system_query(actor(), forced_id, forced.clone())
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let terminal = loop {
        let r = svc.get_system_query(actor(), forced_id).await.unwrap();
        if r.state != "running" {
            break r;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(terminal.state, "completed");
    child.0.wait().unwrap();
    assert_eq!(
        svc.system_query(actor(), forced_id, forced).await.unwrap(),
        terminal
    );
    let reopened = service(dir.path()).await;
    assert_eq!(
        reopened.get_system_query(actor(), forced_id).await.unwrap(),
        terminal
    );
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn query_result_is_persisted_deduplicated_and_actor_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let id = RequestId::new();
    let q = SystemQuery::Info {
        include_gpu: false,
        sample_cpu: false,
    };
    let r = svc.system_query(actor(), id, q.clone()).await.unwrap();
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(svc.system_query(actor(), id, q.clone()).await.unwrap(), r);
    assert!(
        svc.system_query(actor(), id, SystemQuery::Disks { limit: 1 })
            .await
            .is_err()
    );
    let other = OperatorRef::guest(pab_protocol::EndpointKey::new([99; 32]));
    assert!(svc.get_system_query(other, id).await.is_err());
    let reopened = service(dir.path()).await;
    assert_eq!(reopened.get_system_query(actor(), id).await.unwrap(), r);
}

#[tokio::test]
async fn quota_restart_and_missing_worker_do_not_resample_original_id() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let q = SystemQuery::Disks { limit: 1 };
    let id = RequestId::new();
    let permits = svc
        .system_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let r = svc.system_query(actor(), id, q.clone()).await.unwrap();
    assert_eq!(r.state, "failed");
    drop(permits);
    assert_eq!(svc.system_query(actor(), id, q.clone()).await.unwrap(), r);
    let id = RequestId::new();
    svc.store
        .accept_system_query(actor(), id, &q)
        .await
        .unwrap();
    assert_eq!(
        svc.get_system_query(actor(), id).await.unwrap().state,
        "unconfirmed"
    );
    svc.store.interrupt_read_operations().await.unwrap();
    assert_eq!(
        svc.system_query(actor(), id, q).await.unwrap().state,
        "interrupted"
    );
}

#[tokio::test]
async fn collector_busy_is_bounded_and_other_queries_can_resume() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let q = SystemQuery::Disks { limit: 1 };
    let collector = svc.system_collector.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = collector.lock().unwrap();
        ready_tx.send(()).unwrap();
        let _ = release_rx.recv();
    });
    ready_rx.await.unwrap();
    let id = RequestId::new();
    let r = svc.system_query(actor(), id, q.clone()).await.unwrap();
    assert_eq!(r.state, "failed");
    assert!(r.error.unwrap().contains("executor_busy"));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(
        svc.system_query(actor(), RequestId::new(), q)
            .await
            .unwrap()
            .state,
        "completed"
    );
}

#[tokio::test]
async fn native_query_and_original_result_cross_real_quic_streams() {
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let id = RequestId::new();
    let (a, b, client, server) = pair().await;
    let mut first = None;
    for req in [
        DeviceTaskRequest::SystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
            query: SystemQuery::Process {
                pid: std::process::id(),
                sample_cpu: false,
            },
        },
        DeviceTaskRequest::GetSystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
        },
    ] {
        let service = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            service
                .handle_stream(
                    actor(),
                    server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(&req, Duration::from_secs(5))
            .await
            .unwrap();
        let response: DeviceTaskResponse =
            stream.receive_json(Duration::from_secs(5)).await.unwrap();
        let DeviceTaskResponse::SystemQuery { reply } = response else {
            panic!("{response:?}")
        };
        assert_eq!(reply.state, "completed");
        assert!(matches!(reply.data, Some(SystemQueryData::Process { .. })));
        if let Some(prior) = &first {
            assert_eq!(&reply, prior);
        } else {
            first = Some(reply);
        }
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
    }
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn c2_socket_inventory_and_original_sample_cross_quic_after_socket_closes() {
    use pab_protocol::{ConnectionFilter, SocketProtocol, SystemQueryData};
    let dir = tempfile::tempdir().unwrap();
    let svc = service(dir.path()).await;
    let mut socket = Some(std::net::UdpSocket::bind("127.0.0.1:0").unwrap());
    let port = socket.as_ref().unwrap().local_addr().unwrap().port();
    let id = RequestId::new();
    let q = SystemQuery::Connections {
        filter: ConnectionFilter {
            protocol: Some(SocketProtocol::Udp),
            local_port: Some(port),
            pid: Some(std::process::id()),
            ..Default::default()
        },
        limit: 100,
    };
    let (a, b, client, server) = pair().await;
    let mut first = None;
    for req in [
        DeviceTaskRequest::GetEnvironment {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
        },
        DeviceTaskRequest::SystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
            query: q,
        },
        DeviceTaskRequest::GetSystemQuery {
            schema_version: DEVICE_TASK_SCHEMA_VERSION,
            request_id: id,
        },
    ] {
        let service = svc.clone();
        let server = server.clone();
        let worker = tokio::spawn(async move {
            service
                .handle_stream(
                    actor(),
                    server.accept_bi(Duration::from_secs(5)).await.unwrap(),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        });
        let mut stream = client.open_bi(Duration::from_secs(5)).await.unwrap();
        stream
            .send_json(&req, Duration::from_secs(5))
            .await
            .unwrap();
        let response: DeviceTaskResponse =
            stream.receive_json(Duration::from_secs(5)).await.unwrap();
        match response {
            DeviceTaskResponse::Environment {
                system_query_schema_version,
                ..
            } => assert_eq!(
                system_query_schema_version,
                Some(pab_protocol::SYSTEM_QUERY_SCHEMA_VERSION)
            ),
            DeviceTaskResponse::SystemQuery { reply } => {
                assert_eq!(reply.state, "completed", "{reply:?}");
                assert_eq!(reply.returned_count, 1);
                assert!(matches!(
                    reply.data,
                    Some(SystemQueryData::Connections { .. })
                ));
                if let Some(prior) = &first {
                    assert_eq!(&reply, prior);
                } else {
                    first = Some(reply);
                    drop(socket.take());
                }
            }
            r => panic!("unexpected {r:?}"),
        }
        stream
            .expect_receive_end(Duration::from_secs(5))
            .await
            .unwrap();
        worker.await.unwrap();
    }
    drop(socket);
    assert_eq!(
        svc.get_system_query(actor(), id)
            .await
            .unwrap()
            .returned_count,
        1
    );
    let fresh = svc
        .system_query(
            actor(),
            RequestId::new(),
            SystemQuery::Connections {
                filter: ConnectionFilter {
                    protocol: Some(SocketProtocol::Udp),
                    local_port: Some(port),
                    pid: Some(std::process::id()),
                    ..Default::default()
                },
                limit: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(fresh.returned_count, 0);
    a.close().await;
    b.close().await;
}
