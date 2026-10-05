//! Release only inputs journaled by Bridge in this boot and graphical session.
use crate::input_recovery::{KEY_SLOTS, ReleaseLedger};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::{path::PathBuf, sync::OnceLock};

fn ledger_path() -> Result<&'static std::path::Path, String> {
    static PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    PATH.get_or_init(|| {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let mut boot = [0u8; 128];
        let mut size = boot.len();
        // Public sysctl boot UUID prevents stale key releases after a machine reboot.
        if unsafe {
            libc::sysctlbyname(
                c"kern.bootsessionuuid".as_ptr(),
                boot.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err("cannot determine input recovery boot session".into());
        }
        let boot = std::str::from_utf8(&boot[..size.min(boot.len())])
            .map_err(|e| e.to_string())?
            .trim_end_matches('\0');
        if boot.is_empty() || !boot.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
            return Err("invalid boot session UUID".into());
        }
        let mut session = 0;
        let mut attributes = 0;
        if unsafe { super::SessionGetInfo(0xffff_ffff, &mut session, &mut attributes) } != 0 {
            return Err("cannot determine input recovery login session".into());
        }
        let uid = unsafe { libc::geteuid() };
        let root = PathBuf::from(format!("/private/var/tmp/pab-input-{uid}-{boot}-{session}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let meta = root.symlink_metadata().map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err("unsafe input recovery directory".into());
        }
        Ok(root.join("held"))
    })
    .as_ref()
    .map(|p| p.as_path())
    .map_err(Clone::clone)
}

fn new_engine() -> Result<Enigo, String> {
    Enigo::new(&Settings {
        open_prompt_to_get_permissions: false,
        // This wrapper owns cleanup and keeps failed releases in the journal.
        release_keys_when_dropped: false,
        ..Settings::default()
    })
    .map_err(|e| e.to_string())
}

fn release_slot(engine: &mut Enigo, slot: usize) -> Result<(), String> {
    require_input_session()?;
    if slot < KEY_SLOTS {
        engine
            .raw(slot as u16, Direction::Release)
            .map_err(|e| e.to_string())
    } else {
        engine
            .button(
                match slot - KEY_SLOTS {
                    0 => Button::Left,
                    1 => Button::Right,
                    2 => Button::Middle,
                    _ => return Err("invalid mouse recovery slot".into()),
                },
                Direction::Release,
            )
            .map_err(|e| e.to_string())
    }
}

fn require_input_session() -> Result<(), String> {
    super::require_accessibility()?;
    if !super::active_console() {
        return Err("input recovery requires the active console session".into());
    }
    Ok(())
}

pub(crate) fn recover_abandoned() -> Result<(), String> {
    if !super::active_console() || !super::accessibility_allowed() {
        return Ok(());
    }
    let Some(mut ledger) = ReleaseLedger::acquire(ledger_path()?).map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    if ledger.empty() {
        return Ok(());
    }
    let mut engine = new_engine()?;
    ledger.recover(|slot| release_slot(&mut engine, slot))
}

/// No keyboard/mouse-down may bypass this wrapper. Main AppKit thread only.
pub(crate) struct RecoveryInput {
    engine: Enigo,
    ledger: Option<ReleaseLedger>,
}
impl RecoveryInput {
    pub(crate) fn new() -> Result<Self, String> {
        Ok(Self {
            engine: new_engine()?,
            ledger: None,
        })
    }

    fn prepare(&mut self) -> Result<(), String> {
        require_input_session()?;
        if self.ledger.is_none() {
            let mut ledger = ReleaseLedger::acquire(ledger_path()?)
                .map_err(|e| e.to_string())?
                .ok_or("another Bridge input sequence still holds keys")?;
            ledger.recover(|slot| release_slot(&mut self.engine, slot))?;
            self.ledger = Some(ledger);
        }
        Ok(())
    }

    fn send(
        &mut self,
        slot: usize,
        direction: Direction,
        mut send: impl FnMut(&mut Enigo, Direction) -> Result<(), String>,
    ) -> Result<(), String> {
        if direction == Direction::Click {
            return Err("use an explicit press/release pair for recoverable input".into());
        }
        if direction == Direction::Press {
            self.prepare()?;
        }
        let Some(ledger) = &mut self.ledger else {
            return Ok(());
        };
        let result = if direction == Direction::Press {
            ledger.press(slot, || send(&mut self.engine, Direction::Press))
        } else {
            ledger.release(slot, || send(&mut self.engine, Direction::Release))
        };
        if ledger.empty() {
            self.ledger = None;
        }
        result
    }

    pub(crate) fn key(&mut self, key: Key, direction: Direction) -> Result<(), String> {
        let code = u16::try_from(key).map_err(|_| "unsupported key for input recovery")?;
        if usize::from(code) >= KEY_SLOTS {
            return Err("unsupported key for input recovery".into());
        }
        self.send(usize::from(code), direction, |e, d| {
            require_input_session()?;
            // Use the exact journaled native code even if the keyboard layout
            // changes between translation and dispatch.
            e.raw(code, d).map_err(|e| e.to_string())
        })
    }

    pub(crate) fn text(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        self.prepare()?;
        let ledger = self.ledger.as_mut().unwrap();
        // Enigo 0.5 macOS fast_text posts Unicode using keycode 0 and may
        // synthesize Tab (48). Do not persist text or replay an interrupted call.
        ledger.press(0, || Ok(()))?;
        if text.contains('\t') {
            ledger.press(48, || Ok(()))?;
        }
        let result = self.engine.text(text).map_err(|e| e.to_string());
        let cleanup = ledger.recover(|slot| release_slot(&mut self.engine, slot));
        if ledger.empty() {
            self.ledger = None;
        }
        match (result, cleanup) {
            (Err(error), Err(cleanup)) => Err(format!("{error}; input release failed: {cleanup}")),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(format!("input release failed: {error}")),
            _ => Ok(()),
        }
    }

    pub(crate) fn button(&mut self, button: Button, direction: Direction) -> Result<(), String> {
        let slot = KEY_SLOTS
            + match button {
                Button::Left => 0,
                Button::Right => 1,
                Button::Middle => 2,
                _ => return Err("unsupported mouse button for input recovery".into()),
            };
        self.send(slot, direction, |e, d| {
            require_input_session()?;
            e.button(button, d).map_err(|e| e.to_string())
        })
    }
    pub(crate) fn move_mouse(
        &mut self,
        x: i32,
        y: i32,
        coordinate: Coordinate,
    ) -> enigo::InputResult<()> {
        self.engine.move_mouse(x, y, coordinate)
    }
    pub(crate) fn scroll(&mut self, length: i32, axis: Axis) -> enigo::InputResult<()> {
        self.engine.scroll(length, axis)
    }
}
impl Drop for RecoveryInput {
    fn drop(&mut self) {
        if let Some(ledger) = &mut self.ledger {
            let _ = ledger.recover(|slot| release_slot(&mut self.engine, slot));
        }
    }
}
