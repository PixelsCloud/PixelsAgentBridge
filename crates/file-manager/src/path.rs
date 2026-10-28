use crate::FileManagerError;
use pab_protocol::NativeFilePath;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub fn encode_path(path: &Path) -> NativeFilePath {
    if let Some(value) = path.to_str().filter(|value| value.len() <= pab_protocol::MAX_NATIVE_PATH_UNITS) {
        return NativeFilePath::Utf8(value.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        NativeFilePath::UnixBytes(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        NativeFilePath::WindowsWide(path.as_os_str().encode_wide().collect())
    }
}
pub fn decode_path(path: &NativeFilePath) -> Result<PathBuf, FileManagerError> {
    path.validate().map_err(FileManagerError::Invalid)?;
    let native = match path {
        NativeFilePath::Utf8(value) => PathBuf::from(value),
        #[cfg(unix)]
        NativeFilePath::UnixBytes(value) => {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(value.clone()))
        }
        #[cfg(windows)]
        NativeFilePath::WindowsWide(value) => {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(value))
        }
        _ => {
            return Err(FileManagerError::Invalid(
                "native path belongs to another platform",
            ));
        }
    };
    if !native.is_absolute() {
        return Err(FileManagerError::Invalid("native path must be absolute"));
    }
    Ok(native)
}
pub fn display_name(name: &OsStr) -> String {
    if let Some(value) = name.to_str() {
        return value.into();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes()
            .iter()
            .map(|b| {
                if b.is_ascii_graphic() && *b != b'\\' {
                    char::from(*b).to_string()
                } else {
                    format!("\\x{b:02x}")
                }
            })
            .collect()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        char::decode_utf16(name.encode_wide())
            .map(|c| {
                c.map(|c| c.to_string())
                    .unwrap_or_else(|e| format!("\\u{{{:04x}}}", e.unpaired_surrogate()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_path_identity_and_platform_boundary() {
        let root = std::env::current_dir().unwrap();
        assert_eq!(decode_path(&encode_path(&root)).unwrap(), root);
        assert!(decode_path(&NativeFilePath::Utf8("relative".into())).is_err());
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let native = PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800]));
            assert_eq!(decode_path(&encode_path(&native)).unwrap(), native);
            assert!(display_name(native.file_name().unwrap()).contains("d800"));
            assert!(decode_path(&NativeFilePath::UnixBytes(b"/tmp".to_vec())).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let native = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
            assert_eq!(decode_path(&encode_path(&native)).unwrap(), native);
            assert_eq!(display_name(native.file_name().unwrap()), "\\xff");
            assert!(decode_path(&NativeFilePath::WindowsWide(vec![67, 58, 92])).is_err());
        }
    }
}
