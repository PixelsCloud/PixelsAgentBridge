//! Redistributable native mechanics probe. Only owns tempfile-created data.
use pab_file_manager::{create_directory, decode_path, delete, encode_path, inspect, rename};
use pab_protocol::{FileByteCount, ManagedFileKind};
mod file_manager_fixture;

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let parent = std::env::args()
        .nth(1)
        .ok_or("provide an explicitly owned fixture parent")?;
    let root = tempfile::tempdir_in(parent)?;
    let outside = root.path().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("keep.bin"), b"outside-content")?;
    let selected = create_directory(&encode_path(root.path()), "中文 folder")?;
    assert!(create_directory(&encode_path(root.path()), "中文 folder").is_err());
    let file = decode_path(&selected)?.join("empty.bin");
    std::fs::write(&file, [])?;
    let entry = inspect(&encode_path(&file)).await?;
    assert_eq!(entry.kind, ManagedFileKind::File);
    assert_eq!(entry.size, Some(FileByteCount(0)));
    assert_eq!(decode_path(&entry.path)?, file);
    let renamed_file = rename(&entry.path, "renamed.bin")?;
    std::fs::write(decode_path(&selected)?.join("occupied.bin"), b"original")?;
    assert!(rename(&renamed_file, "occupied.bin").is_err());
    assert_eq!(
        std::fs::read(decode_path(&selected)?.join("occupied.bin"))?,
        b"original"
    );
    let moved = rename(&selected, "renamed folder")?;
    let linked = decode_path(&moved)?.join("external");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &linked)?;
    #[cfg(windows)]
    {
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside)
            .output()?;
        if !result.status.success() {
            return Err("cannot create owned junction fixture".into());
        }
    }
    assert!(create_directory(&encode_path(&linked), "must-not-exist").is_err());
    assert!(!outside.join("must-not-exist").exists());
    assert!(delete(&moved, false).is_err());
    delete(&moved, true)?;
    assert_eq!(std::fs::read(outside.join("keep.bin"))?, b"outside-content");
    assert!(!decode_path(&moved)?.exists());

    #[cfg(windows)]
    let raw_name = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800, 0x2e, 0x74, 0x78, 0x74])
    };
    #[cfg(unix)]
    let raw_name = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff, b'.', b'b', b'i', b'n'])
    };
    let raw_path = root.path().join(raw_name);
    std::fs::write(&raw_path, b"native identity")?;
    let entry = inspect(&encode_path(&raw_path)).await?;
    assert_eq!(decode_path(&entry.path)?, raw_path);
    let renamed = rename(&entry.path, "native-renamed.bin")?;
    assert_eq!(std::fs::read(decode_path(&renamed)?)?, b"native identity");
    delete(&renamed, false)?;
    file_manager_fixture::cleanup_fixture(root).await?;
    println!(
        "{}",
        serde_json::json!({"status":"passed","probe":"native_file_manager_operations","exclusive_create":true,"no_clobber_rename":true,"zero_byte_metadata":true,"native_path_roundtrip":true,"linked_parent_rejected":true,"external_link_target_preserved":true,"cleanup_complete":true})
    );
    Ok(())
}
