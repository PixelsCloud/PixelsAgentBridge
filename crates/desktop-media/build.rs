fn main() {
    println!("cargo:rerun-if-changed=native");
    // The source codec silently drops x86 SIMD when NASM is absent. Do not let
    // a release with unexpectedly slow fallback pass as a normal build.
    if matches!(
        std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86" | "x86_64")
    ) {
        assert!(
            std::process::Command::new("nasm")
                .arg("-v")
                .output()
                .is_ok_and(|v| v.status.success()),
            "NASM is required for the bundled software codec on x86/x64; install it before Cargo (after changing this toolchain, rebuild openh264-sys2)"
        );
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // One translation unit; no nested parallel build or network downloads.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include("native/windows")
        .include("native/vendor")
        .file("native/windows/worker.cpp")
        .flag_if_supported("/EHsc")
        .define("UNICODE", None)
        .define("_UNICODE", None)
        .compile("pab_desktop_media");
    for library in [
        "d3d11",
        "dxgi",
        "dxguid",
        "user32",
        "gdi32",
        "mfplat",
        "mfuuid",
        "wmcodecdspuuid",
        "ole32",
        "shcore",
        "shell32",
        "advapi32",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}
