//! Read-only native supervisor acceptance probe. Run as SYSTEM on a test
//! machine with active Windows user sessions; it creates no processes/apps.
#[cfg(windows)]
#[allow(dead_code)]
mod probe {
    include!("../src/windows_session_supervisor.rs");

    fn elevation(token: &OwnedHandle) -> io::Result<i32> {
        let mut value = 0i32;
        let mut needed = 0;
        // SAFETY: TokenElevationType writes one i32 into this live buffer.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenElevationType,
                (&mut value as *mut i32).cast(),
                4,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }

    pub fn main() -> Result<(), Box<dyn std::error::Error>> {
        let targets = application_targets()?;
        if targets.is_empty() {
            return Err("no active user sessions; cannot validate token selection".into());
        }
        for target in targets {
            let original = user_token(target.session_id)?;
            let app = application_token(target)?;
            assert_eq!(logon_id(&original)?, logon_id(&app)?);
            assert_eq!(elevation(&original)?, elevation(&app)?);
            let window = interactive_token(target)?;
            if elevation(&original)? == TokenElevationTypeLimited {
                assert_ne!(elevation(&window)?, TokenElevationTypeLimited);
            }
            let wrong = Target {
                user_logon_id: Some((0, -1)),
                ..target
            };
            assert!(
                application_token(wrong).is_err(),
                "changed logon must reject"
            );
            let user_data = UserEnvironment::new(&app)?.local_data()?;
            assert!(
                !user_data
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("systemprofile")
            );
            println!(
                "{}",
                serde_json::json!({
                    "session_id":target.session_id, "raw_elevation":elevation(&original)?,
                    "app_elevation":elevation(&app)?, "window_elevation":elevation(&window)?,
                    "logon_matches":true, "changed_logon_rejected":true, "user_local_data":user_data,
                })
            );
        }
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    probe::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows SYSTEM test environment required");
    std::process::exit(1);
}
