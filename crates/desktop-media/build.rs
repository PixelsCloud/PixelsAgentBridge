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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let mut native = cc::Build::new();
        native
            .cpp(true)
            .std("c++17")
            .file("native/macos/worker.mm")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .flag("-mmacosx-version-min=12.0")
            .compile("pab_desktop_media");
        // @available emits Clang runtime calls for older deployment targets.
        // Rust links with -nodefaultlibs, so explicitly propagate the runtime
        // selected by the same compiler (not a hard-coded Xcode path).
        let runtime = native
            .get_compiler()
            .to_command()
            .arg("--print-file-name=libclang_rt.osx.a")
            .output()
            .expect("query macOS Clang runtime");
        assert!(
            runtime.status.success(),
            "cannot locate macOS Clang runtime"
        );
        let path = String::from_utf8(runtime.stdout).expect("Clang runtime path is UTF-8");
        let path = std::path::Path::new(path.trim());
        assert!(
            path.is_file(),
            "macOS Clang runtime missing: {}",
            path.display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            path.parent().unwrap().display()
        );
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
        for library in [
            "Foundation",
            "AppKit",
            "ApplicationServices",
            "VideoToolbox",
            "CoreMedia",
            "CoreVideo",
            "IOSurface",
            "Metal",
            "QuartzCore",
        ] {
            println!("cargo:rustc-link-lib=framework={library}");
        }
        // The application still supports macOS 12.0. ScreenCaptureKit is
        // optional until a capture is requested on macOS 12.3 or later. The
        // native unit's 12.0 deployment target makes all SCK imports weak;
        // Apple's linker consequently emits LC_LOAD_WEAK_DYLIB for this entry.
        println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let mut native = cc::Build::new();
        native
            .cpp(true)
            .std("c++17")
            .file("native/linux/worker.cpp");
        for library in [
            "gio-2.0",
            "libpipewire-0.3",
            "gtk+-3.0",
            "epoxy",
            "x11",
            "xext",
            "xrandr",
            "xfixes",
            "xdamage",
        ] {
            let dependency = pkg_config::Config::new()
                .probe(library)
                .unwrap_or_else(|error| {
                    panic!("Linux desktop media requires {library} development files: {error}")
                });
            for path in dependency.include_paths {
                native.include(path);
            }
        }
        native.compile("pab_desktop_media");
        return;
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
