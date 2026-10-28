use crate::{FileManagerError, decode_path, display_name, encode_path};
use pab_protocol::{FileByteCount, ManagedFileEntry, ManagedFileKind, NativeFilePath, RequestId};

pub async fn inspect(path: &NativeFilePath) -> Result<ManagedFileEntry, FileManagerError> {
    let path = decode_path(path)?;
    let meta = tokio::fs::symlink_metadata(&path).await?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    let kind = if meta.file_type().is_symlink() || reparse {
        ManagedFileKind::Symlink
    } else if meta.is_dir() {
        ManagedFileKind::Directory
    } else if meta.is_file() {
        ManagedFileKind::File
    } else {
        ManagedFileKind::Other
    };
    let name = display_name(path.file_name().unwrap_or(path.as_os_str()));
    let mut result = ManagedFileEntry {
        id: RequestId::new(),
        name: name.clone(),
        path: encode_path(&path),
        kind,
        size: (kind == ManagedFileKind::File).then_some(FileByteCount(meta.len())),
        modified_at_unix_ms: crate::modified_at_unix_ms(&meta),
        hidden: name.starts_with('.'),
        readonly: meta.permissions().readonly(),
        unix_mode: None,
        error: None,
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        result.hidden |= meta.file_attributes() & 2 != 0;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        result.unix_mode = Some(meta.mode() & 0o777);
    }
    Ok(result)
}
