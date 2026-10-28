//! Identity-bound receiving files inside native-user-private directories.
//! Callers must durably store the receipt before writing file data. An unknown
//! directory is never adopted, swept by prefix, or treated as already cleaned.
use crate::{StagingTarget, PrivateFileStagingReceipt, decode_path, encode_path};
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt};
use pab_protocol::RequestId;
use std::{fs::File, io, os::unix::fs::MetadataExt, path::Path};

pub struct PrivateFileStaging {
    target:StagingTarget,
    directory:Dir,
    receipt:PrivateFileStagingReceipt,
}

impl PrivateFileStaging {
    /// The returned file is empty and exclusively locked. Persist `receipt()`
    /// in the owning task's private journal before writing bytes or checkpoints.
    /// A crash before that journal write leaves an unknown empty object, never
    /// permission to infer ownership from a name on a later directory scan.
    pub fn create(destination:&Path,overwrite:bool,expected_parent:Option<&str>)->io::Result<(Self,File)> {
        let name=format!(".pab-receive-{}",RequestId::new());
        let logical=destination.with_file_name(&name);
        let target=StagingTarget::new_in_parent(&logical,destination,expected_parent)?;
        target.check_destination(overwrite)?;
        let parent=target.destination_directory()?;
        let mut builder=DirBuilder::new();builder.mode(0o700);
        parent.create_dir_with(&name,&builder)?;
        let directory=parent.open_dir_nofollow(&name)?;
        let target=target.with_private_source(directory.try_clone()?)?;
        let file=target.open(true)?;
        let receipt=PrivateFileStagingReceipt {
            version:1,destination:encode_path(destination),
            parent_identity:identity(&parent)?,directory_name:name,
            directory_identity:identity(&directory)?,
            payload_identity:pab_os_control::filesystem::file_object_identity(&file)?,
        };
        // Process-recovery persistence only. Unsupported directory fsync does
        // not establish a separate power-loss durability promise.
        let _=pab_os_control::filesystem::sync_directory_handle(&directory.try_clone()?.into_std_file());
        let _=pab_os_control::filesystem::sync_directory_handle(&parent.into_std_file());
        Ok((Self {target,directory,receipt},file))
    }

    /// Reopen only the exact recorded private directory and payload. No create,
    /// truncate, guessed owner, or fallback to a similarly named flat partial.
    pub fn reopen(receipt:&PrivateFileStagingReceipt)->io::Result<(Self,File)> {
        receipt.validate()?;
        let destination=decode_path(&receipt.destination).map_err(io::Error::other)?;
        let logical=destination.with_file_name(&receipt.directory_name);
        let target=StagingTarget::new_in_parent(&logical,&destination,Some(&receipt.parent_identity))?;
        let parent=target.destination_directory()?;
        let directory=parent.open_dir_nofollow(&receipt.directory_name)?;
        if identity(&directory)?!=receipt.directory_identity {return Err(changed());}
        let target=target.with_private_source(directory.try_clone()?)?;
        let file=target.open(false)?;
        if pab_os_control::filesystem::file_object_identity(&file)?!=receipt.payload_identity {return Err(changed());}
        Ok((Self {target,directory,receipt:receipt.clone()},file))
    }

    pub fn receipt(&self)->&PrivateFileStagingReceipt {&self.receipt}

    /// Remove only the original open payload. A retained, empty directory is
    /// deliberately reported separately; this primitive never removes a public
    /// directory name by a check/unlink sequence. The owning journal must retain
    /// that resource until its separate directory cleanup is confirmed.
    pub fn discard_open(&self,file:&File)->io::Result<()> {
        self.verify(file)?;
        if file.metadata()?.nlink()==0 {return Ok(());}
        self.target.remove_open(file)
    }

    pub fn publish_open(&self,file:&File,overwrite:bool)->io::Result<crate::publish::PublicationReceipt> {
        self.verify(file)?;
        let mut publication=self.target.publish_open(file,overwrite)?;
        // Publication already happened; an empty-container cleanup failure
        // cannot turn that fact into a fabricated failed transfer.
        publication.staging_retained=!self.retire_empty_container().unwrap_or(false);
        Ok(publication)
    }

    /// Payload removal must be journaled separately before callers retire the
    /// container. A writable shared parent without sticky protection cannot
    /// support a race-free named rmdir: retain and account for that container.
    /// As with native directory creation, root and hostile same-UID code are
    /// outside this primitive's isolation boundary.
    pub fn retire_empty_container(&self)->io::Result<bool> {
        let private=self.directory.try_clone()?.into_std_file();
        pab_os_control::filesystem::verify_private_directory(&private)?;
        if identity(&self.directory)?!=self.receipt.directory_identity {return Err(changed());}
        if private.metadata()?.nlink()==0 {return Ok(true);}
        let parent=self.target.destination_directory()?;
        let metadata=parent.try_clone()?.into_std_file().metadata()?;
        let uid=private.metadata()?.uid();
        if (metadata.uid()!=uid && metadata.uid()!=0)
            || (metadata.mode()&0o022!=0 && metadata.mode()&0o1000==0) {
            return Ok(false);
        }
        let named=parent.open_dir_nofollow(&self.receipt.directory_name)?;
        if identity(&named)?!=self.receipt.directory_identity {return Err(changed());}
        // rmdir itself is the atomic empty check; no recursive cleanup and no
        // pathname-based removal of any payload or unrecorded child.
        parent.remove_dir(&self.receipt.directory_name)?;
        let _=pab_os_control::filesystem::sync_directory_handle(&parent.into_std_file());
        Ok(true)
    }

    fn verify(&self,file:&File)->io::Result<()> {
        pab_os_control::filesystem::verify_private_directory(&self.directory.try_clone()?.into_std_file())?;
        if identity(&self.directory)?!=self.receipt.directory_identity
            || pab_os_control::filesystem::file_object_identity(file)?!=self.receipt.payload_identity {
            return Err(changed());
        }
        Ok(())
    }
}

fn identity(directory:&Dir)->io::Result<String> {
    pab_os_control::filesystem::file_object_identity(&directory.try_clone()?.into_std_file())
}
fn changed()->io::Error {io::Error::new(io::ErrorKind::InvalidData,"receiving staging ownership changed")}
#[cfg(test)]
#[path="private_staging_tests.rs"]
mod tests;
