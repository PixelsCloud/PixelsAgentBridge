//! Create exclusively relative to an opened parent and keep the created object.
use std::{ffi::OsStr, fs::File, io, os::windows::{ffi::OsStrExt, io::{AsRawHandle, FromRawHandle}}};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem::{NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT}},
    Win32::{Foundation::{RtlNtStatusToDosError, UNICODE_STRING, OBJ_CASE_INSENSITIVE}, Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE}, System::IO::IO_STATUS_BLOCK},
};

/// The returned handle belongs to the object created by this exact syscall.
/// No path reopen follows creation. Denying DELETE sharing pins that object
/// until its outcome is durable. Existing objects, including links, are errors.
pub fn create_directory_at(parent:&File,name:&OsStr)->io::Result<File> {
    super::single_component(name)?;
    let mut wide:Vec<u16>=name.encode_wide().collect();
    let bytes=wide.len().checked_mul(2).and_then(|bytes|u16::try_from(bytes).ok())
        .ok_or_else(||io::Error::new(io::ErrorKind::InvalidInput,"directory name is too long"))?;
    let name=UNICODE_STRING {Length:bytes,MaximumLength:bytes,Buffer:wide.as_mut_ptr()};
    let attributes=OBJECT_ATTRIBUTES {
        Length:std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory:parent.as_raw_handle(),ObjectName:&name,Attributes:OBJ_CASE_INSENSITIVE,
        SecurityDescriptor:std::ptr::null(),SecurityQualityOfService:std::ptr::null(),
    };
    let mut handle=std::ptr::null_mut();let mut status_block:IO_STATUS_BLOCK=unsafe {std::mem::zeroed()};
    // All buffers and the borrowed parent remain live. Synchronous operation
    // returns a single owned handle, with no inheritance or backup privileges.
    let status=unsafe {NtCreateFile(&mut handle,FILE_LIST_DIRECTORY|FILE_READ_ATTRIBUTES|SYNCHRONIZE,&attributes,&mut status_block,std::ptr::null(),FILE_ATTRIBUTE_DIRECTORY,FILE_SHARE_READ|FILE_SHARE_WRITE,FILE_CREATE,FILE_DIRECTORY_FILE|FILE_OPEN_REPARSE_POINT|FILE_SYNCHRONOUS_IO_NONALERT,std::ptr::null(),0)};
    if status<0 {return Err(io::Error::from_raw_os_error(unsafe {RtlNtStatusToDosError(status)} as i32));}
    if handle.is_null(){return Err(io::Error::other("directory create returned no handle"));}
    // NtCreateFile transfers this successful handle to the caller; File closes
    // it on all subsequent error paths as well as normal completion.
    let file=unsafe {File::from_raw_handle(handle)};
    use std::os::windows::fs::MetadataExt;
    let metadata=file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes()&0x400!=0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData,"created object is not an ordinary directory"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    fn parent(path:&std::path::Path)->File {std::fs::OpenOptions::new().read(true).share_mode(1|2).custom_flags(0x0220_0000).open(path).unwrap()}
    #[test]
    fn create_returns_the_original_object_and_pins_it_until_recording_finishes() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let parent=parent(&root);
        let created=create_directory_at(&parent,OsStr::new("created")).unwrap();let identity=super::super::file_object_identity(&created).unwrap();
        let opened=std::fs::OpenOptions::new().read(true).share_mode(1|2|4).custom_flags(0x0200_0000).open(root.join("created")).unwrap();
        assert_eq!(super::super::file_object_identity(&opened).unwrap(),identity);drop(opened);
        assert!(std::fs::rename(root.join("created"),root.join("moved")).is_err());
        assert!(std::fs::remove_dir(root.join("created")).is_err());
        std::fs::write(root.join("created/child"),b"child write remains allowed").unwrap();
        assert!(matches!(create_directory_at(&parent,OsStr::new("created")),Err(error) if error.kind()==io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(root.join("created/child")).unwrap(),b"child write remains allowed");
        drop(created);std::fs::rename(root.join("created"),root.join("moved")).unwrap();
        let replacement=create_directory_at(&parent,OsStr::new("created")).unwrap();assert_ne!(super::super::file_object_identity(&replacement).unwrap(),identity);
    }
    #[test]
    fn native_create_rejects_non_components_and_does_not_replace_files() {
        let temp=tempfile::tempdir().unwrap();let parent=parent(temp.path());
        for name in ["..","child/grandchild","child\\grandchild","file:stream","nul\0value"] {assert!(matches!(create_directory_at(&parent,OsStr::new(name)),Err(error) if error.kind()==io::ErrorKind::InvalidInput));}
        std::fs::write(temp.path().join("file"),b"unchanged").unwrap();assert!(create_directory_at(&parent,OsStr::new("file")).is_err());assert_eq!(std::fs::read(temp.path().join("file")).unwrap(),b"unchanged");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(),1);
    }
}
