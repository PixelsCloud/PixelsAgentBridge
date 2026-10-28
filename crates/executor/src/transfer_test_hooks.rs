//! Deterministic opt-in barriers for isolated protocol acceptance only.
use std::{path::{Path,PathBuf},sync::{Arc,atomic::{AtomicBool,Ordering}}};
use tokio::sync::watch;

#[derive(Clone)]
pub struct UploadBarrier {inner:Arc<Inner>}
struct Inner {directory:PathBuf,after:u64,hit:AtomicBool,reached:watch::Sender<Option<u64>>,release:watch::Sender<bool>}
impl UploadBarrier {
    pub fn new(directory:PathBuf,after:u64)->Self {Self {inner:Arc::new(Inner {directory,after,hit:AtomicBool::new(false),reached:watch::channel(None).0,release:watch::channel(false).0})}}
    pub async fn reached(&self)->u64 {let mut receive=self.inner.reached.subscribe();*receive.wait_for(|value|value.is_some()).await.expect("barrier closed").as_ref().unwrap()}
    pub fn release(&self){self.inner.release.send_replace(true);}
    pub(crate) async fn visit(&self,path:&Path,offset:u64)->bool {
        if !path.starts_with(&self.inner.directory) || offset<self.inner.after || self.inner.hit.swap(true,Ordering::AcqRel){return false;}
        self.inner.reached.send_replace(Some(offset));
        let mut release=self.inner.release.subscribe();
        let _=release.wait_for(|value|*value).await;
        true
    }
}
