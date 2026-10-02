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
#[ignore = "explicit native batch acceptance: controls only its own fixture window"]
fn native_batch_unicode_shortcut_and_partial_stop() {
    let fixture = Fixture::new(false);
    let mut session = DesktopSession::new();
    let reference = session.reference(fixture.id, std::process::id()).unwrap();
    let batch = DesktopQuery::Batch {
        window_ref: reference.clone(),
        timeout_ms: 5000,
        actions: vec![
            DesktopAction::Focus {},
            DesktopAction::TypeText {
                text: "Original".into(),
            },
            DesktopAction::Wait { ms: 50 },
            DesktopAction::KeyChord {
                modifiers: vec![DesktopModifier::Control],
                key: "home".into(),
            },
            // Classic EDIT supports Ctrl+Home/Ctrl+Shift+End, but does not implement Ctrl+A.
            DesktopAction::KeyChord {
                modifiers: vec![DesktopModifier::Control, DesktopModifier::Shift],
                key: "end".into(),
            },
            DesktopAction::TypeText {
                text: "批量中文🙂".into(),
            },
        ],
    };
    let result = run(&mut session, batch);
    assert_eq!(result.state, "completed", "{result:?}");
    let Some(SystemQueryData::Desktop { snapshot }) = result.data else {
        panic!()
    };
    assert_eq!(snapshot.batch.unwrap().completed_steps, 6);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while fixture.text() != "批量中文🙂" {
        assert!(
            std::time::Instant::now() < deadline,
            "unexpected text {:?}",
            fixture.text()
        );
        thread::sleep(Duration::from_millis(10));
    }
    let batch = DesktopQuery::Batch {
        window_ref: reference,
        timeout_ms: 5000,
        actions: vec![
            DesktopAction::Control {
                control: WindowControlAction::Minimize,
            },
            DesktopAction::TypeText {
                text: "must-not-type".into(),
            },
            DesktopAction::Control {
                control: WindowControlAction::Restore,
            },
        ],
    };
    let result = run(&mut session, batch);
    assert_eq!(result.state, "failed", "{result:?}");
    let Some(SystemQueryData::Desktop { snapshot }) = result.data else {
        panic!()
    };
    let report = snapshot.batch.unwrap();
    assert_eq!(report.completed_steps, 1);
    assert_eq!(report.failed_step, Some(1));
    assert_eq!(report.steps[2].state, "skipped");
    assert_eq!(fixture.text(), "批量中文🙂");
    assert_ne!(unsafe { IsIconic(fixture.id as usize as HWND) }, 0);
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

#[test]
#[ignore = "fixture child entry; launched only by native capture acceptance"]
fn owned_fixture_child() {
    let ready = std::env::var_os("PAB_D3_FIXTURE_READY").expect("parent fixture path required");
    let fixture = Fixture::new(false);
    std::fs::write(
        ready,
        serde_json::to_vec(
            &serde_json::json!({"pid":std::process::id(),"id":fixture.id,"edit":fixture.edit}),
        )
        .unwrap(),
    )
    .unwrap();
    while unsafe { IsWindow(fixture.id as usize as HWND) } != 0 {
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
#[ignore = "explicit native acceptance: only captures/controls an owned fixture child window"]
fn native_referenced_window_preview_and_cross_process_unicode() {
    use std::process::{Command, Stdio};
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let ready = std::env::temp_dir().join(format!("pab-d3-fixture-{}.json", RequestId::new()));
    let _child = OwnedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_tests::owned_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .env("PAB_D3_FIXTURE_READY", &ready)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    let _ = std::fs::remove_file(&ready);
    let id = data["id"].as_u64().unwrap() as u32;
    let pid = data["pid"].as_u64().unwrap() as u32;
    let mut session = DesktopSession::new();
    let reference = session.reference(id, pid).unwrap();
    let options = ScreenshotOptions {
        window_ref: Some(reference.clone()),
        ..Default::default()
    };
    let image = session.capture_window(&options).unwrap();
    assert_eq!(image.info.window_ref, Some(reference.clone()));
    assert!(image.info.desktop_rect.is_some());
    assert_eq!(image.info.format, ScreenshotFormat::Jpeg);
    assert_eq!(image.info.quality, Some(85));
    assert!(!image.info.resized);
    assert_eq!(
        (image.info.width, image.info.height),
        (image.info.source_width, image.info.source_height)
    );
    assert_eq!(image.info.format, ScreenshotFormat::Jpeg);
    pab_screenshot::verify(&image.bytes, &image.info, &options).unwrap();
    assert!(DesktopSession::new().capture_window(&options).is_err());
    let focus = run(
        &mut session,
        DesktopQuery::Focus {
            window_ref: reference.clone(),
        },
    );
    if focus.state == "completed" {
        let r = run(
            &mut session,
            DesktopQuery::TypeText {
                window_ref: reference.clone(),
                text: "Pixels 中文🙂".into(),
            },
        );
        assert_eq!(r.state, "completed", "{r:?}");
        let edit = data["edit"].as_u64().unwrap() as usize as HWND;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let mut text = [0u16; 1024];
            let mut n = 0usize;
            assert_ne!(
                unsafe {
                    SendMessageTimeoutW(
                        edit,
                        WM_GETTEXT,
                        text.len(),
                        text.as_mut_ptr() as isize,
                        SMTO_ABORTIFHUNG,
                        1000,
                        &mut n,
                    )
                },
                0
            );
            if String::from_utf16(&text[..n]).ok().as_deref() == Some("Pixels 中文🙂") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "unexpected fixture UTF-16: {:?}",
                &text[..n]
            );
            thread::sleep(Duration::from_millis(10));
        }
        eprintln!("UNICODE_DELIVERY_VERIFIED: owned cross-process EDIT received Chinese and emoji");
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { GetWindowRect(edit, &mut rect) }, 0);
        let origin = image.info.desktop_rect.as_ref().unwrap();
        let click = run(
            &mut session,
            DesktopQuery::Batch {
                window_ref: reference.clone(),
                timeout_ms: 5000,
                actions: vec![
                    DesktopAction::Click {
                        x: u16::try_from(rect.left + 5 - origin.x).unwrap(),
                        y: u16::try_from(rect.top + 5 - origin.y).unwrap(),
                        button: DesktopMouseButton::Left,
                    },
                    DesktopAction::Scroll {
                        axis: DesktopScrollAxis::Vertical,
                        amount: 1,
                    },
                ],
            },
        );
        assert_eq!(click.state, "completed", "{click:?}");
        let Some(SystemQueryData::Desktop { snapshot }) = click.data else {
            panic!()
        };
        assert_eq!(snapshot.batch.unwrap().completed_steps, 2);
        eprintln!(
            "BATCH_CLICK_SCROLL_VERIFIED: owned cross-process window accepted click and scroll"
        );
    } else {
        assert!(
            focus
                .error
                .as_deref()
                .unwrap()
                .contains("foreground policy")
        );
        eprintln!("UNICODE_ACCEPTANCE_PENDING: Windows foreground policy refused focus");
    }
    let r = run(
        &mut session,
        DesktopQuery::Control {
            window_ref: reference.clone(),
            control: WindowControlAction::Minimize,
        },
    );
    assert_eq!(r.state, "completed");
    assert!(session.capture_window(&options).is_err());
    let r = run(
        &mut session,
        DesktopQuery::Control {
            window_ref: reference.clone(),
            control: WindowControlAction::Restore,
        },
    );
    assert_eq!(r.state, "completed");
    let marker = session.windows.get(&reference).unwrap().marker;
    native::unmark(id, pid, &session.key, marker);
    assert!(session.capture_window(&options).is_err());
    let fresh = session.reference(id, pid).unwrap();
    let r = run(
        &mut session,
        DesktopQuery::Control {
            window_ref: fresh,
            control: WindowControlAction::Close,
        },
    );
    assert_eq!(r.state, "completed");
    assert!(session.capture_window(&options).is_err());
}
