//! A receiving file remains relative to one retained native directory handle.
//! A parent becoming a link cannot redirect later writes or publication.
use crate::{FileManagerError,mutation::open_directory,publish::PublicationReceipt};
use cap_fs_ext::{FollowSymlinks,OpenOptionsFollowExt};
use cap_std::fs::{Dir,OpenOptions,OpenOptionsExt};
use std::{ffi::OsString,fs::File,io,path::{Path,PathBuf}};

pub struct StagingTarget {directory:Dir,parent:PathBuf,source:OsString,destination:OsString,
    #[cfg(unix)]
    private_source:Option<Dir>,
    #[cfg(any(windows,target_os="linux",target_os="android"))]
    reservation:crate::target_reservation::TargetReservation,
}
impl StagingTarget {
    pub fn new(staging:&Path,destination:&Path)->io::Result<Self> {
        Self::new_in_parent(staging,destination,None)
    }
    /// Compare a frozen parent identity against the retained native directory
    /// before opening/adopting a staging file or reserving a receiving target.
    pub fn new_in_parent(staging:&Path,destination:&Path,expected_parent:Option<&str>)->io::Result<Self> {
        if !staging.is_absolute() || !destination.is_absolute() || staging==destination {return Err(invalid("invalid receiving paths"));}
        let parent=destination.parent().ok_or_else(||invalid("missing destination parent"))?;
        if staging.parent()!=Some(parent){return Err(invalid("staging must be beside destination"));}
        let source=component(staging)?;let destination=component(destination)?;
        let directory=open_directory(parent).map_err(io::Error::other)?;
        verify_parent(&directory,expected_parent)?;
        #[cfg(any(windows,target_os="linux",target_os="android"))]
        let reservation=crate::target_reservation::TargetReservation::acquire(&directory,&destination)?;
        Ok(Self {directory,parent:parent.to_owned(),source,destination,
            #[cfg(unix)]
            private_source:None,
            #[cfg(any(windows,target_os="linux",target_os="android"))]
            reservation,
        })
    }
    #[cfg(unix)]
    pub(crate) fn destination_directory(&self)->io::Result<Dir> {self.directory.try_clone()}
    #[cfg(unix)]
    pub(crate) fn with_private_source(mut self,directory:Dir)->io::Result<Self> {
        pab_os_control::filesystem::verify_private_directory(&directory.try_clone()?.into_std_file())?;
        self.source=OsString::from("payload");self.private_source=Some(directory);Ok(self)
    }
    fn source_directory(&self)->&Dir {
        #[cfg(unix)]
        if let Some(directory)=&self.private_source {return directory;}
        &self.directory
    }
    pub fn check_destination(&self,overwrite:bool)->io::Result<()> {
        match self.directory.symlink_metadata(&self.destination) {
            Ok(_) if !overwrite=>Err(io::Error::new(io::ErrorKind::AlreadyExists,"destination already exists")),
            Ok(meta) if !meta.is_file() || cap_is_link(&meta)=>Err(invalid("destination is not an ordinary file")),
            Ok(_)=>Ok(()),
            Err(error) if error.kind()==io::ErrorKind::NotFound=>Ok(()),
            Err(error)=>Err(error),
        }
    }
    pub fn open(&self,create:bool)->io::Result<File> {
        let mut options=OpenOptions::new();options.read(true).write(true).create(create).truncate(false).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.mode(0o600);
        #[cfg(windows)]
        options.access_mode(0xc001_0000).share_mode(1).custom_flags(0x0020_0000);
        let file=self.source_directory().open_with(&self.source,&options)?.into_std();
        file.try_lock().map_err(io::Error::other)?;
        if !file.metadata()?.is_file() || std_is_link(&file.metadata()?){return Err(invalid("staging is not an ordinary file"));}
        if pab_os_control::filesystem::file_link_count(&file)?!=1{return Err(invalid("staging must not be a hard link"));}
        // Keep the new object's reservation through publication and attribute
        // acknowledgement. After rename, short-name/hard-link aliases identify
        // this same object even if their spelling differs from the target name.
        #[cfg(any(windows,target_os="linux",target_os="android"))]
        self.reservation.reserve_file(&file)?;
        Ok(file)
    }
    pub fn adopt(&self,previous:&Path)->io::Result<()> {
        #[cfg(unix)]
        if self.private_source.is_some(){return Err(invalid("private staging requires its original ownership receipt"));}
        if previous.parent()!=Some(self.parent.as_path()){return Err(invalid("resume staging must be beside destination"));}
        let previous=component(previous)?;
        if previous==self.source || previous==self.destination{return Err(invalid("invalid resume staging name"));}
        let meta=self.directory.symlink_metadata(&previous)?;
        if !meta.is_file() || cap_is_link(&meta){return Err(invalid("resume staging is not an ordinary file"));}
        #[cfg(unix)]
        {
            let mut options=OpenOptions::new();options.read(true).write(true).follow(FollowSymlinks::No);
            let file=self.directory.open_with(&previous,&options)?.into_std();file.try_lock().map_err(io::Error::other)?;
            if !file.metadata()?.is_file() || pab_os_control::filesystem::file_link_count(&file)?!=1{return Err(invalid("invalid resume staging object"));}
            pab_os_control::filesystem::rename_relative_no_replace(&self.directory.try_clone()?.into_std_file(),&previous,&self.source)
        }
        #[cfg(windows)]
        {
            let mut options=OpenOptions::new();options.access_mode(0x0011_0000).share_mode(1|2).custom_flags(0x0020_0000).follow(FollowSymlinks::No);
            let file=self.directory.open_with(previous,&options)?.into_std();
            if std_is_link(&file.metadata()?){return Err(invalid("resume staging is a link"));}
            pab_os_control::filesystem::rename_open(&file,&self.directory.try_clone()?.into_std_file(),&self.source,false)
        }
    }
    /// Discard only the still-open receiving object. Never close the writer
    /// and then delete a pathname that might now identify somebody else's file.
    pub fn remove_open(&self,file:&File)->io::Result<()> {
        let metadata=file.metadata()?;
        if !metadata.is_file() || std_is_link(&metadata) || pab_os_control::filesystem::file_link_count(file)?!=1 {
            return Err(invalid("invalid staging object for cleanup"));
        }
        let current=self.source_directory().symlink_metadata(&self.source)?;
        if !current.is_file() || cap_is_link(&current){return Err(invalid("staging name changed before cleanup"));}
        #[cfg(windows)]
        {
            let mut options=OpenOptions::new();options.read(true).share_mode(1|2|4).custom_flags(0x0020_0000).follow(FollowSymlinks::No);
            let named=self.directory.open_with(&self.source,&options)?.into_std();
            if std_is_link(&named.metadata()?) || pab_os_control::filesystem::file_object_identity(&named)?!=pab_os_control::filesystem::file_object_identity(file)? {
                return Err(invalid("staging identity changed before cleanup"));
            }
            drop(named);
            pab_os_control::filesystem::delete_open_file(file)
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            use cap_std::fs::MetadataExt as _;
            if self.private_source.is_some(){pab_os_control::filesystem::verify_private_directory(&self.source_directory().try_clone()?.into_std_file())?;}
            if current.dev()!=metadata.dev() || current.ino()!=metadata.ino(){return Err(invalid("staging identity changed before cleanup"));}
            self.source_directory().remove_file(&self.source)
        }
    }
    pub fn publish(self,file:File,overwrite:bool)->io::Result<PublicationReceipt> {
        self.publish_open(&file,overwrite)
    }
    /// Keep the verified, locked handle alive for final permission changes.
    /// Never reopen the published pathname to perform that finalization.
    pub fn publish_open(&self,file:&File,overwrite:bool)->io::Result<PublicationReceipt> {
        #[cfg(any(windows,target_os="linux",target_os="android"))]
        self.reservation.reserve_destination(&self.directory,&self.destination)?;
        self.check_destination(overwrite)?;
        let metadata=file.metadata()?;
        if !metadata.is_file() || std_is_link(&metadata){return Err(invalid("staging is not an ordinary file"));}
        if pab_os_control::filesystem::file_link_count(file)?!=1{return Err(invalid("staging link count changed"));}
        file.sync_all()?;
        #[cfg(windows)]
        pab_os_control::filesystem::rename_open(file,&self.directory.try_clone()?.into_std_file(),&self.destination,overwrite)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            use cap_std::fs::MetadataExt as _;
            let current=self.source_directory().symlink_metadata(&self.source)?;
            if cap_is_link(&current) || current.dev()!=metadata.dev() || current.ino()!=metadata.ino(){return Err(invalid("staging identity changed before publication"));}
            if overwrite {self.source_directory().rename(&self.source,&self.directory,&self.destination)?;}
            else {pab_os_control::filesystem::rename_between_no_replace(&self.source_directory().try_clone()?.into_std_file(),&self.source,&self.directory.try_clone()?.into_std_file(),&self.destination)?;}
        }
        #[cfg(unix)]
        let directory_synced=self.directory.try_clone().and_then(|directory|directory.into_std_file().sync_all()).is_ok();
        #[cfg(not(unix))]
        let directory_synced=false;
        Ok(PublicationReceipt {published:true,staging_retained:false,directory_synced})
    }
}
/// Open the selected source without following an ancestor or final link.
pub fn open_source(path:&Path)->io::Result<File> {
    open_source_in_parent(path,None)
}
/// Recovery must not verify identical bytes in a replacement parent and then
/// attribute those bytes to an original write in a different directory.
pub fn open_source_in_parent(path:&Path,expected_parent:Option<&str>)->io::Result<File> {
    let name=component(path)?;let parent=path.parent().ok_or_else(||invalid("missing source parent"))?;
    let directory=open_directory(parent).map_err(io::Error::other)?;
    verify_parent(&directory,expected_parent)?;
    let mut options=OpenOptions::new();options.read(true).follow(FollowSymlinks::No);
    #[cfg(windows)]
    options.share_mode(1).custom_flags(0x0020_0000);
    let file=directory.open_with(name,&options)?.into_std();
    let metadata=file.metadata()?;
    if !metadata.is_file() || std_is_link(&metadata){return Err(invalid("source is not an ordinary file"));}
    Ok(file)
}
fn verify_parent(directory:&Dir,expected:Option<&str>)->io::Result<()> {
    let Some(expected)=expected else {return Ok(());};
    if expected.is_empty() || expected.len()>192 || !expected.bytes().all(|value|value.is_ascii_alphanumeric() || value==b':') {return Err(invalid("invalid expected parent identity"));}
    let actual=pab_os_control::filesystem::file_object_identity(&directory.try_clone()?.into_std_file())?;
    if actual!=expected {return Err(io::Error::new(io::ErrorKind::InvalidData,"destination parent changed since it was selected"));}
    Ok(())
}
fn component(path:&Path)->io::Result<OsString> {
    if !path.is_absolute() || path.components().any(|component|matches!(component,std::path::Component::ParentDir|std::path::Component::CurDir)){return Err(invalid("expected an absolute file path"));}
    let name=path.file_name().ok_or_else(||invalid("missing filename"))?;
    crate::decode_component(&crate::encode_path(Path::new(name))).map_err(|error:FileManagerError|io::Error::other(error))?;
    Ok(name.to_owned())
}
fn invalid(message:&'static str)->io::Error {io::Error::new(io::ErrorKind::InvalidInput,message)}
fn cap_is_link(metadata:&cap_std::fs::Metadata)->bool {
    #[cfg(windows)]
    {use cap_std::fs::MetadataExt;metadata.file_type().is_symlink() || metadata.file_attributes()&0x400!=0}
    #[cfg(not(windows))]
    {metadata.file_type().is_symlink()}
}
fn std_is_link(metadata:&std::fs::Metadata)->bool {
    #[cfg(windows)]
    {use std::os::windows::fs::MetadataExt;metadata.file_type().is_symlink() || metadata.file_attributes()&0x400!=0}
    #[cfg(not(windows))]
    {metadata.file_type().is_symlink()}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn link_directory(target:&Path,path:&Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target,path).unwrap();
        #[cfg(windows)]
        {
            let result=std::process::Command::new("cmd.exe").args(["/d","/c","mklink","/J"]).arg(path).arg(target).output().unwrap();
            assert!(result.status.success(),"junction fixture creation failed");
        }
    }
    #[test]
    fn a_live_staging_writer_prevents_reopening_or_adopting_its_file() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let destination=root.join("destination");
        let original=StagingTarget::new(&root.join(".first"),&destination).unwrap();let file=original.open(true).unwrap();
        assert!(original.open(false).is_err());
        #[cfg(windows)]
        assert!(StagingTarget::new(&root.join(".second"),&destination).is_err());
        drop(original);
        let resumed=StagingTarget::new(&root.join(".second"),&destination).unwrap();
        assert!(resumed.adopt(&root.join(".first")).is_err());assert!(root.join(".first").is_file());assert!(!root.join(".second").exists());
        drop(file);resumed.adopt(&root.join(".first")).unwrap();assert!(resumed.open(false).is_ok());
    }
    #[test]
    fn cleanup_retains_the_original_handle_and_never_touches_the_destination() {
        for size in [0,65537] {
            let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let staging=root.join(".owned.part");let destination=root.join("destination");
            std::fs::write(&destination,b"keep existing destination").unwrap();
            let target=StagingTarget::new(&staging,&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(&vec![17;size]).unwrap();file.sync_all().unwrap();
            target.remove_open(&file).unwrap();drop(file);
            assert!(!staging.exists());assert_eq!(std::fs::read(&destination).unwrap(),b"keep existing destination");
            assert!(target.open(false).is_err(),"cleanup must not recreate an absent staging file");
        }
    }
    #[test]
    fn cleanup_rejects_a_foreign_file_handle_and_an_already_published_file() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let staging=root.join(".owned.part");let destination=root.join("destination");
        let target=StagingTarget::new(&staging,&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"verified payload").unwrap();file.sync_all().unwrap();
        let other=root.join("other");std::fs::write(&other,b"keep unrelated").unwrap();let foreign=File::open(&other).unwrap();
        assert!(target.remove_open(&foreign).is_err());assert_eq!(std::fs::read(&other).unwrap(),b"keep unrelated");assert!(staging.exists());drop(foreign);
        target.publish_open(&file,false).unwrap();assert!(target.remove_open(&file).is_err());drop(file);
        assert_eq!(std::fs::read(&destination).unwrap(),b"verified payload");
    }
    #[test]
    fn cleanup_cannot_delete_a_same_named_replacement() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let staging=root.join(".owned.part");let moved=root.join("original.part");
        let target=StagingTarget::new(&staging,&root.join("destination")).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"owned").unwrap();file.sync_all().unwrap();
        #[cfg(unix)] {
            std::fs::rename(&staging,&moved).unwrap();std::fs::write(&staging,b"keep replacement").unwrap();
            assert!(target.remove_open(&file).is_err());assert_eq!(std::fs::read(&staging).unwrap(),b"keep replacement");assert_eq!(std::fs::read(&moved).unwrap(),b"owned");
        }
        #[cfg(windows)] {
            assert!(std::fs::rename(&staging,&moved).is_err(),"the live receiving handle must deny name replacement");
            target.remove_open(&file).unwrap();drop(file);assert!(!staging.exists());
        }
    }
    #[test]
    fn cleanup_remains_anchored_when_the_parent_path_changes() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=root.join("parent");std::fs::create_dir(&parent).unwrap();let moved=root.join("moved-parent");
        let staging=parent.join(".owned.part");let target=StagingTarget::new(&staging,&parent.join("destination")).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"owned").unwrap();file.sync_all().unwrap();
        #[cfg(unix)] {
            std::fs::rename(&parent,&moved).unwrap();std::fs::create_dir(&parent).unwrap();std::fs::write(&staging,b"keep replacement parent").unwrap();
            target.remove_open(&file).unwrap();drop(file);assert!(!moved.join(".owned.part").exists());assert_eq!(std::fs::read(&staging).unwrap(),b"keep replacement parent");
        }
        #[cfg(windows)] {
            assert!(std::fs::rename(&parent,&moved).is_err());target.remove_open(&file).unwrap();drop(file);assert!(!staging.exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_a_replaced_link_or_an_added_hardlink() {
        for link in [false,true] {
            let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let staging=root.join(".owned.part");let destination=root.join("destination");let other=root.join("other");
            let target=StagingTarget::new(&staging,&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"owned").unwrap();file.sync_all().unwrap();
            if link {std::fs::rename(&staging,root.join("original.part")).unwrap();std::fs::write(&other,b"keep linked target").unwrap();std::os::unix::fs::symlink(&other,&staging).unwrap();}
            else {std::fs::hard_link(&staging,&other).unwrap();}
            assert!(target.remove_open(&file).is_err());assert!(std::fs::symlink_metadata(&staging).is_ok());
            assert_eq!(std::fs::read(&other).unwrap(),if link {&b"keep linked target"[..]}else{&b"owned"[..]});
        }
    }
    #[test]
    fn receiving_and_source_open_reject_linked_ancestors_and_hardlinked_staging() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let outside=root.join("outside");std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"),b"original").unwrap();let alias=root.join("alias");link_directory(&outside,&alias);
        assert!(StagingTarget::new(&alias.join(".stage"),&alias.join("destination")).is_err());
        assert!(open_source(&alias.join("keep")).is_err());assert!(!outside.join(".stage").exists());
        let stage=root.join(".hardlink");std::fs::hard_link(outside.join("keep"),&stage).unwrap();
        let target=StagingTarget::new(&stage,&root.join("destination")).unwrap();assert!(target.open(false).is_err());
        assert_eq!(std::fs::read(outside.join("keep")).unwrap(),b"original");
        #[cfg(windows)]
        std::fs::remove_dir(&alias).unwrap();
    }
    #[test]
    fn a_parent_swap_cannot_redirect_an_open_receiving_file_or_its_publication() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let selected=root.join("selected");let moved=root.join("moved");let outside=root.join("outside");
        std::fs::create_dir(&selected).unwrap();std::fs::create_dir(&outside).unwrap();std::fs::write(outside.join("destination"),b"untouched").unwrap();
        let target=StagingTarget::new(&selected.join(".stage"),&selected.join("destination")).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"first").unwrap();
        let swapped=std::fs::rename(&selected,&moved).is_ok();if swapped{link_directory(&outside,&selected);}
        file.write_all(b" second").unwrap();target.publish(file,false).unwrap();
        assert_eq!(std::fs::read(if swapped{moved.join("destination")}else{selected.join("destination")}).unwrap(),b"first second");
        assert_eq!(std::fs::read(outside.join("destination")).unwrap(),b"untouched");assert!(!outside.join(".stage").exists());
        #[cfg(windows)]
        if swapped{std::fs::remove_dir(&selected).unwrap();}
    }
    #[cfg(windows)]
    #[test]
    fn receiving_pins_every_parent_before_creating_staging_and_until_the_outcome_is_recorded() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let ancestor=root.join("ancestor");let parent=ancestor.join("parent");
        std::fs::create_dir_all(&parent).unwrap();
        let target=StagingTarget::new(&parent.join(".stage"),&parent.join("destination")).unwrap();
        assert!(std::fs::rename(&ancestor,root.join("moved-ancestor")).is_err(),"an ancestor moved before staging was created");
        assert!(std::fs::rename(&parent,ancestor.join("moved-parent")).is_err(),"the target parent moved before staging was created");
        let mut file=target.open(true).unwrap();file.write_all(b"complete").unwrap();target.publish_open(&file,false).unwrap();drop(file);
        assert!(std::fs::rename(&ancestor,root.join("moved-ancestor")).is_err(),"parent pin ended before durable outcome acknowledgement");
        assert!(std::fs::rename(&parent,ancestor.join("moved-parent")).is_err());
        drop(target);
        std::fs::rename(&ancestor,root.join("moved-ancestor")).unwrap();
        assert_eq!(std::fs::read(root.join("moved-ancestor/parent/destination")).unwrap(),b"complete");
    }
    #[test]
    fn frozen_parent_rejects_a_same_named_replacement_before_writing_or_recovering() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();
        let parent=root.join("selected");let original=root.join("original");std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("destination"),b"identical bytes").unwrap();
        let observed=crate::DirectoryObject::open(&crate::encode_path(&parent),false).unwrap();let expected=observed.identity().to_owned();drop(observed);
        std::fs::rename(&parent,&original).unwrap();std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("destination"),b"identical bytes").unwrap();
        std::fs::write(parent.join(".previous"),b"must not adopt or truncate").unwrap();
        let stage=parent.join(".stage");let destination=parent.join("destination");
        assert!(matches!(StagingTarget::new_in_parent(&stage,&destination,Some(&expected)),Err(error) if error.kind()==io::ErrorKind::InvalidData));
        assert!(matches!(open_source_in_parent(&destination,Some(&expected)),Err(error) if error.kind()==io::ErrorKind::InvalidData));
        assert!(!stage.exists());assert_eq!(std::fs::read(parent.join(".previous")).unwrap(),b"must not adopt or truncate");
        assert_eq!(std::fs::read(&destination).unwrap(),b"identical bytes");assert_eq!(std::fs::read(original.join("destination")).unwrap(),b"identical bytes");
        std::fs::rename(&parent,root.join("replacement")).unwrap();std::fs::rename(&original,&parent).unwrap();
        let target=StagingTarget::new_in_parent(&stage,&destination,Some(&expected)).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"new verified bytes").unwrap();target.publish(file,true).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(),b"new verified bytes");
        assert!(open_source_in_parent(&destination,Some(&expected)).is_ok());
        assert_eq!(std::fs::read(root.join("replacement/destination")).unwrap(),b"identical bytes");
    }
}
