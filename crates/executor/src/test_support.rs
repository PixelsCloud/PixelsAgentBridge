//! Opt-in in-process protocol harness. It admits only the actor supplied by the
//! fixture; production listeners use DeviceSessionAcceptor authentication.
use pab_protocol::{DeviceRef,ExecutionContext,OperatorRef};
use std::{path::Path,time::Duration};
pub use crate::transfer_test_hooks::UploadBarrier;

#[derive(Clone)]
pub struct ProtocolService(crate::task_service::TaskService);
impl ProtocolService {
    pub fn with_upload_barrier(mut self,barrier:UploadBarrier)->Self {self.0.transfer_barrier=Some(barrier);self}
    pub async fn stop_pending(&self,id:pab_protocol::RequestId)->Result<bool,String> {self.0.fixture_stop_pending(id).await}
    pub async fn open(database:&Path,device:DeviceRef,context:ExecutionContext)->Result<Self,String> {
        crate::task_service::TaskService::open(database,device,context).await.map(Self).map_err(|error|error.to_string())
    }
    pub async fn handle_stream(&self,actor:OperatorRef,stream:pab_transport::PabBiStream,timeout:Duration)->Result<(),String> {
        self.0.handle_stream(actor,stream,timeout).await.map_err(|error|error.to_string())
    }
}
