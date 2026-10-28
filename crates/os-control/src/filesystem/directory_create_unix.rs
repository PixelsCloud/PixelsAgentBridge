//! Publish an opened directory from a private, same-filesystem staging parent.
//!
//! mkdirat cannot return a descriptor. The intermediate parent is random and
//! owner-only; the child is opened there before it becomes a public target.
//! Descriptors anchor both rename endpoints, and RENAME_NOREPLACE/RENAME_EXCL
//! rejects every existing destination, including links and empty directories.
//! This is not a sandbox against root or a hostile process with the same UID.
use std::{ffi::{CStr,CString,OsStr},fs::File,io,os::{fd::{AsRawFd,FromRawFd},unix::{ffi::OsStrExt,fs::MetadataExt}}};

pub fn create_directory_at(parent:&File,name:&OsStr)->io::Result<File> {
    create_with_hook(parent,name,||Ok(()))
}

fn create_with_hook(parent:&File,name:&OsStr,before_publish:impl FnOnce()->io::Result<()>)->io::Result<File> {
    let _=component(name)?;
    let private=PrivateDirectory::new(parent)?;
    publish_in_private(parent,name,private,|_,_|Ok(()),before_publish)
}

/// The caller persists a random staging name before this call and checkpoints
/// the opened identities before publication. Its journal lock must remain held
/// until this function (including normal cleanup) has returned.
pub fn create_directory_at_tracked(parent:&File,name:&OsStr,staging:&OsStr,checkpoint:impl FnMut(&File,Option<&File>)->io::Result<()>)->io::Result<File> {
    let _=component(name)?;
    validate_staging_name(staging)?;
    let private=PrivateDirectory::named(parent,component(staging)?)?;
    publish_in_private(parent,name,private,checkpoint,||Ok(()))
}

fn publish_in_private(parent:&File,name:&OsStr,private:PrivateDirectory<'_>,mut checkpoint:impl FnMut(&File,Option<&File>)->io::Result<()>,before_publish:impl FnOnce()->io::Result<()>)->io::Result<File> {
    checkpoint(&private.file,None)?;
    mkdir(&private.file,c"entry",0o777)?;
    let file=open_directory(&private.file,c"entry")?;
    let metadata=file.metadata()?;
    if !metadata.is_dir() {return Err(io::Error::other("staged directory has an invalid type"));}
    checkpoint(&private.file,Some(&file))?;
    // The descriptor is retained through publication and returned to the
    // caller, which retains it until the original operation result is durable.
    before_publish()?;
    let current=entry_metadata(&private.file,c"entry")?;
    if current.st_dev as u64!=metadata.dev() || current.st_ino as u64!=metadata.ino() || current.st_mode & libc::S_IFMT!=libc::S_IFDIR {
        return Err(io::Error::other("staged directory identity changed"));
    }
    super::rename_between_no_replace(&private.file,OsStr::new("entry"),parent,name)?;
    // Publication is already a fact. A filesystem that cannot sync directories
    // must not turn it into a fabricated no-effect failure. Power-loss durability
    // is a separate capability; this interface records process-recovery facts.
    let _=file.sync_all();let _=parent.sync_all();
    Ok(file)
}

struct PrivateDirectory<'a> {parent:&'a File,name:CString,file:File}
impl<'a> PrivateDirectory<'a> {
    fn new(parent:&'a File)->io::Result<Self> {
        for _ in 0..8 {
            let name=CString::new(format!(".pab-dir-{}",pab_protocol::RequestId::new())).unwrap();
            match Self::named(parent,name) {
                Err(error) if error.kind()==io::ErrorKind::AlreadyExists=>continue,
                Err(error)=>return Err(error),
                Ok(guard)=>return Ok(guard),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists,"cannot allocate a private directory"))
    }
    fn named(parent:&'a File,name:CString)->io::Result<Self> {
        mkdir(parent,&name,0o700)?;
        let file=open_directory(parent,&name)?;
        let guard=Self {parent,name,file};
        verify_private_directory(&guard.file)?;
        Ok(guard)
    }
}

pub fn verify_private_directory(file:&File)->io::Result<()> {
    let metadata=file.metadata()?;
    if metadata.uid()!=unsafe {libc::geteuid()} || metadata.mode()&0o077!=0 || !metadata.is_dir() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,"directory is not private to the executing user"));
    }
    Ok(())
}

/// Capability directories may be O_PATH descriptors on Linux. Obtain an
/// actual readable directory FD relative to that anchor before fsync.
pub fn sync_directory_handle(file:&File)->io::Result<()> {open_directory(file,c".")?.sync_all()}

