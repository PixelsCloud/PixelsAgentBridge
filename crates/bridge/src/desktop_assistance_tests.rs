use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    now: Arc<AtomicI64>,
    owner: Uuid,
    desktop: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicI64::new(1_000_000));
        let clock = now.clone();
        let mut store = Store::open(&dir.path().join("assistance.db"), "account:1".into())
            .await
            .unwrap();
        store.clock = Arc::new(move || clock.load(Ordering::Relaxed));
        let owner = Uuid::new_v4();
        let desktop = Uuid::new_v4();
        store.heartbeat_requester(owner).await.unwrap();
        Self {
            _dir: dir,
            store,
            now,
            owner,
            desktop,
        }
    }
    fn request(&self) -> Request {
        Request {
            request_id: Uuid::new_v4(),
            device_code: "123456789".parse().unwrap(),
            reason: "Generated assistance fixture".into(),
            expected_action: "Inspect generated UI".into(),
            screen_id: Some("display-2".into()),
            expires_in_seconds: 900,
        }
    }
    async fn create(&self) -> Snapshot {
        self.store
            .request(self.owner, self.request())
            .await
            .unwrap()
    }
    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn delivery_requires_a_live_desktop_and_explicit_ui_ack() {
    let f = Fixture::new().await;
    let initial = f.create().await;
    assert_eq!(initial.delivery, Delivery::DesktopNotRunning);
    assert_eq!(initial.status, Status::Waiting);
    f.store.heartbeat_desktop(f.desktop).await.unwrap();
    let pending = f.store.pending(f.desktop).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].delivery, Delivery::Pending);
    assert!(matches!(
        f.store.complete(f.desktop, initial.assistance_id).await,
        Err(Error::NotActive)
    ));
    let displayed = f
        .store
        .displayed(f.desktop, initial.assistance_id)
        .await
        .unwrap();
    assert_eq!(displayed.delivery, Delivery::Displayed);
    assert_eq!(displayed.status, Status::Waiting);
    assert_eq!(
        f.store
            .displayed(f.desktop, initial.assistance_id)
            .await
            .unwrap()
            .revision,
        displayed.revision
    );
    f.store
        .activate(f.desktop, initial.assistance_id, "desktop-viewer-test")
        .await
        .unwrap();
    let completed = f
        .store
        .complete(f.desktop, initial.assistance_id)
        .await
        .unwrap();
    assert_eq!(completed.status, Status::Completed);
    assert_eq!(completed.outcome.as_deref(), Some("user_completed"));
    assert!(f.store.pending(f.desktop).await.unwrap().is_empty());
    assert_eq!(
        f.store
            .cancel(f.owner, initial.assistance_id)
            .await
            .unwrap()
            .status,
        Status::Completed
    );
}

