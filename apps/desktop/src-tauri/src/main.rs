// The desktop writes diagnostics to rotating files in every build profile.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod webview_runtime;

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
        Some("--media-worker") => {
            std::process::exit(pab_desktop_media::worker_main(std::env::args().skip(2)));
        }
        #[cfg(target_os = "linux")]
        Some("--local-data-path") => {
            match pab_desktop_lib::linux_user_data_path() {
                Ok(path) => println!("{}", path.display()),
                Err(error) => { eprintln!("{error}"); std::process::exit(1); }
            }
        }
        #[cfg(target_os = "macos")]
        Some("--file-access-probe") => {
            if pab_executor::file_access::worker_main().is_err() { std::process::exit(1); }
        }
        Some("--ui-worker") => {
            if pab_desktop_control::ui_worker_entry::run().is_err() {
                std::process::exit(1);
            }
        }
        #[cfg(target_os = "macos")]
        Some("--macos-check") => {
            println!("{}", pab_desktop_lib::macos_diagnostics());
        }
        Some("--session-helper" | "--application-helper" | "--media-helper") => {
            if let Err(error) = pab_desktop_lib::run_session_helper() {
                eprintln!("pab-session-helper: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(any(windows, target_os = "linux"))]
        Some("--media-supervisor") => {
            if let Err(error) = pab_desktop_lib::run_media_supervisor() {
                eprintln!("pab-media-supervisor: {error}");
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
        _ => {
            #[cfg(windows)]
            webview_runtime::prepare();
            pab_desktop_lib::run();
        }
    }
}
