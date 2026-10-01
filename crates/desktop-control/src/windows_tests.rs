//! Native acceptance touches only a window created and owned by this test.
use super::*;
use std::{sync::mpsc, thread, time::Duration};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
};
const CLEANUP: u32 = WM_APP + 1;
const IGNORE_CLOSE: isize = 1;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn procedure(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: called only by Windows for our registered fixture window.
    unsafe {
        match msg {
            WM_CLOSE if GetWindowLongPtrW(w, GWLP_USERDATA) == IGNORE_CLOSE => 0,
            CLEANUP => {
                DestroyWindow(w);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_SETFOCUS => {
                let child = GetWindow(w, GW_CHILD);
                if !child.is_null() {
                    SetFocus(child);
                }
                0
            }
            _ => DefWindowProcW(w, msg, wp, lp),
        }
    }
}
struct Fixture {
    id: u32,
    edit: u32,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(ignore_close: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let name = wide(&format!("PAB_owned_fixture_{}", RequestId::new()));
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..std::mem::zeroed()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let w = CreateWindowExW(
                0,
                name.as_ptr(),
                wide("PAB D2 owned test window").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                50,
                50,
                400,
                200,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!w.is_null());
            if ignore_close {
                SetWindowLongPtrW(w, GWLP_USERDATA, IGNORE_CLOSE);
            }
            let edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide("").as_ptr(),
                WS_CHILD | WS_VISIBLE | ES_MULTILINE as u32,
                10,
                10,
                300,
                100,
                w,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!edit.is_null());
            SetFocus(edit);
            tx.send((w as usize as u32, edit as usize as u32)).unwrap();
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            UnregisterClassW(name.as_ptr(), instance);
        });
        let (id, edit) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            id,
            edit,
            worker: Some(worker),
        }
    }
    fn text(&self) -> String {
        unsafe {
            let mut text = [0u16; 1024];
            let n = GetWindowTextW(
                self.edit as usize as HWND,
                text.as_mut_ptr(),
                text.len() as i32,
            );
            String::from_utf16(&text[..n as usize]).unwrap()
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.id as usize as HWND, CLEANUP, 0, 0);
        }
        if let Some(w) = self.worker.take() {
            w.join().unwrap();
        }
    }
}
fn run(session: &mut DesktopSession, q: DesktopQuery) -> SystemQueryReply {
    session.query(RequestId::new(), &q)
}
#[test]
#[ignore = "explicit native desktop acceptance: creates/focuses only owned test windows"]
fn native_window_lifecycle_unicode_and_stale_references() {
    let fixture = Fixture::new(false);
    let mut session = DesktopSession::new();
    // xcap deliberately excludes its own process on Windows to avoid
    // synchronous GetWindowText deadlocks. Our mutation fixture is registered
    // directly, so tests never attach markers to the user's other windows.
    assert!(
        xcap::Window::all()
            .unwrap()
            .iter()
            .all(|w| w.id().ok() != Some(fixture.id))
    );
    let monitors = run(&mut session, DesktopQuery::Monitors {});
    assert_eq!(monitors.state, "completed", "{monitors:?}");
    let reference = session.reference(fixture.id, std::process::id()).unwrap();
    assert_eq!(
        reference,
        session.reference(fixture.id, std::process::id()).unwrap()
    );
    for control in [
        WindowControlAction::Minimize,
        WindowControlAction::Maximize,
        WindowControlAction::Restore,
    ] {
        let r = run(
            &mut session,
            DesktopQuery::Control {
                window_ref: reference.clone(),
                control,
            },
        );
        assert_eq!(r.state, "completed", "{r:?}");
    }
    let r = run(
        &mut session,
        DesktopQuery::Focus {
            window_ref: reference.clone(),
        },
    );
    let can_focus = r.state == "completed";
    if !can_focus {
        assert!(
            r.error.as_deref().unwrap().contains("foreground policy"),
            "{r:?}"
        );
        eprintln!(
            "UNICODE_ACCEPTANCE_PENDING: Windows denied foreground; input guard is tested, successful Unicode delivery requires interactive acceptance"
        );
    }
    let text = "Pixels 中文🙂";
    let r = run(
        &mut session,
        DesktopQuery::TypeText {
            window_ref: reference.clone(),
            text: text.into(),
        },
    );
    if can_focus {
        assert_eq!(r.state, "completed", "{r:?}");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while fixture.text() != text {
            assert!(
                std::time::Instant::now() < deadline,
                "input content mismatch: {}",
                fixture.text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    } else {
        assert_eq!(r.state, "failed", "{r:?}");
        let Some(SystemQueryData::Desktop { snapshot }) = r.data else {
            panic!("missing snapshot")
        };
        assert!(!snapshot.action_started);
        assert!(fixture.text().is_empty());
    }
    let expected_text = fixture.text();
    let mut other = DesktopSession::new();
    assert_eq!(
        run(
            &mut other,
            DesktopQuery::Focus {
                window_ref: reference.clone()
            }
        )
        .state,
        "failed"
    );
    let marker = session.windows.get(&reference).unwrap().marker;
    native::unmark(fixture.id, std::process::id(), &session.key, marker);
    assert_eq!(
        run(
            &mut session,
            DesktopQuery::TypeText {
                window_ref: reference.clone(),
                text: "must not type".into()
            }
        )
        .state,
        "failed"
    );
    assert_eq!(fixture.text(), expected_text);
    let fresh = session.reference(fixture.id, std::process::id()).unwrap();
    assert_ne!(fresh, reference);
    let r = run(
        &mut session,
        DesktopQuery::Control {
            window_ref: fresh.clone(),
            control: WindowControlAction::Close,
        },
    );
    assert_eq!(r.state, "completed", "{r:?}");
    assert_eq!(
        run(&mut session, DesktopQuery::Focus { window_ref: fresh }).state,
        "failed"
    );
    let refusing = Fixture::new(true);
    let reference = session.reference(refusing.id, std::process::id()).unwrap();
    let r = run(
        &mut session,
        DesktopQuery::Control {
            window_ref: reference,
            control: WindowControlAction::Close,
        },
    );
    assert_eq!(r.state, "failed");
    let Some(SystemQueryData::Desktop { snapshot }) = r.data else {
        panic!("missing snapshot")
    };
    assert!(snapshot.action_started);
    assert!(snapshot.verification.unwrap().starts_with("unconfirmed"));
    assert_ne!(
        unsafe { IsWindow(refusing.id as usize as HWND) },
        0,
        "normal close must not kill refusing window"
    );
}
#[test]
fn malformed_query_does_not_start_native_action() {
    let mut session = DesktopSession::new();
    let r = run(
        &mut session,
        DesktopQuery::Focus {
            window_ref: "invalid".into(),
        },
    );
    assert_eq!(r.state, "failed");
    let Some(SystemQueryData::Desktop { snapshot }) = r.data else {
        panic!("missing snapshot")
    };
    assert!(!snapshot.action_started);
}
