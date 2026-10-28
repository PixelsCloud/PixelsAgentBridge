//! Directory finalization is bound to one native object, never just its name.
use crate::{FileManagerError, decode_path, mutation::open_directory};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use cap_std::fs::OpenOptionsExt;
use pab_protocol::{FileTransferMetadata, FileTransferMetadataResult, NativeFilePath};
use std::fs::File;

/// Keep this value alive until the attribute outcome has been recorded. Opening
/// for modification never creates a directory, follows a link or changes ACLs.
pub struct DirectoryObject {
    file: File,
    identity: String,
    // Keep the selected parent anchored until the created/opened result is
    // recorded, not merely until the native child handle has been acquired.
    _parent:File,
    #[cfg(unix)]
    creation_receipt:Option<crate::directory_staging::Guard>,
}
impl DirectoryObject {
    #[cfg(unix)]
    pub fn prepare_journaled(database:&std::path::Path,request:pab_protocol::RequestId,path:&NativeFilePath,expected_parent:&str,existing_object:Option<&str>)->Result<Self,FileManagerError> {
        if existing_object.is_some() {return Self::prepare(path,expected_parent,existing_object);}
        let path=decode_path(path)?;
        let parent_path=path.parent().ok_or(FileManagerError::Invalid("cannot create a filesystem root"))?;
        let name=path.file_name().ok_or(FileManagerError::Invalid("cannot create a filesystem root"))?;
        crate::decode_component(&crate::encode_path(std::path::Path::new(name)))?;
        let parent=open_directory(parent_path)?.into_std_file();Self::verify_parent(&parent,Some(expected_parent))?;
        let (file,receipt)=crate::directory_staging::create_tracked(database,request,&parent,parent_path,name,expected_parent)?;
        let mut object=Self::from_opened(file,parent).map_err(|error|FileManagerError::OutcomeUnconfirmed(error.to_string()))?;
        object.creation_receipt=Some(receipt);Ok(object)
    }
    /// Bind an existing directory or return a newly created directory without
    /// dropping/reopening its handle between the effect and its recorded ID.
    pub fn prepare(path:&NativeFilePath,expected_parent:&str,existing_object:Option<&str>)->Result<Self,FileManagerError> {
        if let Some(expected)=existing_object {
            pab_protocol::FileTransferParentConstraint {identity:expected.to_owned()}.validate().map_err(FileManagerError::Invalid)?;
            let object=Self::open_in_parent(path,false,Some(expected_parent))?;
            if object.identity()!=expected {return Err(FileManagerError::Conflict);}
            return Ok(object);
        }
        {
            let path=decode_path(path)?;
            let parent=path.parent().ok_or(FileManagerError::Invalid("cannot create a filesystem root"))?;
            let name=path.file_name().ok_or(FileManagerError::Invalid("cannot create a filesystem root"))?;
            Self::create_in_parent(&crate::encode_path(parent),&crate::encode_path(std::path::Path::new(name)),expected_parent).map(|(_,object)|object)
        }
    }
    pub fn open(path: &NativeFilePath, writable: bool) -> Result<Self, FileManagerError> {
        Self::open_in_parent(path,writable,None)
    }
    pub fn open_in_parent(path:&NativeFilePath,writable:bool,expected_parent:Option<&str>)->Result<Self,FileManagerError> {
        let path = decode_path(path)?;
        let parent = path.parent().ok_or(FileManagerError::Invalid("cannot finalize a filesystem root"))?;
        let name = path.file_name().ok_or(FileManagerError::Invalid("cannot finalize a filesystem root"))?;
        crate::decode_component(&crate::encode_path(std::path::Path::new(name)))?;
        let directory = open_directory(parent)?;
        let parent=directory.try_clone()?.into_std_file();
        Self::verify_parent(&parent,expected_parent)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.custom_flags(pab_os_control::filesystem::DIRECTORY_OPEN_FLAGS);
        #[cfg(windows)]
        {
            // Metadata-only access does not enforce Windows delete sharing.
            // GENERIC_READ makes the no-DELETE share effective; add only
            // WRITE_ATTRIBUTES for finalization. Child writes still work.
            options.access_mode(0x8000_0000 | if writable { 0x100 } else { 0 })
                .share_mode(1 | 2).custom_flags(0x0220_0000);
        }
        #[cfg(not(windows))]
        let _ = writable;
        // Open the actual final component relative to its no-follow parent.
        let file = directory.open_with(name, &options)?.into_std();
        Self::from_opened(file,parent)
    }
    fn from_opened(file:File,parent:File)->Result<Self,FileManagerError> {
        let metadata = file.metadata()?;
        #[cfg(windows)]
        let reparse = { use std::os::windows::fs::MetadataExt; metadata.file_attributes() & 0x400 != 0 };
        #[cfg(not(windows))]
        let reparse = false;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || reparse {
            return Err(FileManagerError::Invalid("expected an ordinary directory"));
        }
        let identity = pab_os_control::filesystem::file_object_identity(&file)?;
        Ok(Self { file, identity, _parent:parent,#[cfg(unix)] creation_receipt:None })
    }
    fn verify_parent(parent:&File,expected:Option<&str>)->Result<(),FileManagerError> {
        if let Some(expected)=expected {
            pab_protocol::FileTransferParentConstraint {identity:expected.to_owned()}.validate().map_err(FileManagerError::Invalid)?;
            if pab_os_control::filesystem::file_object_identity(parent)?!=expected {return Err(FileManagerError::Invalid("destination parent changed since it was selected"));}
        }
        Ok(())
    }
    /// Windows creates and returns the object directly; Unix publishes an
    /// opened object from a private staging parent without replacing a target.
    pub fn create_in_parent(parent:&NativeFilePath,name:&NativeFilePath,expected_parent:&str)->Result<(NativeFilePath,Self),FileManagerError> {
        let name=crate::decode_component(name)?;let parent_path=decode_path(parent)?;
        let target=crate::encode_path(&parent_path.join(&name));target.validate().map_err(FileManagerError::Invalid)?;
        let parent=open_directory(&parent_path)?.into_std_file();Self::verify_parent(&parent,Some(expected_parent))?;
        let file=pab_os_control::filesystem::create_directory_at(&parent,&name)?;
        Ok((target,Self::from_opened(file,parent).map_err(|error|FileManagerError::OutcomeUnconfirmed(error.to_string()))?))
    }
    pub fn identity(&self) -> &str { &self.identity }
    /// Release the creation journal only after the caller has durably saved
    /// this object's original outcome. An omitted/failed acknowledgement leaves
    /// that fact recoverable without opening the current public pathname.
    pub fn acknowledge_recorded(&mut self)->Result<(),FileManagerError> {
        #[cfg(unix)]
        if let Some(receipt)=self.creation_receipt.take(){receipt.acknowledge()?;}
        Ok(())
    }
    pub fn metadata(&self) -> Result<FileTransferMetadata, FileManagerError> {
        Ok(crate::file_transfer_metadata(&self.file.metadata()?))
    }
    pub fn apply(&self, expected: &str, requested: &FileTransferMetadata) -> Result<FileTransferMetadataResult, FileManagerError> {
        requested.validate().map_err(FileManagerError::Invalid)?;
        if expected != self.identity { return Err(FileManagerError::Conflict); }
        Ok(crate::apply_file_metadata(&self.file, requested)?)
    }
    /// Recovery observes the original object without reapplying attributes.
    pub fn observe(&self, expected: &str, requested: &FileTransferMetadata) -> Result<FileTransferMetadataResult, FileManagerError> {
        if expected != self.identity { return Err(FileManagerError::Conflict); }
        Ok(crate::observe_file_metadata(&self.file.metadata()?, requested)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn directory_open_rejects_a_fifo_without_waiting_for_a_writer() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("named-pipe");
        assert!(std::process::Command::new("mkfifo").arg(&path).status().unwrap().success());
        let (send,receive)=std::sync::mpsc::channel();let native=crate::encode_path(&path);
        let worker=std::thread::spawn(move||{let result=DirectoryObject::open(&native,false);let _=send.send(result.is_err());});
        assert!(receive.recv_timeout(std::time::Duration::from_secs(2)).expect("directory open waited for a FIFO writer"));worker.join().unwrap();
        use std::os::unix::fs::FileTypeExt;assert!(std::fs::symlink_metadata(path).unwrap().file_type().is_fifo());
    }
    #[cfg(unix)]
    #[test]
    fn unix_create_rejects_replaced_parent_and_returns_the_published_object() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();
        let native=crate::encode_path(&parent);let expected=DirectoryObject::open(&native,false).unwrap().identity().to_owned();let target=crate::encode_path(&parent.join("created"));
        std::fs::rename(&parent,root.join("original")).unwrap();std::fs::create_dir(&parent).unwrap();
        assert!(DirectoryObject::prepare(&target,&expected,None).is_err());assert_eq!(std::fs::read_dir(&parent).unwrap().count(),0);
        std::fs::rename(&parent,root.join("replacement")).unwrap();std::fs::rename(root.join("original"),&parent).unwrap();
        let created=DirectoryObject::prepare(&target,&expected,None).unwrap();assert_eq!(created.identity(),DirectoryObject::open(&target,false).unwrap().identity());assert!(DirectoryObject::prepare(&target,&expected,None).is_err());
        std::fs::rename(parent.join("created"),parent.join("moved")).unwrap();std::fs::create_dir(parent.join("created")).unwrap();
        assert!(DirectoryObject::prepare(&target,&expected,Some(created.identity())).is_err());assert_eq!(std::fs::read_dir(parent.join("created")).unwrap().count(),0);
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(),2);assert_eq!(std::fs::read_dir(root.join("replacement")).unwrap().count(),0);
    }
    #[cfg(windows)]
    #[test]
    fn directory_create_binds_the_parent_before_effects_and_retains_both_objects() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();
        let native=crate::encode_path(&parent);let expected=DirectoryObject::open(&native,false).unwrap().identity().to_owned();
        std::fs::rename(&parent,root.join("original")).unwrap();std::fs::create_dir(&parent).unwrap();
        let name=crate::encode_path(std::path::Path::new("created"));
        assert!(DirectoryObject::create_in_parent(&native,&name,&expected).is_err());assert_eq!(std::fs::read_dir(&parent).unwrap().count(),0);
        std::fs::rename(&parent,root.join("replacement")).unwrap();std::fs::rename(root.join("original"),&parent).unwrap();
        let (path,object)=DirectoryObject::create_in_parent(&native,&name,&expected).unwrap();let identity=object.identity().to_owned();
        assert_eq!(DirectoryObject::open(&path,false).unwrap().identity(),identity);
        assert!(std::fs::rename(parent.join("created"),parent.join("moved")).is_err());assert!(std::fs::rename(&parent,root.join("moved-parent")).is_err());
        std::fs::write(parent.join("created/child"),b"data").unwrap();assert!(DirectoryObject::create_in_parent(&native,&name,&expected).is_err());
        assert_eq!(std::fs::read(parent.join("created/child")).unwrap(),b"data");
        drop(object);std::fs::rename(parent.join("created"),parent.join("moved")).unwrap();std::fs::rename(&parent,root.join("moved-parent")).unwrap();
        assert_eq!(std::fs::read_dir(root.join("replacement")).unwrap().count(),0);
    }
    #[test]
    fn directory_open_with_parent_identity_does_not_adopt_a_same_named_replacement() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir_all(parent.join("child")).unwrap();
        let native=crate::encode_path(&parent);let expected=DirectoryObject::open(&native,false).unwrap().identity().to_owned();let child=crate::encode_path(&parent.join("child"));
        let original=DirectoryObject::open_in_parent(&child,false,Some(&expected)).unwrap().identity().to_owned();
        std::fs::rename(&parent,root.join("original")).unwrap();std::fs::create_dir_all(parent.join("child")).unwrap();
        assert!(DirectoryObject::open_in_parent(&child,false,Some(&expected)).is_err());
        assert_ne!(DirectoryObject::open(&child,false).unwrap().identity(),original);
        assert_eq!(std::fs::read_dir(parent.join("child")).unwrap().count(),0);
    }
    #[test]
    fn directory_attributes_bind_the_object_and_reject_replacement_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("target");
        std::fs::create_dir(&path).unwrap();
        let requested = FileTransferMetadata { modified_at_unix_ms: Some(1_700_000_000_123), unix_mode: None };
        let object = DirectoryObject::open(&crate::encode_path(&path), true).unwrap();
        let identity = object.identity().to_owned();
        let result = object.apply(&identity, &requested).unwrap();
        assert!(result.warnings.is_empty(), "{result:?}");
        assert_eq!(object.metadata().unwrap().modified_at_unix_ms, requested.modified_at_unix_ms);
        std::fs::write(path.join("child"), b"content").unwrap();
        assert_eq!(object.identity(), identity);
        drop(object);
        std::fs::rename(&path, root.join("old-target")).unwrap();
        std::fs::create_dir(&path).unwrap();
        let replacement = DirectoryObject::open(&crate::encode_path(&path), true).unwrap();
        let before = replacement.metadata().unwrap();
        assert_ne!(replacement.identity(), identity);
        assert!(matches!(replacement.apply(&identity, &requested), Err(FileManagerError::Conflict)));
        assert!(replacement.observe(&identity, &requested).is_err());
        assert_eq!(replacement.metadata().unwrap(), before);
        let observed = replacement.observe(replacement.identity(), &requested).unwrap();
        assert!(!observed.warnings.is_empty());
        assert_eq!(replacement.metadata().unwrap(), before, "observation must never repair attributes");
    }
    #[test]
    fn invalid_privileges_and_regular_files_cannot_be_finalized_as_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = crate::encode_path(&root);
        let object = DirectoryObject::open(&path, true).unwrap();
        let before = object.metadata().unwrap();
        assert!(object.apply(object.identity(), &FileTransferMetadata { modified_at_unix_ms: Some(1), unix_mode: Some(0o4755) }).is_err());
        assert_eq!(object.metadata().unwrap(), before);
        let file = root.join("file"); std::fs::write(&file, b"untouched").unwrap();
        assert!(DirectoryObject::open(&crate::encode_path(&file), true).is_err());
        assert_eq!(std::fs::read(file).unwrap(), b"untouched");
    }
    #[cfg(unix)]
    #[test]
    fn links_are_rejected_and_retained_handle_survives_readonly_finalization() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap(); let root = temp.path().canonicalize().unwrap();
        let target = root.join("target"); std::fs::create_dir(&target).unwrap();
        symlink(&target, root.join("link")).unwrap();
        assert!(DirectoryObject::open(&crate::encode_path(&root.join("link")), true).is_err());
        std::fs::create_dir(target.join("child")).unwrap();
        assert!(DirectoryObject::open(&crate::encode_path(&root.join("link/child")), true).is_err());
        let object = DirectoryObject::open(&crate::encode_path(&target), true).unwrap();
        let result = object.apply(object.identity(), &FileTransferMetadata {modified_at_unix_ms: Some(1_700_000_000_123), unix_mode: Some(0)}).unwrap();
        let actual = object.metadata().unwrap();
        // Restore only our fixture through its retained handle before asserts.
        object.file.set_permissions(std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.warnings.is_empty(), "{result:?}"); assert_eq!(actual.unix_mode, Some(0));
    }
}