fn validate_staging_name(name:&OsStr)->io::Result<()> {
    let text=name.to_str().and_then(|name|name.strip_prefix(".pab-dir-")).ok_or_else(||io::Error::new(io::ErrorKind::InvalidInput,"invalid staging name"))?;
    let id=text.parse::<pab_protocol::RequestId>().map_err(|_|io::Error::new(io::ErrorKind::InvalidInput,"invalid staging ID"))?;
    if id.to_string()!=text {return Err(io::Error::new(io::ErrorKind::InvalidInput,"noncanonical staging ID"));}
    Ok(())
}

/// Remove only the exact, identity-checked empty objects from an inactive
/// creation journal. The public target is deliberately not an input. Missing
/// identity checkpoints remain uncertain and are never guessed from a name.
pub fn cleanup_directory_staging_at(parent:&File,name:&OsStr,expected_private:Option<&str>,expected_entry:Option<&str>)->io::Result<()> {
    validate_staging_name(name)?;
    let name=component(name)?;
    let private=match open_directory(parent,&name) {Ok(file)=>file,Err(error) if error.kind()==io::ErrorKind::NotFound=>return Ok(()),Err(error)=>return Err(error)};
    verify_private_directory(&private)?;
    let identity=super::file_object_identity(&private)?;
    if expected_private!=Some(identity.as_str()) {return Err(io::Error::other("staging directory identity is unconfirmed or changed"));}
    match entry_metadata(&private,c"entry") {
        Ok(metadata)=>{
            if metadata.st_mode & libc::S_IFMT!=libc::S_IFDIR {return Err(io::Error::other("staging child is not an ordinary directory"));}
            let child=open_directory(&private,c"entry")?;
            if expected_entry!=Some(super::file_object_identity(&child)?.as_str()) {return Err(io::Error::other("staging child identity is unconfirmed or changed"));}
            if unsafe {libc::unlinkat(private.as_raw_fd(),c"entry".as_ptr(),libc::AT_REMOVEDIR)}!=0 {return Err(io::Error::last_os_error());}
        },
        Err(error) if error.kind()==io::ErrorKind::NotFound=>{},
        Err(error)=>return Err(error),
    }
    let named=entry_metadata(parent,&name)?;let opened=private.metadata()?;
    if opened.dev()!=named.st_dev as u64 || opened.ino()!=named.st_ino as u64 || named.st_mode & libc::S_IFMT!=libc::S_IFDIR {return Err(io::Error::other("staging name changed during cleanup"));}
    if unsafe {libc::unlinkat(parent.as_raw_fd(),name.as_ptr(),libc::AT_REMOVEDIR)}!=0 {return Err(io::Error::last_os_error());}
    Ok(())
}
impl Drop for PrivateDirectory<'_> {
    fn drop(&mut self) {
        // Only remove empty staging objects. Never recursively traverse content
        // or delete the public destination, including on failed publication.
        if let Ok(entry)=entry_metadata(&self.file,c"entry") {
            if entry.st_mode & libc::S_IFMT==libc::S_IFDIR {
                let _=unsafe {libc::unlinkat(self.file.as_raw_fd(),c"entry".as_ptr(),libc::AT_REMOVEDIR)};
            }
        }
        if let (Ok(opened),Ok(named))=(self.file.metadata(),entry_metadata(self.parent,&self.name)) {
            if opened.dev()==named.st_dev as u64 && opened.ino()==named.st_ino as u64 && named.st_mode & libc::S_IFMT==libc::S_IFDIR {
                let _=unsafe {libc::unlinkat(self.parent.as_raw_fd(),self.name.as_ptr(),libc::AT_REMOVEDIR)};
            }
        }
    }
}
fn component(name:&OsStr)->io::Result<CString> {
    super::single_component(name)?;
    CString::new(name.as_bytes()).map_err(|_|io::Error::new(io::ErrorKind::InvalidInput,"NUL in directory name"))
}
fn mkdir(parent:&File,name:&CStr,mode:libc::mode_t)->io::Result<()> {
    // Borrowed parent and NUL-terminated single component remain live.
    if unsafe {libc::mkdirat(parent.as_raw_fd(),name.as_ptr(),mode)}==0 {Ok(())}else{Err(io::Error::last_os_error())}
}
fn open_directory(parent:&File,name:&CStr)->io::Result<File> {
    let fd=unsafe {libc::openat(parent.as_raw_fd(),name.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC)};
    if fd<0 {Err(io::Error::last_os_error())}else{Ok(unsafe {File::from_raw_fd(fd)})}
}
fn entry_metadata(parent:&File,name:&CStr)->io::Result<libc::stat> {
    let mut value=std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {libc::fstatat(parent.as_raw_fd(),name.as_ptr(),value.as_mut_ptr(),libc::AT_SYMLINK_NOFOLLOW)}!=0 {Err(io::Error::last_os_error())}else{Ok(unsafe {value.assume_init()})}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString,os::unix::{ffi::OsStringExt,fs::{symlink,PermissionsExt}}};
    #[test]
    fn concurrent_publishers_have_one_winner_and_leave_no_staging_directories() {
        let temp=tempfile::tempdir().unwrap();let parent=std::sync::Arc::new(File::open(temp.path()).unwrap());let barrier=std::sync::Arc::new(std::sync::Barrier::new(12));
        let tasks:Vec<_>=(0..12).map(|_|{let parent=parent.clone();let barrier=barrier.clone();std::thread::spawn(move||create_with_hook(&parent,OsStr::new("winner"),||{barrier.wait();Ok(())}))}).collect();
        let results:Vec<_>=tasks.into_iter().map(|task|task.join().unwrap()).collect();assert_eq!(results.iter().filter(|r|r.is_ok()).count(),1);
        assert!(results.iter().filter_map(|r|r.as_ref().err()).all(|e|e.kind()==io::ErrorKind::AlreadyExists));assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(),1);
        let winner=results.into_iter().find_map(Result::ok).unwrap();assert_eq!(super::super::file_object_identity(&winner).unwrap(),super::super::file_object_identity(&File::open(temp.path().join("winner")).unwrap()).unwrap());
    }
    #[test]
    fn publication_retains_identity_and_anchors_parent_even_after_rename() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("parent");std::fs::create_dir(&path).unwrap();let parent=File::open(&path).unwrap();
        let moved=temp.path().join("original");
        let created=create_with_hook(&parent,OsStr::new("created"),||{std::fs::rename(&path,&moved)?;std::fs::create_dir(&path)?;Ok(())}).unwrap();
        assert!(moved.join("created").is_dir());assert_eq!(std::fs::read_dir(&path).unwrap().count(),0);
        let identity=super::super::file_object_identity(&created).unwrap();assert_eq!(super::super::file_object_identity(&File::open(moved.join("created")).unwrap()).unwrap(),identity);
        std::fs::rename(moved.join("created"),moved.join("renamed")).unwrap();std::fs::create_dir(moved.join("created")).unwrap();
        assert_eq!(super::super::file_object_identity(&created).unwrap(),identity);assert_ne!(super::super::file_object_identity(&File::open(moved.join("created")).unwrap()).unwrap(),identity);
        assert!(std::fs::read_dir(&moved).unwrap().all(|e|!e.unwrap().file_name().to_string_lossy().starts_with(".pab-dir-")));
    }
    #[test]
    fn collision_link_and_injected_failure_never_replace_or_leave_normal_staging() {
        let temp=tempfile::tempdir().unwrap();let parent=File::open(temp.path()).unwrap();
        let target=temp.path().join("target");
        let error=create_with_hook(&parent,OsStr::new("target"),||{std::fs::create_dir(&target)?;std::fs::write(target.join("sentinel"),b"keep")?;Ok(())}).unwrap_err();assert_eq!(error.kind(),io::ErrorKind::AlreadyExists);assert_eq!(std::fs::read(target.join("sentinel")).unwrap(),b"keep");
        symlink(&target,temp.path().join("link")).unwrap();assert!(create_directory_at(&parent,OsStr::new("link")).is_err());assert!(std::fs::symlink_metadata(temp.path().join("link")).unwrap().file_type().is_symlink());
        assert!(create_with_hook(&parent,OsStr::new("absent"),||Err(io::Error::other("injected failure"))).is_err());assert!(!temp.path().join("absent").exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(),2);
        for name in ["..","nested/name","nul\0value",""] {assert!(create_directory_at(&parent,OsStr::new(name)).is_err());}
        let raw=OsString::from_vec(vec![b'n',0xff]);let file=create_directory_at(&parent,&raw).unwrap();assert!(file.metadata().unwrap().is_dir());assert!(temp.path().join(raw).is_dir());
        // A private staging parent must not leave the published directory
        // permanently inaccessible to its owner.
        assert_ne!(file.metadata().unwrap().permissions().mode()&0o700,0);
    }
}