#[tokio::test]
async fn concurrent_duplicate_requests_share_one_id_and_cannot_change_arguments() {
    let f = Fixture::new().await;
    let request = f.request();
    let (a, b) = tokio::join!(
        f.store.request(f.owner, request.clone()),
        f.store.request(f.owner, request.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.assistance_id, b.assistance_id);
    let mut changed = request.clone();
    changed.expected_action = "Different operation".into();
    assert!(matches!(
        f.store.request(f.owner, changed).await,
        Err(Error::Conflict)
    ));
    let cancelled = f.store.cancel(f.owner, a.assistance_id).await.unwrap();
    let repeat = f.store.request(f.owner, request).await.unwrap();
    assert_eq!(repeat.status, Status::Cancelled);
    assert_eq!(repeat.revision, cancelled.revision);
    let replay = f
        .store
        .replay(f.owner, &repeat.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.assistance_id, repeat.assistance_id);
    assert_eq!(replay.status, Status::Cancelled);
}

#[tokio::test]
async fn closing_or_restarting_desktop_never_completes_a_request() {
    let f = Fixture::new().await;
    f.store.heartbeat_desktop(f.desktop).await.unwrap();
    let id = f.create().await.assistance_id;
    f.store
        .activate(f.desktop, id, "desktop-viewer-test")
        .await
        .unwrap();
    f.store
        .window_closed(f.desktop, "desktop-viewer-unrelated")
        .await
        .unwrap();
    assert_eq!(
        f.store.status(f.owner, id).await.unwrap().status,
        Status::Active
    );
    f.store
        .window_closed(f.desktop, "desktop-viewer-test")
        .await
        .unwrap();
    let waiting = f.store.status(f.owner, id).await.unwrap();
    assert_eq!(waiting.status, Status::Waiting);
    assert!(waiting.window_label.is_none());
    f.store
        .activate(f.desktop, id, "desktop-viewer-test")
        .await
        .unwrap();
    f.store.stop_desktop(f.desktop).await.unwrap();
    let stopped = f.store.status(f.owner, id).await.unwrap();
    assert_eq!(stopped.status, Status::Waiting);
    assert_eq!(stopped.delivery, Delivery::DesktopNotRunning);
    let replacement = Uuid::new_v4();
    f.store.heartbeat_desktop(replacement).await.unwrap();
    assert!(matches!(
        f.store.complete(f.desktop, id).await,
        Err(Error::ParticipantGone)
    ));
    assert!(matches!(
        f.store.complete(replacement, id).await,
        Err(Error::NotActive)
    ));
    assert_eq!(
        f.store.pending(replacement).await.unwrap()[0].delivery,
        Delivery::Pending
    );
}

#[tokio::test]
async fn account_revision_service_store_and_requester_are_isolated() {
    let f = Fixture::new().await;
    let id = f.create().await.assistance_id;
    let stranger = Uuid::new_v4();
    f.store.heartbeat_requester(stranger).await.unwrap();
    assert!(matches!(
        f.store.status(stranger, id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        f.store.cancel(stranger, id).await,
        Err(Error::NotFound)
    ));
    let mut revision = f.store.clone();
    revision.scope = "account:2".into();
    assert!(matches!(
        revision.status(f.owner, id).await,
        Err(Error::NotFound)
    ));
    revision.heartbeat_desktop(f.desktop).await.unwrap();
    assert!(revision.pending(f.desktop).await.unwrap().is_empty());
    let other = Store::open(&f._dir.path().join("other-service.db"), "account:1".into())
        .await
        .unwrap();
    assert!(matches!(
        other.status(f.owner, id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        f.store.status(f.owner, id).await.unwrap().status,
        Status::Waiting
    );
}

#[tokio::test]
async fn requester_exit_and_lease_expiry_cancel_only_its_own_requests() {
    let f = Fixture::new().await;
    let first = f.create().await.assistance_id;
    let second_owner = Uuid::new_v4();
    f.store.heartbeat_requester(second_owner).await.unwrap();
    let second = f
        .store
        .request(second_owner, f.request())
        .await
        .unwrap()
        .assistance_id;
    f.store.stop_requester(f.owner).await.unwrap();
    assert_eq!(
        f.store
            .status(f.owner, first)
            .await
            .unwrap()
            .outcome
            .as_deref(),
        Some("requester_unavailable")
    );
    assert_eq!(
        f.store.status(second_owner, second).await.unwrap().status,
        Status::Waiting
    );
    f.advance(LEASE_MS + 1);
    // A late heartbeat cannot revive a request whose requester disappeared.
    f.store.heartbeat_requester(second_owner).await.unwrap();
    assert_eq!(
        f.store.status(second_owner, second).await.unwrap().status,
        Status::Cancelled
    );
}

#[tokio::test]
async fn desktop_crash_returns_to_waiting_and_old_instance_cannot_finish() {
    let f = Fixture::new().await;
    f.store.heartbeat_desktop(f.desktop).await.unwrap();
    let id = f.create().await.assistance_id;
    f.store
        .activate(f.desktop, id, "desktop-viewer-test")
        .await
        .unwrap();
    let other = Uuid::new_v4();
    assert!(matches!(
        f.store.heartbeat_desktop(other).await,
        Err(Error::DesktopBusy)
    ));
    f.advance(20_000);
    f.store.heartbeat_requester(f.owner).await.unwrap();
    f.advance(15_000);
    f.store.heartbeat_desktop(other).await.unwrap();
    let status = f.store.status(f.owner, id).await.unwrap();
    assert_eq!(status.status, Status::Waiting);
    assert!(status.window_label.is_none());
    assert!(matches!(
        f.store.activate(f.desktop, id, "desktop-viewer-test").await,
        Err(Error::ParticipantGone)
    ));
}

#[tokio::test]
async fn deadline_wins_over_completion_and_expired_rows_are_reaped() {
    let f = Fixture::new().await;
    f.store.heartbeat_desktop(f.desktop).await.unwrap();
    let mut req = f.request();
    req.expires_in_seconds = 1;
    let id = f.store.request(f.owner, req).await.unwrap().assistance_id;
    f.store
        .activate(f.desktop, id, "desktop-viewer-test")
        .await
        .unwrap();
    f.advance(1000);
    assert_eq!(
        f.store.complete(f.desktop, id).await.unwrap().status,
        Status::Expired
    );
    f.advance(RETAIN_MS + 1);
    f.store.heartbeat_requester(f.owner).await.unwrap();
    assert!(matches!(
        f.store.status(f.owner, id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn malformed_requests_do_not_create_prompts_and_cancel_does_not_own_windows() {
    let f = Fixture::new().await;
    f.store.heartbeat_desktop(f.desktop).await.unwrap();
    for request in [
        Request {
            reason: " ".into(),
            ..f.request()
        },
        Request {
            expires_in_seconds: 0,
            ..f.request()
        },
        Request {
            screen_id: Some(String::new()),
            ..f.request()
        },
    ] {
        assert!(matches!(
            f.store.request(f.owner, request).await,
            Err(Error::Invalid)
        ));
    }
    assert!(f.store.pending(f.desktop).await.unwrap().is_empty());
    let one = f.create().await.assistance_id;
    let two = f.create().await.assistance_id;
    for id in [one, two] {
        f.store
            .activate(f.desktop, id, "desktop-viewer-shared")
            .await
            .unwrap();
    }
    f.store.cancel(f.owner, one).await.unwrap();
    assert_eq!(
        f.store.status(f.owner, two).await.unwrap().status,
        Status::Active
    );
    assert_eq!(
        f.store
            .status(f.owner, two)
            .await
            .unwrap()
            .window_label
            .as_deref(),
        Some("desktop-viewer-shared")
    );
}
