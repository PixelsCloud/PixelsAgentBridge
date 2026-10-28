//! Attributes are applied to the same retained file, before publication.
use std::{fs::{File,FileTimes,Metadata},io,time::{Duration,SystemTime,UNIX_EPOCH}};
use pab_protocol::{FileTransferAttribute as Attribute,FileTransferMetadata,FileTransferMetadataResult,FileTransferMetadataWarning};

pub fn modified_at_unix_ms(metadata:&Metadata)->Option<i64> {
    let time=metadata.modified().ok()?;
    let millis=match time.duration_since(UNIX_EPOCH) {
        Ok(duration)=>i128::try_from(duration.as_millis()).ok()?,
        // Floor negative sub-millisecond times consistently with positive
        // timestamps, without treating pre-1970 files as unknown metadata.
        Err(error)=>-i128::try_from(error.duration().as_nanos().div_ceil(1_000_000)).ok()?,
    };
    i64::try_from(millis).ok()
}
pub fn file_transfer_metadata(metadata:&Metadata)->FileTransferMetadata {
    #[cfg(unix)]
    let mode={use std::os::unix::fs::PermissionsExt;Some(metadata.permissions().mode()&0o777)};
    #[cfg(not(unix))]
    let mode=None;
    FileTransferMetadata {modified_at_unix_ms:modified_at_unix_ms(metadata),unix_mode:mode}
}
fn timestamp(millis:i64)->io::Result<SystemTime> {
    let duration=Duration::from_millis(millis.unsigned_abs());
    (if millis>=0 {UNIX_EPOCH.checked_add(duration)}else{UNIX_EPOCH.checked_sub(duration)})
        .ok_or_else(||io::Error::new(io::ErrorKind::InvalidInput,"file timestamp is outside the platform range"))
}

/// Call only on the source handle that will actually be hashed and streamed.
pub fn verify_source_metadata(file:&File,size:u64,modified:Option<i64>)->io::Result<()> {
    let metadata=file.metadata()?;
    verify_opened_source_metadata(&metadata,size,modified)
}
/// `metadata` must come from the retained source handle, never an ambient stat
/// followed by a new path open. Tokio file adapters can use their fstat result.
pub fn verify_opened_source_metadata(metadata:&Metadata,size:u64,modified:Option<i64>)->io::Result<()> {
    if !metadata.is_file() || metadata.len()!=size || modified.is_some_and(|expected|modified_at_unix_ms(metadata)!=Some(expected)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData,"source changed after it was selected"));
    }
    Ok(())
}

/// Observe the verified final object after an interrupted attribute outcome.
/// This deliberately does not retry chmod/set_times or alter user files.
pub fn observe_file_metadata(metadata:&Metadata,requested:&FileTransferMetadata)->io::Result<FileTransferMetadataResult> {
    requested.validate().map_err(|message|io::Error::new(io::ErrorKind::InvalidInput,message))?;
    let actual=file_transfer_metadata(metadata);let mut warnings=Vec::new();
    for (attribute,missing) in [
        (Attribute::ModifiedTime,requested.modified_at_unix_ms.is_some() && requested.modified_at_unix_ms!=actual.modified_at_unix_ms),
        (Attribute::UnixMode,requested.unix_mode.is_some() && requested.unix_mode!=actual.unix_mode),
    ] {
        if missing {let unsupported=attribute==Attribute::UnixMode && actual.unix_mode.is_none();warnings.push(FileTransferMetadataWarning {
            attribute,code:if unsupported {"unsupported"}else{"not_preserved"}.into(),
            message:if unsupported {"The destination does not support POSIX permissions"}else{"The file contents were verified after interruption, but this attribute was not preserved"}.into(),
        });}
    }
    Ok(FileTransferMetadataResult {requested:requested.clone(),actual,warnings})
}

