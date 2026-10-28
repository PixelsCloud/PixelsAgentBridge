use crate::{ExecutionSelection, RequestId, valid_file_hash};
use serde::{Deserialize, Serialize};

pub const FILE_TRANSFER_SCHEMA_VERSION: u16 = 2;
pub const FILE_TRANSFER_NATIVE_SCHEMA_VERSION: u16 = 3;
/// Explicit durable receiver stop acknowledgement (pause/cancel).
pub const FILE_TRANSFER_CONTROL_SCHEMA_VERSION: u16 = 4;
/// Native account/session precondition checked before accepting file IO.
pub const FILE_TRANSFER_IDENTITY_SCHEMA_VERSION:u16=5;
/// Portable destination attributes and retained-source preconditions.
pub const FILE_TRANSFER_METADATA_SCHEMA_VERSION:u16=6;
/// Receiver parent object bound before staging and during publication recovery.
pub const FILE_TRANSFER_PARENT_SCHEMA_VERSION:u16=7;

#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferParentConstraint {pub identity:String}
impl FileTransferParentConstraint {
    pub fn validate(&self)->Result<(),&'static str> {
        if self.identity.is_empty() || self.identity.len()>192 || !self.identity.bytes().all(|byte|byte.is_ascii_alphanumeric() || byte==b':') {
            return Err("invalid destination parent identity");
        }
        Ok(())
    }
}

/// Filesystem identity is independent of cwd, PATH or an OS patch version.
/// It remains strict about the native account and its session generation.
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileExecutionIdentity {
    pub os_family:crate::OsFamily,
    pub path_style:crate::PathStyle,
    pub identity:crate::ExecutionIdentity,
}
impl FileExecutionIdentity {
    pub fn from_context(context:&crate::ExecutionContext)->Result<Self,&'static str> {
        let value=Self {os_family:context.os_family,path_style:context.path_style,identity:context.identity.clone().ok_or("file execution identity is unavailable")?};value.validate()?;Ok(value)
    }
    pub fn validate(&self)->Result<(),&'static str> {
        self.identity.validate()?;
        if !matches!(self.identity.mode,crate::ExecutionMode::Service|crate::ExecutionMode::User){return Err("file operations require service or native user execution");}
        if (self.os_family==crate::OsFamily::Windows)!=(self.path_style==crate::PathStyle::Windows){return Err("file execution platform and path style differ");}
        Ok(())
    }
    pub fn matches_context(&self,context:&crate::ExecutionContext)->bool {
        self.validate().is_ok() && self.os_family==context.os_family && self.path_style==context.path_style && context.identity.as_ref()==Some(&self.identity)
    }
}

