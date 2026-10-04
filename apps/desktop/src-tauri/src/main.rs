// The desktop writes diagnostics to rotating files in every build profile.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "macos")]
    {
        // Finder/launchd do not read shell profiles. Preserve configured search order,
        // then append standard Homebrew and system locations before any threads start.
        let mut paths: Vec<_> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        for path in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ] {
            let path = std::path::PathBuf::from(path);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        if let Ok(path) = std::env::join_paths(paths) {
            unsafe {
                std::env::set_var("PATH", path);
            }
        }
    }
    match std::env::args().nth(1).as_deref() {
        #[cfg(target_os = "macos")]
        Some("--macos-check") => {
            println!("{}", pab_desktop_lib::macos_diagnostics());
        }
        Some("--session-helper") => {
            if let Err(error) = pab_desktop_lib::run_session_helper() {
                eprintln!("pab-session-helper: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(windows)]
        Some("--session-supervisor") => {
            if let Err(error) = pab_desktop_lib::run_session_supervisor() {
                eprintln!("pab-session-supervisor: {error}");
                std::process::exit(1);
            }
        }
        _ => pab_desktop_lib::run(),
    }
}