/// Setting attributes is best effort, but its outcome is explicit. Invalid
/// privileged bits are rejected before changing any attribute. The caller
/// retains this handle through its subsequent sync and atomic publication.
pub fn apply_file_metadata(file:&File,requested:&FileTransferMetadata)->io::Result<FileTransferMetadataResult> {
    requested.validate().map_err(|message|io::Error::new(io::ErrorKind::InvalidInput,message))?;
    let mut warnings=Vec::new();
    let mut warn=|attribute,code:&str,message:String|warnings.push(FileTransferMetadataWarning {attribute,code:code.into(),message:message.chars().take(256).collect()});
    if let Some(modified)=requested.modified_at_unix_ms {
        if let Err(error)=timestamp(modified).and_then(|time|file.set_times(FileTimes::new().set_modified(time))) {warn(Attribute::ModifiedTime,"set_failed",error.to_string());}
    }
    if let Some(mode)=requested.unix_mode {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error)=file.set_permissions(std::fs::Permissions::from_mode(mode)){warn(Attribute::UnixMode,"set_failed",error.to_string());}
        }
        #[cfg(not(unix))]
        {
            let _=mode;
            warn(Attribute::UnixMode,"unsupported","The destination does not support POSIX permissions".into());
        }
    }
    let actual=file_transfer_metadata(&file.metadata()?);
    if requested.modified_at_unix_ms.is_some() && requested.modified_at_unix_ms!=actual.modified_at_unix_ms && !warnings.iter().any(|warning|warning.attribute==Attribute::ModifiedTime) {
        warnings.push(FileTransferMetadataWarning {attribute:Attribute::ModifiedTime,code:"precision_loss".into(),message:"The destination stored a different modification time".into()});
    }
    if requested.unix_mode.is_some() && requested.unix_mode!=actual.unix_mode && !warnings.iter().any(|warning|warning.attribute==Attribute::UnixMode) {
        warnings.push(FileTransferMetadataWarning {attribute:Attribute::UnixMode,code:"not_preserved".into(),message:"The destination did not retain the requested permissions".into()});
    }
    Ok(FileTransferMetadataResult {requested:requested.clone(),actual,warnings})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Seek,Write};
    #[test]
    fn staging_metadata_and_bytes_survive_the_same_handle_atomic_publication() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let destination=root.join("result");std::fs::write(&destination,b"original").unwrap();
        let target=crate::StagingTarget::new(&root.join("staging"),&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"complete content").unwrap();
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:if cfg!(unix){Some(0o640)}else{None}};
        let result=apply_file_metadata(&file,&metadata).unwrap();assert!(result.warnings.is_empty(),"{result:?}");
        assert_eq!(std::fs::read(&destination).unwrap(),b"original");target.publish(file,true).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(),b"complete content");let actual=file_transfer_metadata(&std::fs::metadata(&destination).unwrap());assert_eq!(actual.modified_at_unix_ms,metadata.modified_at_unix_ms);
        #[cfg(unix)] assert_eq!(actual.unix_mode,metadata.unix_mode);
    }
    #[test]
    fn final_permissions_use_the_retained_published_handle_without_reopening_its_name() {
        let temp=tempfile::tempdir().unwrap();let root=temp.path().canonicalize().unwrap();let destination=root.join("result");let target=crate::StagingTarget::new(&root.join("staging"),&destination).unwrap();let mut file=target.open(true).unwrap();file.write_all(b"complete content").unwrap();
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:if cfg!(unix){Some(0o440)}else{None}};
        let before=FileTransferMetadata {unix_mode:None,..metadata.clone()};assert!(apply_file_metadata(&file,&before).unwrap().warnings.is_empty());target.publish_open(&file,true).unwrap();
        let result=apply_file_metadata(&file,&metadata).unwrap();file.sync_all().unwrap();assert!(result.warnings.is_empty(),"{result:?}");drop(file);
        assert_eq!(std::fs::read(&destination).unwrap(),b"complete content");assert_eq!(file_transfer_metadata(&std::fs::metadata(&destination).unwrap()),result.actual);
    }
    #[test]
    fn source_validation_uses_the_open_file_and_rejects_changed_size_or_time() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("source");let mut file=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();file.write_all(b"old").unwrap();
        let metadata=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:None};assert!(apply_file_metadata(&file,&metadata).unwrap().warnings.is_empty());
        verify_source_metadata(&file,3,metadata.modified_at_unix_ms).unwrap();file.write_all(b"changed").unwrap();assert!(verify_source_metadata(&file,3,metadata.modified_at_unix_ms).is_err());file.set_len(3).unwrap();
        let mut changed=metadata.clone();changed.modified_at_unix_ms=Some(1_700_000_005_123);apply_file_metadata(&file,&changed).unwrap();assert!(verify_source_metadata(&file,3,metadata.modified_at_unix_ms).is_err());
        verify_source_metadata(&file,3,changed.modified_at_unix_ms).unwrap();
    }
    #[test]
    fn privileged_bits_are_rejected_before_any_change_and_pre_epoch_times_roundtrip() {
        let temp=tempfile::tempdir().unwrap();let path=temp.path().join("file");let mut file=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();file.write_all(b"unchanged").unwrap();
        let before=file_transfer_metadata(&file.metadata().unwrap());let invalid=FileTransferMetadata {modified_at_unix_ms:Some(1_700_000_000_123),unix_mode:Some(0o4755)};assert!(apply_file_metadata(&file,&invalid).is_err());assert_eq!(file_transfer_metadata(&file.metadata().unwrap()),before);
        let result=apply_file_metadata(&file,&FileTransferMetadata {modified_at_unix_ms:Some(-1),unix_mode:None}).unwrap();assert!(result.warnings.is_empty(),"{result:?}");assert_eq!(result.actual.modified_at_unix_ms,Some(-1));
        file.rewind().unwrap();let mut bytes=Vec::new();file.read_to_end(&mut bytes).unwrap();assert_eq!(bytes,b"unchanged");
    }
    #[cfg(not(unix))]
    #[test]
    fn unavailable_posix_permissions_are_explicitly_reported() {
        let file=tempfile::tempfile().unwrap();let result=apply_file_metadata(&file,&FileTransferMetadata {modified_at_unix_ms:None,unix_mode:Some(0o640)}).unwrap();assert_eq!(result.warnings.len(),1);assert_eq!(result.warnings[0].attribute,Attribute::UnixMode);assert_eq!(result.warnings[0].code,"unsupported");assert_eq!(result.actual.unix_mode,None);
    }
}