/// Portable attributes only: no ownership, ACL or privileged mode bits.
#[derive(Debug,Clone,Default,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferMetadata {
    pub modified_at_unix_ms:Option<i64>,
    pub unix_mode:Option<u32>,
}
impl FileTransferMetadata {
    pub fn validate(&self)->Result<(),&'static str> {
        if self.unix_mode.is_some_and(|mode|mode&!0o777!=0){return Err("only ordinary POSIX permission bits may be transferred");}
        Ok(())
    }
}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferSourceConstraint {
    pub size:crate::FileByteCount,
    pub modified_at_unix_ms:Option<i64>,
}
impl FileTransferSourceConstraint {
    pub fn validate(&self)->Result<(),&'static str> {
        if self.size.0>i64::MAX as u64 {return Err("source file size exceeds the supported range");}
        Ok(())
    }
}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum FileTransferAttribute {ModifiedTime,UnixMode}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferMetadataWarning {pub attribute:FileTransferAttribute,pub code:String,pub message:String}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferMetadataResult {
    pub requested:FileTransferMetadata,
    pub actual:FileTransferMetadata,
    pub warnings:Vec<FileTransferMetadataWarning>,
}
impl FileTransferMetadataResult {
    pub fn validate(&self)->Result<(),&'static str> {
        self.requested.validate()?;self.actual.validate()?;
        if self.warnings.len()>2 || self.warnings.iter().any(|warning|warning.code.is_empty() || warning.code.len()>32 || !warning.code.bytes().all(|byte|byte.is_ascii_lowercase() || byte==b'_') || warning.message.is_empty() || warning.message.len()>1024 || match warning.attribute {FileTransferAttribute::ModifiedTime=>self.requested.modified_at_unix_ms.is_none(),FileTransferAttribute::UnixMode=>self.requested.unix_mode.is_none()}) || self.warnings.len()==2 && self.warnings[0].attribute==self.warnings[1].attribute {return Err("invalid file metadata warnings");}
        for (attribute,changed) in [(FileTransferAttribute::ModifiedTime,self.requested.modified_at_unix_ms.is_some() && self.requested.modified_at_unix_ms!=self.actual.modified_at_unix_ms),(FileTransferAttribute::UnixMode,self.requested.unix_mode.is_some() && self.requested.unix_mode!=self.actual.unix_mode)] {
            if changed && !self.warnings.iter().any(|warning|warning.attribute==attribute){return Err("file metadata mismatch requires an explicit warning");}
        }
        Ok(())
    }
    pub fn validate_for(&self,requested:&FileTransferMetadata)->Result<(),&'static str> {
        self.validate()?;
        if &self.requested!=requested {return Err("file metadata result belongs to different requested attributes");}
        Ok(())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferOptions {
    /// Receiver-side constraint; for downloads this stays on the local client.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub destination_parent:Option<FileTransferParentConstraint>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub source_constraint:Option<FileTransferSourceConstraint>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub metadata:Option<FileTransferMetadata>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub expected_execution:Option<FileExecutionIdentity>,
    #[serde(default)]
    pub execution: ExecutionSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<RequestId>,
}
impl FileTransferOptions {
    pub fn required_schema_version(&self,native:bool)->u16 {
        if self.destination_parent.is_some(){return FILE_TRANSFER_PARENT_SCHEMA_VERSION;}
        required_schema(native,self.expected_execution.is_some(),self.source_constraint.is_some() || self.metadata.is_some())
    }
}
fn required_schema(native:bool,identity:bool,attributes:bool)->u16 {
    if attributes {FILE_TRANSFER_METADATA_SCHEMA_VERSION}
    else if identity {FILE_TRANSFER_IDENTITY_SCHEMA_VERSION}
    else if native {FILE_TRANSFER_NATIVE_SCHEMA_VERSION}
    else {FILE_TRANSFER_SCHEMA_VERSION}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferRequest {
    /// Upload receiver only. Download requests describe the remote source.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub destination_parent:Option<FileTransferParentConstraint>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub source_constraint:Option<FileTransferSourceConstraint>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub metadata:Option<FileTransferMetadata>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub expected_execution:Option<FileExecutionIdentity>,
    pub request_id: RequestId,
    /// Operational identity for v3. `operation.path` remains a bounded display
    /// label and must never be reopened instead of this native value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_path: Option<crate::NativeFilePath>,
    #[serde(default)]
    pub execution: ExecutionSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<RequestId>,
    pub operation: FileTransferOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileTransferOperation {
    Upload {
        path: String,
        size: u64,
        sha256: String,
        overwrite: bool,
    },
    Download {
        path: String,
        offset: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
    },
}
impl FileTransferOperation {
    pub fn path(&self) -> &str {
        match self {
            Self::Upload { path, .. } | Self::Download { path, .. } => path,
        }
    }
    pub fn direction(&self) -> &'static str {
        match self {
            Self::Upload { .. } => "receive",
            Self::Download { .. } => "send",
        }
    }
}
impl FileTransferRequest {
    pub fn required_schema_version(&self)->u16 {
        if self.destination_parent.is_some(){return FILE_TRANSFER_PARENT_SCHEMA_VERSION;}
        required_schema(self.native_path.is_some(),self.expected_execution.is_some(),self.source_constraint.is_some() || self.metadata.is_some())
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(parent)=&self.destination_parent {
            parent.validate()?;
            if self.native_path.is_none() || !matches!(self.operation,FileTransferOperation::Upload {..}) {
                return Err("destination parent constraint requires a native upload receiver");
            }
        }
        if self.source_constraint.is_some() || self.metadata.is_some() {
            if self.native_path.is_none(){return Err("file attributes and source preconditions require a native path");}
            if let Some(metadata)=&self.metadata {metadata.validate()?;}
            if let Some(source)=&self.source_constraint {
                source.validate()?;
                match self.operation {
                    FileTransferOperation::Upload {size,..} if size!=source.size.0=>return Err("upload size differs from the source precondition"),
                    FileTransferOperation::Download {offset,..} if offset>source.size.0=>return Err("download offset exceeds the source precondition"),
                    _=>{},
                }
            }
        }
        if let Some(expected)=&self.expected_execution {
            expected.validate()?;
            if self.native_path.is_none() || (expected.identity.mode==crate::ExecutionMode::Service)!=self.execution.is_service(){return Err("file transfer execution precondition differs from its selection");}
        }
        if !matches!(
            self.execution,
            ExecutionSelection::Service {} | ExecutionSelection::User { .. }
        ) {
            return Err("transfer supports service or user execution only");
        }
        if self.resume_from == Some(self.request_id) {
            return Err("resume_from must reference another request");
        }
        let path = self.operation.path();
        if path.is_empty() || path.len() > 4096 || path.contains('\0') {
            return Err("invalid transfer path");
        }
        if let Some(native) = &self.native_path {
            native.validate()?;
            if native.display_label() != path {
                return Err("native transfer display label mismatch");
            }
        }
        match &self.operation {
            FileTransferOperation::Upload { size, sha256, .. }
                if *size > i64::MAX as u64 || !valid_file_hash(sha256) =>
            {
                Err("invalid upload size or sha256")
            }
            FileTransferOperation::Download {
                offset,
                expected_sha256,
                ..
            } if *offset > i64::MAX as u64
                || expected_sha256
                    .as_deref()
                    .is_some_and(|v| !valid_file_hash(v)) =>
            {
                Err("invalid download offset or sha256")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receiver_parent_is_bounded_versioned_and_never_a_remote_download_constraint() {
        let native=crate::NativeFilePath::Utf8("/fixture/file".into());
        let mut request=FileTransferRequest {destination_parent:Some(FileTransferParentConstraint {identity:"volume:123:456".into()}),source_constraint:None,metadata:None,expected_execution:None,request_id:RequestId::new(),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:4,sha256:"0".repeat(64),overwrite:false}};
        request.validate().unwrap();assert_eq!(request.required_schema_version(),7);
        assert_eq!(serde_json::from_slice::<FileTransferRequest>(&serde_json::to_vec(&request).unwrap()).unwrap(),request);
        for identity in ["".to_owned(),"x".repeat(193),"../other".into(),"v\0x".into()] {
            request.destination_parent.as_mut().unwrap().identity=identity;assert!(request.validate().is_err());
        }
        request.destination_parent.as_mut().unwrap().identity="volume:123".into();
        request.native_path=None;assert!(request.validate().is_err());request.native_path=Some(native.clone());
        request.operation=FileTransferOperation::Download {path:native.display_label(),offset:0,expected_sha256:None};assert!(request.validate().is_err());
        let options=FileTransferOptions {destination_parent:request.destination_parent,..Default::default()};assert_eq!(options.required_schema_version(true),7);
    }
    #[test]
    fn attributes_and_source_preconditions_have_a_separate_capability_and_preserve_absent_wire_fields() {
        let native=crate::NativeFilePath::Utf8("/fixture.bin".into());
        let mut request=FileTransferRequest { destination_parent:None,source_constraint:None,metadata:None,expected_execution:None,request_id:RequestId::new(),native_path:Some(native.clone()),execution:Default::default(),resume_from:None,operation:FileTransferOperation::Upload {path:native.display_label(),size:4,sha256:"0".repeat(64),overwrite:false}};
        let wire=serde_json::to_value(&request).unwrap();assert!(!wire.as_object().unwrap().contains_key("source_constraint"));assert!(!wire.as_object().unwrap().contains_key("metadata"));assert_eq!(request.required_schema_version(),3);
        request.source_constraint=Some(FileTransferSourceConstraint {size:crate::FileByteCount(5),modified_at_unix_ms:Some(-1)});assert!(request.validate().is_err());
        request.source_constraint.as_mut().unwrap().size=crate::FileByteCount(4);request.validate().unwrap();assert_eq!(request.required_schema_version(),6);
        request.metadata=Some(FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o4755)});assert!(request.validate().is_err());request.metadata.as_mut().unwrap().unix_mode=Some(0o640);request.validate().unwrap();
        let wire=serde_json::to_value(&request).unwrap();assert_eq!(wire["source_constraint"]["size"],"4");assert_eq!(serde_json::from_value::<FileTransferRequest>(wire).unwrap(),request);
        request.operation=FileTransferOperation::Download {path:native.display_label(),offset:5,expected_sha256:None};assert!(request.validate().is_err());
        request.native_path=None;assert!(request.validate().is_err());
        let mut options=FileTransferOptions::default();assert_eq!(options.required_schema_version(false),2);options.resume_from=Some(RequestId::new());assert_eq!(options.required_schema_version(false),2);options.execution=ExecutionSelection::User {context_ref:crate::ExecutionContextRef::new()};assert_eq!(options.required_schema_version(false),2);assert_eq!(options.required_schema_version(true),3);options.source_constraint=request.source_constraint;assert_eq!(options.required_schema_version(true),6);
    }
    #[test]
    fn missing_or_unrelated_attribute_outcomes_cannot_claim_metadata_success() {
        let requested=FileTransferMetadata {modified_at_unix_ms:Some(123),unix_mode:None};
        let mut result=FileTransferMetadataResult {requested:requested.clone(),actual:FileTransferMetadata {modified_at_unix_ms:Some(122),unix_mode:None},warnings:vec![]};assert!(result.validate().is_err());
        result.warnings.push(FileTransferMetadataWarning {attribute:FileTransferAttribute::ModifiedTime,code:"precision_loss".into(),message:"Stored with lower precision".into()});result.validate_for(&requested).unwrap();
        assert!(result.validate_for(&FileTransferMetadata::default()).is_err());
        result.warnings[0].attribute=FileTransferAttribute::UnixMode;assert!(result.validate().is_err());
    }
    #[test]
    fn file_identity_allows_environment_changes_but_never_a_different_native_session() {
        use crate::*;
        let mut context=ExecutionContext {os_family:OsFamily::Windows,os_name:"Windows".into(),os_version:"1".into(),architecture:CpuArchitecture::X86_64,execution_scope:ExecutionScope::Native,identity:Some(ExecutionIdentity {mode:ExecutionMode::User,account_id:"S-1-5-21-123-456-789-1001".into(),account_name:"fixture\\user".into(),home:"C:\\Users\\fixture".into(),primary_group:None,session_id:Some("1".into()),logon_id:Some("login-1".into()),environment_source:ExecutionEnvironmentSource::NativeAccount}),path_style:PathStyle::Windows,interpreter:None,cwd:Some("C:\\Users\\fixture".into()),environment_revision:"environment-1".into()};
        let identity=FileExecutionIdentity::from_context(&context).unwrap();context.cwd=Some("D:\\files".into());context.environment_revision="environment-2".into();context.os_version="patched".into();assert!(identity.matches_context(&context));
        let original=context.clone();context.identity.as_mut().unwrap().logon_id=Some("login-2".into());assert!(!identity.matches_context(&context));context=original.clone();context.identity.as_mut().unwrap().account_id="S-1-5-18".into();assert!(!identity.matches_context(&context));context=original.clone();context.identity.as_mut().unwrap().session_id=Some("2".into());assert!(!identity.matches_context(&context));context=original;context.identity=None;assert!(FileExecutionIdentity::from_context(&context).is_err());
    }
    #[test]
    fn metadata_warnings_are_bounded_and_privileged_modes_are_not_transferable() {
        let mut result=FileTransferMetadataResult {requested:FileTransferMetadata {modified_at_unix_ms:Some(-1),unix_mode:Some(0o640)},actual:FileTransferMetadata {modified_at_unix_ms:Some(-1),unix_mode:None},warnings:vec![FileTransferMetadataWarning {attribute:FileTransferAttribute::UnixMode,code:"unsupported".into(),message:"POSIX permissions unavailable".into()}]};result.validate().unwrap();
        let bytes=serde_json::to_vec(&result).unwrap();assert_eq!(serde_json::from_slice::<FileTransferMetadataResult>(&bytes).unwrap(),result);
        result.warnings.push(result.warnings[0].clone());assert!(result.validate().is_err());result.warnings.pop();result.requested.unix_mode=Some(0o2640);assert!(result.validate().is_err());
    }
    #[test]
    fn native_transfer_identity_is_separate_from_its_bounded_display_label() {
        let native = crate::NativeFilePath::WindowsWide(vec![67, 58, 92, 0xd800]);
        let mut request = FileTransferRequest { destination_parent:None, source_constraint:None,metadata:None,expected_execution:None,
            request_id: RequestId::new(),
            native_path: Some(native.clone()),
            execution: ExecutionSelection::Service {},
            resume_from: None,
            operation: FileTransferOperation::Upload {
                path: native.display_label(),
                size: 0,
                sha256: "0".repeat(64),
                overwrite: false,
            },
        };
        request.validate().unwrap();
        let wire = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<FileTransferRequest>(&wire).unwrap(),
            request
        );
        request.native_path = Some(crate::NativeFilePath::Utf8("D:\\different.bin".into()));
        assert!(request.validate().is_err());
        request.native_path = None;
        assert!(
            !serde_json::to_value(&request)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("native_path")
        );
        let long = crate::NativeFilePath::Utf8(format!("/{}", "中文".repeat(2000)));
        assert!(long.display_label().len() <= 4096);
        assert!(long.display_label().ends_with("..."));
    }
    #[test]
    fn transfer_execution_has_strict_identity_and_distinct_wire_variant() {
        let value = serde_json::json!({"request_id":RequestId::new(),"execution":{"mode":"user","context_ref":crate::ExecutionContextRef::new()},"operation":{"direction":"upload","path":"/tmp/new.bin","size":0,"sha256":"0".repeat(64),"overwrite":false}});
        let request: FileTransferRequest = serde_json::from_value(value.clone()).unwrap();
        request.validate().unwrap();
        let wire = serde_json::to_value(crate::DeviceTaskRequest::TransferFile {
            schema_version: crate::DEVICE_TASK_SCHEMA_VERSION,
            request: request.clone(),
        })
        .unwrap();
        assert_eq!(wire["type"], "transfer_file");
        for field in ["username", "password", "identity"] {
            let mut wrong = value.clone();
            wrong["execution"][field] = serde_json::json!("claimed");
            assert!(serde_json::from_value::<FileTransferRequest>(wrong).is_err());
        }
        let mut invalid = request;
        invalid.resume_from = Some(invalid.request_id);
        assert!(invalid.validate().is_err());
        invalid.resume_from = None;
        invalid.execution = ExecutionSelection::DesktopUser {
            context_ref: crate::ExecutionContextRef::new(),
        };
        assert!(invalid.validate().is_err());
    }
}
