use std::sync::atomic::AtomicU32;

struct BudgetBackend {
    inner: Backend,
    budget: Arc<AtomicU32>,
    calls: Arc<AtomicUsize>,
    pending: bool,
    unsupported: bool,
}
struct BudgetSource {
    inner: Box<dyn DesktopSource>,
    budget: Arc<AtomicU32>,
    calls: Arc<AtomicUsize>,
    pending: bool,
    supported: bool,
    unsupported: bool,
}
impl DesktopSource for BudgetSource {
    fn next_frame(&mut self) -> MediaFuture<'_, MediaFrame> {
        self.inner.next_frame()
    }
    fn encoding_budget(&self) -> Option<u32> {
        self.supported.then(|| self.budget.load(Ordering::SeqCst))
    }
    fn set_encoding_budget(&mut self, value: u32) -> MediaFuture<'_, ()> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.pending {
                std::future::pending::<()>().await;
            }
            if self.unsupported {
                self.supported = false;
            } else {
                self.budget.store(value, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}
impl DesktopBackend for BudgetBackend {
    fn screens(&self) -> MediaFuture<'_, Vec<DesktopScreen>> {
        self.inner.screens()
    }
    fn start(&self, config: DesktopVideoConfig) -> MediaFuture<'_, Box<dyn DesktopSource>> {
        Box::pin(async move {
            self.budget.store(0, Ordering::SeqCst);
            Ok(Box::new(BudgetSource {
                inner: self.inner.start(config).await?,
                budget: self.budget.clone(),
                calls: self.calls.clone(),
                pending: self.pending,
                supported: true,
                unsupported: self.unsupported,
            }) as Box<dyn DesktopSource>)
        })
    }
}
fn backend(pending: bool, unsupported: bool) -> Arc<BudgetBackend> {
    Arc::new(BudgetBackend {
        inner: Backend::default(),
        budget: Arc::new(AtomicU32::new(0)),
        calls: Arc::new(AtomicUsize::new(0)),
        pending,
        unsupported,
    })
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn encoding_budget_crosses_verified_helper_and_restores_without_interrupting_frames() {
    let _lock = LOCK.lock().await;
    let backend = backend(false, false);
    let pair = pair(backend.clone()).await;
    let mut source = HelperMediaBackend.start(config()).await.unwrap();
    assert_eq!(source.encoding_budget(), Some(0));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    for value in [3_000_000, 750_000, 0] {
        source.set_encoding_budget(value).await.unwrap();
        assert_eq!(source.encoding_budget(), Some(value));
        assert_eq!(backend.budget.load(Ordering::SeqCst), value);
        assert_eq!(source.next_frame().await.unwrap().bytes.len(), 200_013);
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    drop(source);
    ended(pair, &backend.inner).await;
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn legacy_helper_never_receives_an_unnegotiated_budget_operation() {
    let _lock = LOCK.lock().await;
    let backend = backend(false, false);
    let pair = pair_with_budget_hint(backend.clone(), false, false).await;
    let mut source = HelperMediaBackend.start(config()).await.unwrap();
    assert_eq!(source.encoding_budget(), None);
    assert!(source.set_encoding_budget(750_000).await.is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.next_frame().await.unwrap().bytes.len(), 200_013);
    drop(source);
    ended(pair, &backend.inner).await;
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn unsupported_native_budget_preserves_the_healthy_helper_source() {
    let _lock = LOCK.lock().await;
    let backend = backend(false, true);
    let pair = pair(backend.clone()).await;
    let mut source = HelperMediaBackend.start(config()).await.unwrap();
    source.set_encoding_budget(750_000).await.unwrap();
    assert_eq!(source.encoding_budget(), None);
    assert_eq!(source.next_frame().await.unwrap().bytes.len(), 200_013);
    count(&backend.inner, 1).await;
    drop(source);
    ended(pair, &backend.inner).await;
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an active local graphical user; run on the Linux VM with --ignored"
)]
async fn cancelled_budget_update_retires_the_owned_helper_and_capture() {
    let _lock = LOCK.lock().await;
    let backend = backend(true, false);
    let pair = pair(backend.clone()).await;
    let mut source = HelperMediaBackend.start(config()).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            source.set_encoding_budget(750_000)
        )
        .await
        .is_err()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    drop(source);
    ended(pair, &backend.inner).await;
}
