//! Portable journal data; native operations remain platform-specific.
use pab_protocol::{NativeFilePath,RequestId};
use serde::{Deserialize,Serialize};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrivateFileStagingReceipt {
    pub version:u8,
    pub destination:NativeFilePath,
    pub parent_identity:String,
    pub directory_name:String,
    pub directory_identity:String,
    pub payload_identity:String,
}
impl PrivateFileStagingReceipt {
    pub fn validate(&self)->io::Result<()> {
        let invalid=||io::Error::new(io::ErrorKind::InvalidData,"invalid receiving staging receipt");
        if self.version!=1 {return Err(invalid());}
        let id=self.directory_name.strip_prefix(".pab-receive-").and_then(|value|value.parse::<RequestId>().ok()).ok_or_else(invalid)?;
        if self.directory_name!=format!(".pab-receive-{id}"){return Err(invalid());}
        self.destination.validate().map_err(|_|invalid())?;
        for value in [&self.parent_identity,&self.directory_identity,&self.payload_identity] {
            pab_protocol::FileTransferParentConstraint {identity:value.clone()}.validate().map_err(|_|invalid())?;
        }
        Ok(())
    }
}
