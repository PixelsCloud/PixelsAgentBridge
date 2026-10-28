//! Deterministic barriers compiled only into the libtest binary.
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use tokio::sync::watch;
#[derive(Clone)]
pub(crate) struct PublicationBarrier {inner:Arc<Inner>}
struct Inner {hit:AtomicBool,reached:watch::Sender<bool>,release:watch::Sender<bool>,cancelled:watch::Sender<bool>}
impl PublicationBarrier {
    pub(crate) fn new()->Self {Self {inner:Arc::new(Inner {hit:AtomicBool::new(false),reached:watch::channel(false).0,release:watch::channel(false).0,cancelled:watch::channel(false).0})}}
    pub(crate) async fn reached(&self){let mut receive=self.inner.reached.subscribe();let _=receive.wait_for(|value|*value).await.unwrap();}
    pub(crate) async fn cancelled(&self){let mut receive=self.inner.cancelled.subscribe();let _=receive.wait_for(|value|*value).await.unwrap();}
    pub(crate) fn signal_cancel(&self){self.inner.cancelled.send_replace(true);}
    pub(crate) fn release(&self){self.inner.release.send_replace(true);}
    pub(crate) async fn enter(&self) {
        if self.inner.hit.swap(true,Ordering::AcqRel){return;}
        self.inner.reached.send_replace(true);let mut release=self.inner.release.subscribe();let _=release.wait_for(|value|*value).await.unwrap();
    }
}
