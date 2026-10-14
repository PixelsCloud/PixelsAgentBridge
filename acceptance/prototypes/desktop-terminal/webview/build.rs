fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = root.join("../../../../apps/desktop/src-tauri/icons/icon.ico");
    std::fs::create_dir_all(root.join("icons")).unwrap();
    std::fs::copy(icon, root.join("icons/icon.ico")).unwrap();
    tauri_build::build();
}
