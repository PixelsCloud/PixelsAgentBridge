fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Declare the executable as a pre-login graphical agent, as used by
        // Chromium remoting and RustDesk. TCC authorization still applies.
        // Limit this to the app executable, not its test/library targets.
        println!("cargo:rustc-link-arg-bin=pab-desktop=-Wl,-sectcreate,__CGPreLoginApp,__cgpreloginapp,/dev/null");
    }
    tauri_build::build()
}
