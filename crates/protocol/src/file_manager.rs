//! File-manager contracts preserve native path identity and integer precision.
use crate::{ExecutionSelection, RequestId};
use serde::{Deserialize, Serialize};

pub const FILE_MANAGER_SCHEMA_VERSION: u16 = 1;
pub const MAX_NATIVE_PATH_UNITS: usize = 8192;
pub const MAX_FILE_MANAGER_PAGE: u16 = 256;
pub const FILE_MANAGER_PAGE_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "encoding",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NativeFilePath {
    Utf8(String),
    UnixBytes(#[serde(with = "crate::native_path_encoding::unix_bytes")] Vec<u8>),
    WindowsWide(#[serde(with = "crate::native_path_encoding::windows_wide")] Vec<u16>),
}
impl NativeFilePath {
    pub fn validate_component(&self) -> Result<(), &'static str> {
        self.validate()?;
        let invalid = match self {
            // Backslash is a valid POSIX filename byte. Windows adapters also
            // validate their native separator before performing any operation.
            Self::Utf8(name) => name == "." || name == ".." || name.contains('/'),
            Self::UnixBytes(name) => name == b"." || name == b".." || name.contains(&b'/'),
            Self::WindowsWide(name) => name == &[46] || name == &[46,46] || name.iter().any(|unit| *unit == 47 || *unit == 92),
        };
        if invalid { Err("expected a single native filename") } else { Ok(()) }
    }
    /// Human-readable only. Filesystem adapters must use the original encoding.
    pub fn display_label(&self) -> String {
        let mut text = match self {
            Self::Utf8(value) => value.clone(),
            Self::UnixBytes(value) => value
                .iter()
                .map(|byte| {
                    if byte.is_ascii_graphic() && *byte != b'\\' {
                        char::from(*byte).to_string()
                    } else {
                        format!("\\x{byte:02x}")
                    }
                })
                .collect(),
            Self::WindowsWide(value) => char::decode_utf16(value.iter().copied())
                .map(|unit| {
                    unit.map(|c| c.to_string()).unwrap_or_else(|error| {
                        format!("\\u{{{:04x}}}", error.unpaired_surrogate())
                    })
                })
                .collect(),
        };
        if text.len() > 4096 {
            let end = text
                .char_indices()
                .map(|(offset, _)| offset)
                .take_while(|offset| *offset <= 4093)
                .last()
                .unwrap_or(0);
            text.truncate(end);
            text.push_str("...");
        }
        text
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let (length, nul) = match self {
            Self::Utf8(value) => (value.len(), value.contains('\0')),
            Self::UnixBytes(value) => (value.len(), value.contains(&0)),
            Self::WindowsWide(value) => (value.len(), value.contains(&0)),
        };
        if length == 0 || length > MAX_NATIVE_PATH_UNITS || nul {
            Err("invalid native path length or NUL")
        } else {
            Ok(())
        }
    }
}
impl From<String> for NativeFilePath { fn from(value: String) -> Self { Self::Utf8(value) } }
impl From<&str> for NativeFilePath { fn from(value: &str) -> Self { Self::Utf8(value.into()) } }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileByteCount(pub u64);
impl Serialize for FileByteCount {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for FileByteCount {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || value.len() > 20
            || !value.bytes().all(|b| b.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(serde::de::Error::custom(
                "expected canonical u64 decimal string",
            ));
        }
        value.parse().map(Self).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileManagerSort {
    #[default]
    Name,
    Size,
    Modified,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileManagerFilter {
    #[serde(default)]
    pub sort: FileManagerSort,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub query: String,
}
impl FileManagerFilter {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.query.len() > 1024 || self.query.contains('\0') {
            Err("invalid directory filter")
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileManagerRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_execution: Option<crate::FileExecutionIdentity>,
    pub schema_version: u16,
    pub request_id: RequestId,
    #[serde(default)]
    pub execution: ExecutionSelection,
    pub operation: FileManagerOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileManagerOperation {
    Locations {},
    OpenDirectory {
        path: NativeFilePath,
    },
    ReadDirectory {
        snapshot_id: RequestId,
        after: Option<String>,
        limit: u16,
        filter: FileManagerFilter,
    },
    CloseDirectory {
        snapshot_id: RequestId,
    },
    Inspect {
        path: NativeFilePath,
    },
    InspectDirectoryObject {
        path: NativeFilePath,
    },
    InspectPathIdentity {
        path: NativeFilePath,
    },
    SetDirectoryMetadata {
        path: NativeFilePath,
        expected_object: String,
        metadata: crate::FileTransferMetadata,
    },
    /// Exclusive creation or binding of a previously observed existing object.
    /// The returned object identity belongs to the original durable operation.
    PrepareDirectory {
        path:NativeFilePath,
        expected_parent:String,
        existing_object:Option<String>,
    },
    CreateDirectory {
        parent: NativeFilePath,
        name: NativeFilePath,
    },
    Rename {
        path: NativeFilePath,
        name: NativeFilePath,
    },
    Delete {
        path: NativeFilePath,
        recursive: bool,
    },
}
impl FileManagerRequest {
    /// Reuse authenticated filesystem acceptance and request deduplication.
    /// This display path never identifies the operational native object.
    pub fn filesystem_request(&self) -> crate::FileSystemRequest {
        crate::FileSystemRequest {
            request_id: self.request_id,
            execution: self.execution,
            path: "file-manager".into(),
            operation: crate::FileSystemAction::Managed {
                expected_execution: self.expected_execution.clone(),
                operation: self.operation.clone(),
            },
            payload_size: 0,
            payload_sha256: None,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != FILE_MANAGER_SCHEMA_VERSION {
            return Err("unsupported file-manager schema version");
        }
        if let Some(expected) = &self.expected_execution {
            expected.validate()?;
            if expected.identity.mode != self.execution.mode() {
                return Err("file-manager execution mode differs from the pinned identity");
            }
        }
        if !matches!(
            self.execution,
            ExecutionSelection::Service {} | ExecutionSelection::User { .. }
        ) {
            return Err("file manager requires service or user execution");
        }
        match &self.operation {
            FileManagerOperation::Locations {} => Ok(()),
            FileManagerOperation::OpenDirectory { path } => path.validate(),
            FileManagerOperation::ReadDirectory {
                after,
                limit,
                filter,
                ..
            } => {
                if *limit == 0
                    || *limit > MAX_FILE_MANAGER_PAGE
                    || after
                        .as_ref()
                        .is_some_and(|value| value.is_empty() || value.len() > 256)
                {
                    return Err("invalid file-manager page bounds");
                }
                filter.validate()
            }
            FileManagerOperation::CloseDirectory { .. } => Ok(()),
            FileManagerOperation::Inspect { path } | FileManagerOperation::InspectDirectoryObject { path } | FileManagerOperation::InspectPathIdentity { path } | FileManagerOperation::Delete { path, .. } => {
                path.validate()
            }
            FileManagerOperation::SetDirectoryMetadata { path, expected_object, metadata } => {
                path.validate()?;
                validate_object_identity(expected_object)?;
                metadata.validate()
            }
            FileManagerOperation::PrepareDirectory {path,expected_parent,existing_object} => {
                path.validate()?;validate_object_identity(expected_parent)?;
                if let Some(identity)=existing_object {validate_object_identity(identity)?;}
                Ok(())
            }
            FileManagerOperation::CreateDirectory { parent: path, name }
            | FileManagerOperation::Rename { path, name } => {
                path.validate()?;
                name.validate_component()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManagerReply {
    pub schema_version: u16,
    pub request_id: RequestId,
    pub execution_context: Option<crate::ExecutionContext>,
    pub result: FileManagerResult,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FileManagerResult {
    DirectoryPrepared {path:NativeFilePath,identity:String,created:bool},
    Locations { locations: FileManagerLocations },
    Opened { snapshot_id: RequestId },
    Directory { page: ManagedDirectoryPage },
    Entry { entry: ManagedFileEntry },
    DirectoryObject { path: NativeFilePath, identity: String, metadata: crate::FileTransferMetadata },
    PathIdentity { path: NativeFilePath, identity: FilePathIdentity },
    DirectoryMetadata { path: NativeFilePath, identity: String, metadata: crate::FileTransferMetadataResult },
    Applied { path: NativeFilePath },
    Closed { snapshot_id: RequestId },
    Error { error: crate::FileSystemError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedFileKind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFileEntry {
    pub id: RequestId,
    pub name: String,
    pub path: NativeFilePath,
    pub kind: ManagedFileKind,
    pub size: Option<FileByteCount>,
    pub modified_at_unix_ms: Option<i64>,
    pub hidden: bool,
    pub readonly: bool,
    pub unix_mode: Option<u32>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDirectoryPage {
    pub snapshot_id: RequestId,
    pub path: NativeFilePath,
    pub entries: Vec<ManagedFileEntry>,
    pub next_after: Option<String>,
    pub total_entries: FileByteCount,
    pub observed_at_unix_ms: i64,
    /// Whether enumeration finished; `next_after` independently marks paging.
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManagerLocations {
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub filesystem_scope: Option<String>,
    pub path_style: crate::PathStyle,
    pub home: Option<NativeFilePath>,
    pub roots: Vec<FileManagerRoot>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManagerRoot {
    pub name: String,
    pub path: NativeFilePath,
}

/// Read-only native object observations, scoped to one host boot/installation.
/// Ancestors contain existing directories only; an absent target is allowed.
/// These are preflight observations, never mutation authorization or a CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePathIdentity {
    pub filesystem_scope: String,
    pub object: Option<String>,
    pub kind: Option<ManagedFileKind>,
    pub ancestors: Vec<String>,
}
fn validate_filesystem_scope(scope: &str) -> Result<(), &'static str> {
    if scope.len()!=64 || !scope.bytes().all(|value| value.is_ascii_hexdigit()) { Err("invalid filesystem scope") } else { Ok(()) }
}
impl FilePathIdentity {
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_filesystem_scope(&self.filesystem_scope)?;
        if self.object.is_some()!=self.kind.is_some() || self.ancestors.len()>256 { return Err("invalid filesystem object observation"); }
        if let Some(object)=&self.object { validate_object_identity(object)?; }
        for ancestor in &self.ancestors { validate_object_identity(ancestor)?; }
        Ok(())
    }
    pub fn validate_copy_destination(&self, destination: &Self) -> Result<(), &'static str> {
        self.validate()?; destination.validate()?;
        let object=self.object.as_ref().ok_or("source no longer exists")?;
        if self.filesystem_scope!=destination.filesystem_scope { return Ok(()); }
        if destination.object.as_ref()==Some(object) { return Err("Source and destination are the same filesystem object"); }
        if self.kind==Some(ManagedFileKind::Directory) && destination.ancestors.contains(object) { return Err("A directory cannot be copied into itself or its descendants"); }
        Ok(())
    }
}

impl FileManagerReply {
    pub fn validate_for(&self, request: &FileManagerRequest) -> Result<(), &'static str> {
        request.validate()?;
        if self.schema_version != FILE_MANAGER_SCHEMA_VERSION
            || self.request_id != request.request_id
        {
            return Err("file-manager reply identity mismatch");
        }
        if !matches!(self.result, FileManagerResult::Error { .. }) && request.expected_execution.as_ref().is_some_and(|expected|self.execution_context.as_ref().is_none_or(|actual|!expected.matches_context(actual))) {
            return Err("file-manager reply execution identity mismatch");
        }
        match (&request.operation, &self.result) {
            (FileManagerOperation::PrepareDirectory {path,existing_object,..},FileManagerResult::DirectoryPrepared {path:actual,identity,created}) if path==actual && *created==existing_object.is_none() && existing_object.as_ref().is_none_or(|expected|identity==expected) => validate_object_identity(identity),
            (FileManagerOperation::InspectPathIdentity {path}, FileManagerResult::PathIdentity {path:actual,identity}) if path==actual => identity.validate(),
            (FileManagerOperation::InspectDirectoryObject { path }, FileManagerResult::DirectoryObject { path: actual, identity, metadata }) if path == actual => {
                validate_object_identity(identity)?;
                metadata.validate()
            }
            (FileManagerOperation::SetDirectoryMetadata { path, expected_object, metadata }, FileManagerResult::DirectoryMetadata { path: actual, identity, metadata: result }) if path == actual && identity == expected_object => {
                result.validate_for(metadata)
            }
            (FileManagerOperation::Locations {}, FileManagerResult::Locations { locations }) => {
                if let Some(scope)=&locations.filesystem_scope { validate_filesystem_scope(scope)?; }
                if locations.roots.is_empty()
                    || locations.roots.len() > 64
                    || self
                        .execution_context
                        .as_ref()
                        .is_some_and(|context| context.path_style != locations.path_style)
                {
                    return Err("invalid file location roots");
                }
                if let Some(home) = &locations.home {
                    home.validate()?;
                }
                for root in &locations.roots {
                    root.path.validate()?;
                    if root.name.is_empty() || root.name.len() > 1024 {
                        return Err("invalid file root label");
                    }
                }
                Ok(())
            }
            (FileManagerOperation::OpenDirectory { .. }, FileManagerResult::Opened { .. }) => {
                Ok(())
            }
            (
                FileManagerOperation::ReadDirectory {
                    snapshot_id,
                    after,
                    limit,
                    ..
                },
                FileManagerResult::Directory { page },
            ) => {
                if page.snapshot_id != *snapshot_id
                    || page.entries.len() > *limit as usize
                    || page.total_entries.0 < page.entries.len() as u64
                    || page.next_after.as_ref().is_some_and(|next| {
                        next.is_empty() || next.len() > 256 || Some(next) == after.as_ref()
                    })
                    || page.next_after.is_some() && page.entries.is_empty()
                {
                    return Err("invalid managed directory bounds or cursor");
                }
                page.path.validate()?;
                let mut ids = std::collections::HashSet::new();
                for entry in &page.entries {
                    entry.validate()?;
                    if !ids.insert(entry.id) {
                        return Err("duplicate managed directory entry");
                    }
                }
                Ok(())
            }
            (FileManagerOperation::Inspect { path }, FileManagerResult::Entry { entry })
                if *path == entry.path =>
            {
                entry.validate()
            }
            (
                FileManagerOperation::CloseDirectory { snapshot_id },
                FileManagerResult::Closed {
                    snapshot_id: closed,
                },
            ) if snapshot_id == closed => Ok(()),
            (
                FileManagerOperation::CreateDirectory { .. } | FileManagerOperation::Rename { .. },
                FileManagerResult::Applied { path },
            ) => path.validate(),
            (
                FileManagerOperation::Delete { path, .. },
                FileManagerResult::Applied { path: deleted },
            ) if path == deleted => path.validate(),
            (_, FileManagerResult::Error { error })
                if error.code.len() <= 128
                    && error.phase.len() <= 128
                    && error.message.len() <= 8192 =>
            {
                Ok(())
            }
            _ => Err("file-manager result does not match requested action"),
        }
    }
}
fn validate_object_identity(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > 192 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b':') {
        return Err("invalid filesystem object identity");
    }
    Ok(())
}
impl ManagedFileEntry {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.path.validate()?;
        if self.name.len() > MAX_NATIVE_PATH_UNITS * 8
            || self.name.contains('\0')
            || self.error.as_ref().is_some_and(|error| error.len() > 8192)
            || self.unix_mode.is_some_and(|mode| mode & !0o777 != 0)
        {
            return Err("invalid managed file metadata");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_preparation_binds_parent_existing_object_and_durable_result() {
        for existing in [None,Some("w:existing".to_owned())] {
            let request=FileManagerRequest {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:RequestId::new(),execution:Default::default(),expected_execution:None,operation:FileManagerOperation::PrepareDirectory {path:"/parent/child".into(),expected_parent:"w:parent".into(),existing_object:existing.clone()}};
            request.validate().unwrap();assert_eq!(request.filesystem_request().operation.schema_version(),11);assert!(request.filesystem_request().operation.mutates());
            let reply=FileManagerReply {schema_version:FILE_MANAGER_SCHEMA_VERSION,request_id:request.request_id,execution_context:None,result:FileManagerResult::DirectoryPrepared {path:"/parent/child".into(),identity:existing.clone().unwrap_or("w:new".into()),created:existing.is_none()}};
            reply.validate_for(&request).unwrap();
            for change in 0..3 {let mut bad=reply.clone();let FileManagerResult::DirectoryPrepared {path,identity,created}=&mut bad.result else {unreachable!()};match change {0=>*path="/other".into(),1=>*created=!*created,_=>*identity=if existing.is_some(){"w:other".into()}else{"../invalid".into()}};assert!(bad.validate_for(&request).is_err());}
            let mut bad=request.clone();let FileManagerOperation::PrepareDirectory {expected_parent,..}=&mut bad.operation else {unreachable!()};*expected_parent="".into();assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn path_identity_validation_binds_requests_and_does_not_confuse_different_hosts() {
        let source=FilePathIdentity {filesystem_scope:"a".repeat(64),object:Some("u:1:2:3".into()),kind:Some(ManagedFileKind::Directory),ancestors:vec!["u:1:1:1".into()]};
        assert!(source.validate_copy_destination(&source).is_err());
        let mut different_host=source.clone();different_host.filesystem_scope="b".repeat(64);
        source.validate_copy_destination(&different_host).unwrap();
        let descendant=FilePathIdentity {filesystem_scope:source.filesystem_scope.clone(),object:None,kind:None,ancestors:vec![source.object.clone().unwrap()]};
        assert!(source.validate_copy_destination(&descendant).is_err());
        let request=FileManagerRequest {schema_version:1,request_id:RequestId::new(),execution:ExecutionSelection::Service {},expected_execution:None,operation:FileManagerOperation::InspectPathIdentity {path:"/source".into()}};
        assert_eq!(request.filesystem_request().schema_version(),10);
        let mut reply=FileManagerReply {schema_version:1,request_id:request.request_id,execution_context:None,result:FileManagerResult::PathIdentity {path:"/source".into(),identity:source.clone()}};
        reply.validate_for(&request).unwrap();
        if let FileManagerResult::PathIdentity {path,..}=&mut reply.result {*path="/other".into();}assert!(reply.validate_for(&request).is_err());
        for invalid in 0..4 {let mut malformed=source.clone();match invalid {0=>malformed.filesystem_scope="unknown".into(),1=>malformed.kind=None,2=>malformed.ancestors=vec!["u:1".into();257],_=>malformed.object=Some("".into())};assert!(malformed.validate().is_err());}
    }
    #[test]
    fn directory_metadata_replies_bind_object_path_and_requested_attributes() {
        let metadata=crate::FileTransferMetadata {modified_at_unix_ms:Some(1),unix_mode:Some(0o750)};
        let request=FileManagerRequest {schema_version:1,request_id:RequestId::new(),execution:ExecutionSelection::Service {},expected_execution:None,
            operation:FileManagerOperation::SetDirectoryMetadata {path:"/target".into(),expected_object:"u:1:2:3".into(),metadata:metadata.clone()}};
        assert_eq!(request.filesystem_request().schema_version(),9);assert!(request.filesystem_request().operation.mutates());
        let reply=FileManagerReply {schema_version:1,request_id:request.request_id,execution_context:None,
            result:FileManagerResult::DirectoryMetadata {path:"/target".into(),identity:"u:1:2:3".into(),metadata:crate::FileTransferMetadataResult {requested:metadata.clone(),actual:metadata,warnings:Vec::new()}}};
        reply.validate_for(&request).unwrap();
        for change in 0..4 {
            let mut bad=reply.clone();let FileManagerResult::DirectoryMetadata {path,identity,metadata}=&mut bad.result else {unreachable!()};
            match change {0=>*path="/different".into(),1=>identity.push('4'),2=>metadata.requested.unix_mode=Some(0o700),_=>metadata.actual.modified_at_unix_ms=None};
            assert!(bad.validate_for(&request).is_err());
        }
        let mut bad=request.clone();let FileManagerOperation::SetDirectoryMetadata {expected_object,..}=&mut bad.operation else {unreachable!()};
        *expected_object="x".repeat(193);assert!(bad.validate().is_err());
    }
    #[test]
    fn native_wire_paths_are_compact_and_reject_oversized_or_odd_encodings() {
        let path = NativeFilePath::WindowsWide(vec![0xd800; MAX_NATIVE_PATH_UNITS]);
        let encoded = serde_json::to_string(&path).unwrap();
        assert!(encoded.len() < 24 * 1024);
        assert_eq!(
            serde_json::from_str::<NativeFilePath>(&encoded).unwrap(),
            path
        );
        for json in [
            serde_json::json!({"encoding":"windows_wide","value":"AA=="}),
            serde_json::json!({"encoding":"unix_bytes","value":"A==="}),
            serde_json::json!({"encoding":"unix_bytes","value":"A".repeat(MAX_NATIVE_PATH_UNITS * 2)}),
        ] {
            assert!(serde_json::from_value::<NativeFilePath>(json).is_err());
        }
    }
    #[test]
    fn replies_cannot_change_action_snapshot_or_repeat_a_cursor() {
        let snapshot_id = RequestId::new();
        let request = FileManagerRequest {
            expected_execution:None,schema_version: FILE_MANAGER_SCHEMA_VERSION,
            request_id: RequestId::new(),
            execution: ExecutionSelection::Service {},
            operation: FileManagerOperation::ReadDirectory {
                snapshot_id,
                after: Some("first".into()),
                limit: 2,
                filter: Default::default(),
            },
        };
        let mut reply = FileManagerReply {
            schema_version: 1,
            request_id: request.request_id,
            execution_context: None,
            result: FileManagerResult::Directory {
                page: ManagedDirectoryPage {
                    snapshot_id,
                    path: NativeFilePath::Utf8("/directory".into()),
                    entries: vec![ManagedFileEntry {
                        id: RequestId::new(),
                        name: "file".into(),
                        path: NativeFilePath::Utf8("/directory/file".into()),
                        kind: ManagedFileKind::File,
                        size: Some(FileByteCount(0)),
                        modified_at_unix_ms: None,
                        hidden: false,
                        readonly: false,
                        unix_mode: Some(0o644),
                        error: None,
                    }],
                    next_after: Some("second".into()),
                    total_entries: FileByteCount(3),
                    observed_at_unix_ms: 1,
                    complete: true,
                },
            },
        };
        reply.validate_for(&request).unwrap();
        if let FileManagerResult::Directory { page } = &mut reply.result {
            page.next_after = Some("first".into());
        }
        assert!(reply.validate_for(&request).is_err());
        if let FileManagerResult::Directory { page } = &mut reply.result {
            page.next_after = None;
            page.snapshot_id = RequestId::new();
        }
        assert!(reply.validate_for(&request).is_err());
        reply.result = FileManagerResult::Closed { snapshot_id };
        assert!(reply.validate_for(&request).is_err());
        let envelope = request.filesystem_request();
        assert_eq!(envelope.request_id, request.request_id);
        assert_eq!(envelope.schema_version(), 7);
        envelope.validate().unwrap();
    }
    #[test]
    fn file_manager_requests_are_versioned_bounded_and_unambiguous() {
        let mut request = FileManagerRequest {
            expected_execution:None,schema_version: FILE_MANAGER_SCHEMA_VERSION,
            request_id: RequestId::new(),
            execution: ExecutionSelection::Service {},
            operation: FileManagerOperation::ReadDirectory {
                snapshot_id: RequestId::new(),
                after: None,
                limit: 256,
                filter: FileManagerFilter::default(),
            },
        };
        request.validate().unwrap();
        request.schema_version += 1;
        assert!(request.validate().is_err());
        request.schema_version -= 1;
        if let FileManagerOperation::ReadDirectory { limit, .. } = &mut request.operation {
            *limit = 257;
        }
        assert!(request.validate().is_err());
        for name in ["", ".", "..", "../escape", "nul\0"] {
            request.operation = FileManagerOperation::CreateDirectory {
                parent: NativeFilePath::Utf8("/target".into()),
                name: name.into(),
            };
            assert!(request.validate().is_err(), "{name}");
        }
        assert!(NativeFilePath::Utf8("nested\\name".into()).validate_component().is_ok(),"backslash is a POSIX filename character");
        assert!(NativeFilePath::WindowsWide("nested\\name".encode_utf16().collect()).validate_component().is_err());
        request.operation = FileManagerOperation::Rename {
            path: NativeFilePath::Utf8("/target/file".into()),
            name: "新文件.txt".into(),
        };
        request.validate().unwrap();
    }
    #[test]
    fn file_counts_are_exact_and_reject_lossy_or_noncanonical_inputs() {
        for n in [0, 1, 1 << 32, (1 << 53) + 1, u64::MAX] {
            let json = serde_json::to_string(&FileByteCount(n)).unwrap();
            assert_eq!(json, format!("\"{n}\""));
            assert_eq!(serde_json::from_str::<FileByteCount>(&json).unwrap().0, n);
        }
        for json in [
            "123",
            "\"-1\"",
            "\"01\"",
            "\"1.0\"",
            "\"18446744073709551616\"",
            "\"\"",
        ] {
            assert!(
                serde_json::from_str::<FileByteCount>(json).is_err(),
                "{json}"
            );
        }
    }
    #[test]
    fn native_names_roundtrip_without_lossy_conversion() {
        for path in [
            NativeFilePath::UnixBytes(vec![b'/', 0xff]),
            NativeFilePath::WindowsWide(vec![67, 58, 92, 0xd800]),
        ] {
            path.validate().unwrap();
            assert_eq!(
                serde_json::from_slice::<NativeFilePath>(&serde_json::to_vec(&path).unwrap())
                    .unwrap(),
                path
            );
        }
        assert!(
            NativeFilePath::Utf8("/bad\0path".into())
                .validate()
                .is_err()
        );
        assert!(
            NativeFilePath::UnixBytes(vec![1; MAX_NATIVE_PATH_UNITS + 1])
                .validate()
                .is_err()
        );
    }
}
