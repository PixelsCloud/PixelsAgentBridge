// The desktop writes diagnostics to rotating files in every build profile.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    match std::env::args().nth(1).as_deref() {
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
