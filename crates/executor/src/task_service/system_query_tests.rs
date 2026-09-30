use super::*;
use crate::task_service::transfer_tests::{actor, pair, service};
use pab_protocol::{
    DEVICE_TASK_SCHEMA_VERSION, DeviceTaskRequest, DeviceTaskResponse, RequestId, SystemQuery,
    SystemQueryData,
};
use std::time::Duration;

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
