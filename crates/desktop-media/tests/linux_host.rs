#![cfg(target_os = "linux")]
//! Opt-in graphical integration; creates only an isolated test window.
use pab_desktop_media::{LinuxNativeViewer, MainThreadDispatch};
use pab_protocol::*;
use pab_remote_desktop::VideoFrame;
use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn gtk_init_check(argc: *mut i32, argv: *mut *mut *mut u8) -> i32;
    fn gtk_window_new(kind: i32) -> *mut c_void;
    fn gtk_window_set_default_size(window: *mut c_void, width: i32, height: i32);
    fn gtk_label_new(text: *const u8) -> *mut c_void;
    fn gtk_container_add(parent: *mut c_void, child: *mut c_void);
    fn gtk_bin_get_child(parent: *mut c_void) -> *mut c_void;
    fn gtk_widget_show_all(widget: *mut c_void);
    fn gtk_widget_show(widget: *mut c_void);
    fn gtk_widget_hide(widget: *mut c_void);
    fn gtk_widget_destroy(widget: *mut c_void);
    fn g_object_ref_sink(object: *mut c_void) -> *mut c_void;
    fn g_object_unref(object: *mut c_void);
    fn g_main_context_iteration(context: *mut c_void, block: i32) -> i32;
    fn g_main_context_pending(context: *mut c_void) -> i32;
    fn g_idle_add_full(
        priority: i32,
        callback: extern "C" fn(*mut c_void) -> i32,
        data: *mut c_void,
        drop: extern "C" fn(*mut c_void),
    ) -> u32;
}
fn pump() {
    // SAFETY: this test exclusively owns GTK's default main context.
    unsafe {
        for _ in 0..16 {
            if g_main_context_pending(std::ptr::null_mut()) == 0 {
                break;
            }
            g_main_context_iteration(std::ptr::null_mut(), 0);
        }
    }
}
fn until(ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready() {
        pump();
        std::thread::sleep(Duration::from_millis(2));
        assert!(Instant::now() < deadline, "GTK host timed out");
    }
}
struct UiWork(Option<Box<dyn FnOnce() + Send>>);
extern "C" fn execute(data: *mut c_void) -> i32 {
    // SAFETY: idle source owns this box until destroy; callback runs once.
    let work = unsafe { &mut *data.cast::<UiWork>() }.0.take().unwrap();
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
        std::process::abort();
    }
    0
}
extern "C" fn destroy_work(data: *mut c_void) {
    // SAFETY: GLib invokes the destructor exactly once after callback removal.
    unsafe {
        drop(Box::from_raw(data.cast::<UiWork>()));
    }
}
fn dispatch() -> MainThreadDispatch {
    Arc::new(|work| {
        let data = Box::into_raw(Box::new(UiWork(Some(work)))).cast();
        // SAFETY: GLib allows attaching an idle source from a background thread.
        let id = unsafe { g_idle_add_full(0, execute, data, destroy_work) };
        assert_ne!(id, 0);
        Ok(())
    })
}
fn fixture() -> (DesktopVideoConfig, Vec<VideoFrame>) {
    let data = include_bytes!("fixtures/apple-low-latency.pabvideo");
    let number = |offset: usize| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let screen = DesktopScreen {
        id: "test".into(),
        name: "Generated pixels".into(),
        x: 0,
        y: 0,
        desktop_width: number(8),
        desktop_height: number(12),
        pixel_width: number(8),
        pixel_height: number(12),
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
    let mut offset = 24;
    let mut frames = Vec::new();
    for index in 0..number(20) {
        let size = number(offset) as usize;
        let key = number(offset + 20) != 0;
        offset += 24;
        frames.push(VideoFrame {
            header: DesktopFrameHeader {
                version: REMOTE_DESKTOP_SCHEMA_VERSION,
                session_id: config.session_id,
                generation: 1,
                frame_id: u64::from(index) + 1,
                capture_time_us: u64::from(index) * 66666,
                keyframe: key,
                payload_bytes: size as u32,
            },
            bytes: data[offset..offset + size].to_vec(),
        });
        offset += size;
    }
    (config, frames)
}
#[test]
#[ignore = "requires an authorized graphical session; uses generated pixels only"]
fn gtk_host_decode_present_cancel_and_restore() {
    // SAFETY: all GTK calls stay on this single test thread; dispatched work is
    // drained here. Background tasks receive only the public viewer API.
    unsafe {
        assert_ne!(
            gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()),
            0
        );
        let window = gtk_window_new(0);
        g_object_ref_sink(window);
        gtk_window_set_default_size(window, 960, 600);
        let root = gtk_label_new(c"Pixels Rust host test".as_ptr().cast());
        gtk_container_add(window, root);
        gtk_widget_show_all(window);
        pump();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let entered = runtime.enter();
        let (config, frames) = fixture();
        let count = frames.len();
        let (mut viewer, mut input) =
            LinuxNativeViewer::create_interactive(window, &config, dispatch()).unwrap();
        let mut presentations = viewer.presentations();
        let task = runtime.spawn(async move {
            input.set_enabled(true).await.unwrap();
            input.set_enabled(false).await.unwrap();
            for frame in frames {
                viewer.submit(&frame).await.unwrap();
                assert_eq!(
                    presentations.next().await.unwrap().frame_id,
                    frame.header.frame_id
                );
            }
            drop(viewer);
        });
        until(|| task.is_finished());
        runtime.block_on(task).unwrap();
        until(|| gtk_bin_get_child(window) == root);
        // Retire while input enable is queued on GTK. The waiter must fail
        // without needing the queued callback to run on a closing window.
        let (viewer, mut input) =
            LinuxNativeViewer::create_interactive(window, &config, dispatch()).unwrap();
        let mut enabling = std::pin::pin!(input.set_enabled(true));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(enabling.as_mut(), &mut context).is_pending());
        drop(viewer);
        assert!(
            runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(2), enabling)
                        .await
                        .unwrap()
                })
                .is_err()
        );
        until(|| gtk_bin_get_child(window) == root);
        let (_, frames) = fixture();
        // A malformed generation is rejected before it enters the renderer.
        let mut viewer = LinuxNativeViewer::create(window, &config, dispatch()).unwrap();
        let mut frame = VideoFrame {
            header: frames[0].header.clone(),
            bytes: frames[0].bytes.clone(),
        };
        frame.header.session_id = config.session_id;
        frame.header.generation = 2;
        assert!(runtime.block_on(viewer.submit(&frame)).is_err());
        frame.header.generation = 1;
        let mut presentations = viewer.presentations();
        gtk_widget_hide(window);
        pump();
        let consumed = Arc::new(AtomicBool::new(false));
        let signal = consumed.clone();
        let task = runtime.spawn(async move {
            viewer.submit(&frame).await.unwrap();
            signal.store(true, Ordering::Release);
            assert_eq!(presentations.next().await.unwrap().frame_id, 1);
            drop(viewer);
        });
        until(|| consumed.load(Ordering::Acquire));
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            pump();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!task.is_finished(), "hidden GTK frame was acknowledged");
        gtk_widget_show(window);
        until(|| task.is_finished());
        runtime.block_on(task).unwrap();
        until(|| gtk_bin_get_child(window) == root);
        // Cancel a consumed frame before queued GTK work can run. Dropping must
        // wake the serial decoder and retire queued work without touching GTK.
        let mut viewer = LinuxNativeViewer::create(window, &config, dispatch()).unwrap();
        let mut frame = VideoFrame {
            header: frames[0].header.clone(),
            bytes: frames[0].bytes.clone(),
        };
        frame.header.session_id = config.session_id;
        let task = runtime.spawn(async move {
            viewer.submit(&frame).await.unwrap();
            drop(viewer);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !task.is_finished() {
            std::thread::sleep(Duration::from_millis(2));
            assert!(Instant::now() < deadline);
        }
        runtime.block_on(task).unwrap();
        until(|| gtk_bin_get_child(window) == root);
        let mut viewer = LinuxNativeViewer::create(window, &config, dispatch()).unwrap();
        let mut frame = VideoFrame {
            header: frames[0].header.clone(),
            bytes: frames[0].bytes.clone(),
        };
        frame.header.session_id = config.session_id;
        frame.bytes = b"invalid-h264".to_vec();
        frame.header.payload_bytes = frame.bytes.len() as u32;
        let mut presentations = viewer.presentations();
        let task = runtime.spawn(async move {
            assert!(viewer.submit(&frame).await.is_err());
            assert!(presentations.next().await.is_err());
            drop(viewer);
        });
        until(|| task.is_finished());
        runtime.block_on(task).unwrap();
        until(|| gtk_bin_get_child(window) == root);
        let mut viewer = LinuxNativeViewer::create(window, &config, dispatch()).unwrap();
        let mut frame = VideoFrame {
            header: frames[0].header.clone(),
            bytes: frames[0].bytes.clone(),
        };
        frame.header.session_id = config.session_id;
        let mut presentations = viewer.presentations();
        let consumed = Arc::new(AtomicBool::new(false));
        let signal = consumed.clone();
        gtk_widget_hide(window);
        pump();
        let task = runtime.spawn(async move {
            viewer.submit(&frame).await.unwrap();
            signal.store(true, Ordering::Release);
            assert!(presentations.next().await.is_err());
            drop(viewer);
        });
        until(|| consumed.load(Ordering::Acquire));
        gtk_widget_destroy(window);
        until(|| task.is_finished());
        runtime.block_on(task).unwrap();
        pump();
        g_object_unref(window);
        pump();
        drop(entered);
        // A leaked blocking decoder would prevent normal runtime shutdown.
        drop(runtime);
        println!(
            "Rust GTK host: {count} generated frames, input enable/disable acknowledged, retired input mode waiter failed, stale generation rejected, hidden ACK withheld, restore, queued cancellation, decoder failure, original child restored, host close with pending frame, runtime stopped"
        );
    }
}
