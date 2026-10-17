//! Opt-in AppKit integration using generated fixture pixels only. Run from an
//! authorized graphical session. No screen capture, system input or TCC edits.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_host_check requires macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    check::run();
}

#[cfg(target_os = "macos")]
mod check {
    use pab_desktop_media::{MacNativeViewer, MainThreadDispatch};
    use pab_protocol::*;
    use pab_remote_desktop::VideoFrame;
    use std::{
        ffi::{CStr, c_char, c_void},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };
    type Obj = *mut c_void;
    type Work = Box<dyn FnOnce() + Send>;
    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Obj;
        fn sel_registerName(name: *const c_char) -> Obj;
        fn objc_msgSend();
    }
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: Obj;
        fn CFRunLoopRunInMode(mode: Obj, seconds: f64, once: bool) -> i32;
    }
    unsafe fn sel(name: &CStr) -> Obj {
        unsafe { sel_registerName(name.as_ptr()) }
    }
    unsafe fn object(receiver: Obj, name: &CStr) -> Obj {
        let call: unsafe extern "C" fn(Obj, Obj) -> Obj =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { call(receiver, sel(name)) }
    }
    unsafe fn argument(receiver: Obj, name: &CStr, value: Obj) {
        let call: unsafe extern "C" fn(Obj, Obj, Obj) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { call(receiver, sel(name), value) }
    }
    unsafe fn boolean(receiver: Obj, name: &CStr, value: bool) {
        let call: unsafe extern "C" fn(Obj, Obj, bool) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { call(receiver, sel(name), value) }
    }
    unsafe fn action(receiver: Obj, name: &CStr) {
        let call: unsafe extern "C" fn(Obj, Obj) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { call(receiver, sel(name)) }
    }
    struct Ui {
        app: Obj,
        work: mpsc::Receiver<Work>,
        dispatch: MainThreadDispatch,
        mode: Obj,
        past: Obj,
    }
    impl Ui {
        fn pump(&self) {
            // SAFETY: all AppKit messages target owned live objects on main;
            // signatures below match the named Objective-C methods exactly.
            unsafe {
                for work in self.work.try_iter().take(64) {
                    work();
                }
                let next: unsafe extern "C" fn(Obj, Obj, u64, Obj, Obj, bool) -> Obj =
                    std::mem::transmute(objc_msgSend as *const ());
                for _ in 0..32 {
                    let event = next(
                        self.app,
                        sel(c"nextEventMatchingMask:untilDate:inMode:dequeue:"),
                        u64::MAX,
                        self.past,
                        self.mode,
                        true,
                    );
                    if event.is_null() {
                        break;
                    }
                    argument(self.app, c"sendEvent:", event);
                }
                action(self.app, c"updateWindows");
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.002, true);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        fn until(&self, ready: impl Fn() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready() {
                self.pump();
                assert!(Instant::now() < deadline, "AppKit host timed out");
            }
        }
    }
    fn fixture(odd: bool) -> (DesktopVideoConfig, Vec<VideoFrame>) {
        let data = include_bytes!("../tests/fixtures/apple-low-latency.pabvideo");
        let number = |p: usize| u32::from_le_bytes(data[p..p + 4].try_into().unwrap());
        let (width, height) = if odd {
            (1297, 753)
        } else {
            (number(8), number(12))
        };
        let screen = DesktopScreen {
            id: "fixture".into(),
            name: "Generated pixels".into(),
            x: 0,
            y: 0,
            desktop_width: width,
            desktop_height: height,
            pixel_width: width,
            pixel_height: height,
            scale_milli: 1000,
            rotation: DesktopRotation::Identity,
            primary: true,
        };
        let config = DesktopVideoConfig {
            session_id: RequestId::new(),
            generation: 1,
            screen,
            codec: DesktopCodec::H264AnnexB,
            color: DesktopColor::Bt709Limited,
            fps: 15,
        };
        if odd {
            let (w, h) = (width as usize, height as usize);
            let mut encoder = pab_desktop_media::software::SoftwareEncoder::new(w, h, 15).unwrap();
            let mut pixels = vec![255u8; w * h * 4];
            let mut frames = Vec::new();
            for index in 0..8 {
                for y in 1..h - 1 {
                    for x in 1..w - 1 {
                        let value = 40 + index * 12;
                        pixels[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(value);
                    }
                }
                let bytes = encoder.encode_bgra(&pixels, w * 4).unwrap().to_vec();
                frames.push(VideoFrame {
                    header: DesktopFrameHeader {
                        version: REMOTE_DESKTOP_SCHEMA_VERSION,
                        session_id: config.session_id,
                        generation: 1,
                        frame_id: u64::from(index) + 1,
                        capture_time_us: u64::from(index) * 66666,
                        keyframe: index == 0,
                        payload_bytes: bytes.len() as u32,
                    },
                    bytes,
                });
            }
            return (config, frames);
        }
        let mut offset = 24;
        let mut frames = Vec::new();
        for index in 0..number(20) {
            let size = number(offset) as usize;
            let keyframe = number(offset + 20) != 0;
            offset += 24;
            frames.push(VideoFrame {
                header: DesktopFrameHeader {
                    version: REMOTE_DESKTOP_SCHEMA_VERSION,
                    session_id: config.session_id,
                    generation: 1,
                    frame_id: u64::from(index) + 1,
                    capture_time_us: u64::from(index) * 66666,
                    keyframe,
                    payload_bytes: size as u32,
                },
                bytes: data[offset..offset + size].to_vec(),
            });
            offset += size;
        }
        (config, frames)
    }
    pub fn run() {
        // SAFETY: this executable's main thread exclusively owns the AppKit
        // window. Background jobs communicate only through the dispatch queue.
        unsafe {
            let pool = object(
                object(objc_getClass(c"NSAutoreleasePool".as_ptr()), c"alloc"),
                c"init",
            );
            let app = object(
                objc_getClass(c"NSApplication".as_ptr()),
                c"sharedApplication",
            );
            let policy: unsafe extern "C" fn(Obj, Obj, isize) -> bool =
                std::mem::transmute(objc_msgSend as *const ());
            assert!(policy(app, sel(c"setActivationPolicy:"), 1));
            action(app, c"finishLaunching");
            let init: unsafe extern "C" fn(Obj, Obj, Rect, usize, usize, bool) -> Obj =
                std::mem::transmute(objc_msgSend as *const ());
            let window = init(
                object(objc_getClass(c"NSWindow".as_ptr()), c"alloc"),
                sel(c"initWithContentRect:styleMask:backing:defer:"),
                Rect {
                    origin: Point { x: 100., y: 100. },
                    size: Size {
                        width: 960.,
                        height: 600.,
                    },
                },
                15,
                2,
                false,
            );
            assert!(!window.is_null());
            boolean(window, c"setReleasedWhenClosed:", false);
            argument(window, c"makeKeyAndOrderFront:", std::ptr::null_mut());
            boolean(app, c"activateIgnoringOtherApps:", true);
            let string: unsafe extern "C" fn(Obj, Obj, *const c_char) -> Obj =
                std::mem::transmute(objc_msgSend as *const ());
            let mode = string(
                objc_getClass(c"NSString".as_ptr()),
                sel(c"stringWithUTF8String:"),
                c"kCFRunLoopDefaultMode".as_ptr(),
            );
            let (send, receive) = mpsc::channel::<Work>();
            let ui = Ui {
                app,
                work: receive,
                dispatch: Arc::new(move |work| send.send(work).map_err(|e| e.to_string())),
                mode,
                past: object(objc_getClass(c"NSDate".as_ptr()), c"distantPast"),
            };
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let entered = runtime.enter();
            for (software, odd) in [(false, false), (true, false), (false, true), (true, true)] {
                let (config, frames) = fixture(odd);
                let count = frames.len();
                let (mut viewer, _input) =
                    MacNativeViewer::create(window, &config, software, ui.dispatch.clone())
                        .unwrap();
                let mut presented = viewer.presentations();
                let task = runtime.spawn(async move {
                    for frame in frames {
                        viewer.submit(&frame).await.unwrap();
                        assert_eq!(
                            presented.next().await.unwrap().frame_id,
                            frame.header.frame_id
                        );
                    }
                    drop(viewer);
                });
                ui.until(|| task.is_finished());
                runtime.block_on(task).unwrap();
                ui.pump();
                let (config, mut frames) = fixture(odd);
                let frame = frames.remove(0);
                argument(window, c"orderOut:", std::ptr::null_mut());
                ui.pump();
                let (mut viewer, _input) =
                    MacNativeViewer::create(window, &config, software, ui.dispatch.clone())
                        .unwrap();
                let mut presented = viewer.presentations();
                let checked = Arc::new(AtomicBool::new(false));
                let signal = checked.clone();
                let task = runtime.spawn(async move {
                    viewer.submit(&frame).await.unwrap();
                    assert!(
                        tokio::time::timeout(Duration::from_millis(250), presented.next())
                            .await
                            .is_err(),
                        "hidden frame was acknowledged"
                    );
                    signal.store(true, Ordering::Release);
                    assert_eq!(presented.next().await.unwrap().frame_id, 1);
                    drop(viewer);
                });
                ui.until(|| checked.load(Ordering::Acquire));
                argument(window, c"makeKeyAndOrderFront:", std::ptr::null_mut());
                boolean(app, c"activateIgnoringOtherApps:", true);
                ui.until(|| task.is_finished());
                runtime.block_on(task).unwrap();
                ui.pump();
                // Cancellation must wake a decoder waiting for a hidden frame;
                // clean runtime shutdown below detects a stranded blocking job.
                argument(window, c"orderOut:", std::ptr::null_mut());
                ui.pump();
                let (config, mut frames) = fixture(odd);
                let frame = frames.remove(0);
                let (mut viewer, _input) =
                    MacNativeViewer::create(window, &config, software, ui.dispatch.clone())
                        .unwrap();
                let mut presented = viewer.presentations();
                let task = runtime.spawn(async move {
                    viewer.submit(&frame).await.unwrap();
                    assert!(
                        tokio::time::timeout(Duration::from_millis(100), presented.next())
                            .await
                            .is_err()
                    );
                    drop(viewer);
                });
                ui.until(|| task.is_finished());
                runtime.block_on(task).unwrap();
                ui.pump();
                argument(window, c"makeKeyAndOrderFront:", std::ptr::null_mut());
                boolean(app, c"activateIgnoringOtherApps:", true);
                println!(
                    "AppKit host: software={software}, odd={odd}, frames={count}, hidden ACK withheld, static restore, pending-frame cancellation passed"
                );
            }
            drop(entered);
            drop(runtime);
            ui.pump();
            action(window, c"close");
            action(window, c"release");
            action(pool, c"drain");
        }
    }
}
