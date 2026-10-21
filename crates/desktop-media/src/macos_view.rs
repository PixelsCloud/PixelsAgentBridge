//! In-process AppKit view with serial background decoding. Only the host can
//! supply an NSWindow, on its main thread. Packets and callbacks stay bounded;
//! cancellation retires the entire generation, including queued UI work.
use crate::{NativePresentations, NativeViewerInput, renderer::Renderer, viewer_input::InputSink};
use pab_protocol::{DesktopRotation, DesktopVideoConfig};
use pab_remote_desktop::VideoFrame;
use std::{
    ffi::{CStr, c_char, c_void},
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, watch},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
pub type MainThreadDispatch =
    Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + Send + Sync>;
unsafe extern "C" {
    fn pab_mac_view_new(
        window: *mut c_void,
        width: u32,
        height: u32,
        rotation: u32,
        context: *mut c_void,
        callback: extern "C" fn(*mut c_void, *const u8) -> bool,
        drop: extern "C" fn(*mut c_void),
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn pab_mac_view_poll(
        view: *mut c_void,
        presented: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_mac_view_input(
        view: *mut c_void,
        enabled: bool,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_mac_view_render(
        view: *mut c_void,
        surface: *mut c_void,
        index: u64,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_mac_view_drop(view: *mut c_void);
    fn pab_mac_view_cursor(
        view: *mut c_void,
        bytes: *const u8,
        size: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_mac_decoder_new(
        width: u32,
        height: u32,
        fps: u32,
        software: bool,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn pab_mac_decoder_frame(
        decoder: *mut c_void,
        bytes: *const u8,
        size: usize,
        index: u64,
        software: *mut bool,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn pab_mac_decoder_drop(decoder: *mut c_void);
    fn pab_mac_surface_drop(surface: *mut c_void);
}
fn message(bytes: &[c_char; 512]) -> String {
    // SAFETY: zero initialized buffer; the native ABI always NUL-terminates.
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}
struct Surface(usize);
impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: owns one retained CVPixelBuffer; CF release is thread safe.
        unsafe { pab_mac_surface_drop(self.0 as *mut c_void) };
    }
}
struct Decoder(*mut c_void);
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: uniquely owned and dropped on the same serial decode thread.
        unsafe { pab_mac_decoder_drop(self.0) };
    }
}
struct State {
    view: AtomicUsize,
    stopped: AtomicBool,
    poll_pending: AtomicBool,
    displayed: AtomicU64,
    wake: Condvar,
    gate: Mutex<()>,
    dispatch: MainThreadDispatch,
    events: mpsc::Sender<Vec<u8>>,
    failure: watch::Sender<Option<String>>,
}
impl State {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }
    fn fail(&self, error: impl Into<String>) {
        self.stop();
        let mut error = Some(error.into());
        self.failure.send_if_modified(|current| {
            if current.is_some() {
                false
            } else {
                *current = error.take();
                true
            }
        });
    }
    fn report(&self, bytes: Vec<u8>) {
        if self.events.try_send(bytes).is_err() {
            self.fail("native presentation queue ended or overflowed");
        }
    }
    fn event(&self, kind: u8, index: u64) {
        let mut bytes = vec![kind];
        bytes.extend_from_slice(&index.to_le_bytes());
        self.report(bytes);
    }
    fn schedule(self: &Arc<Self>, run: impl FnOnce(&Self) + Send + 'static) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(io::Error::other("native view is closed"));
        }
        let state = self.clone();
        (self.dispatch)(Box::new(move || {
            if !state.stopped.load(Ordering::Acquire) {
                run(&state);
            }
        }))
        .map_err(io::Error::other)
    }
    fn pointer(&self) -> *mut c_void {
        self.view.load(Ordering::Acquire) as *mut c_void
    }
    fn wait_presented(&self, index: u64) -> bool {
        let Ok(mut guard) = self.gate.lock() else {
            self.fail("native display gate failed");
            return false;
        };
        while !self.stopped.load(Ordering::Acquire)
            && self.displayed.load(Ordering::Acquire) <= index
        {
            let Ok((next, _)) = self.wake.wait_timeout(guard, Duration::from_millis(50)) else {
                self.fail("native display gate failed");
                return false;
            };
            guard = next;
        }
        !self.stopped.load(Ordering::Acquire)
    }
}
impl Drop for State {
    fn drop(&mut self) {
        let view = self.view.load(Ordering::Acquire);
        if view != 0 {
            // SAFETY: all earlier UI closures retained this State. Its final
            // drop schedules the unique native destructor on the host thread.
            let _ = (self.dispatch)(Box::new(move || unsafe {
                pab_mac_view_drop(view as *mut c_void)
            }));
        }
    }
}
extern "C" fn input(context: *mut c_void, bytes: *const u8) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || bytes.is_null() {
            return false;
        }
        // SAFETY: native sink owns the boxed Arc until its matching destructor;
        // every callback borrows exactly eight bytes for this synchronous call.
        let sink = unsafe { &*context.cast::<Arc<InputSink>>() };
        let packet = unsafe { *bytes.cast::<[u8; 8]>() };
        sink.emit(packet)
    }))
    .unwrap_or(false)
}
extern "C" fn drop_input(context: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: native sink takes this allocation once, including new failure.
        let sink = unsafe { Box::from_raw(context.cast::<Arc<InputSink>>()) };
        sink.fail();
    }));
}
pub struct MacNativeViewer {
    state: Arc<State>,
    config: DesktopVideoConfig,
    next_frame: u64,
    frames: mpsc::Sender<(u64, Vec<u8>)>,
    renderer: Renderer,
    relay: tokio::task::JoinHandle<()>,
    timer: tokio::task::JoinHandle<()>,
    decode: tokio::task::JoinHandle<()>,
}
/// Independent of video submission. A cancelled or failed submission retires
/// this handle; queued native work can only run within the owning generation.
pub struct MacCursorRenderer {
    state: Option<std::sync::Weak<State>>,
    config: DesktopVideoConfig,
    next_sequence: u64,
    image_id: u64,
    acknowledgements: crate::renderer::CursorAcknowledgements,
}
impl MacCursorRenderer {
    pub async fn overlay(&mut self, text: &str) -> Result<(), Error> {
        if text.len() > 2048 {
            return Err("viewer statistics too large".into());
        }
        self.send([&[2][..], text.as_bytes()].concat()).await
    }
    pub async fn submit(&mut self, update: &pab_remote_desktop::CursorUpdate) -> Result<(), Error> {
        let next = self
            .next_sequence
            .checked_add(1)
            .ok_or("cursor sequence exhausted")?;
        if update
            .header
            .validate(&self.config, self.next_sequence, self.image_id)?
            != update.bytes.len()
        {
            return Err("cursor payload length mismatch".into());
        }
        self.send(crate::cursor_packet::encode(&self.config, update))
            .await?;
        self.next_sequence = next;
        self.image_id = update.header.image_id;
        Ok(())
    }
    pub async fn clear(&mut self) -> Result<(), Error> {
        self.send(vec![0]).await
    }
    async fn send(&mut self, packet: Vec<u8>) -> Result<(), Error> {
        let weak = self.state.take().ok_or("cursor renderer ended")?;
        let state = weak.upgrade().ok_or("native viewer closed")?;
        let sequence = self.acknowledgements.prepare()?;
        state.schedule(move |state| {
            let mut error = [0; 512];
            // SAFETY: schedule retains the view on AppKit main; packet remains
            // alive for the synchronous upload. The native side retains no bytes.
            if unsafe {
                pab_mac_view_cursor(
                    state.pointer(),
                    packet.as_ptr(),
                    packet.len(),
                    error.as_mut_ptr(),
                    error.len(),
                )
            } {
                state.event(6, sequence);
            } else {
                state.fail(message(&error));
            }
        })?;
        drop(state);
        self.acknowledgements.consumed(sequence).await?;
        self.state = Some(weak);
        Ok(())
    }
}
impl MacNativeViewer {
    /// Create on the host main thread, within an entered Tokio runtime.
    ///
    /// # Safety
    /// `window` must be a live, host-owned NSWindow pointer. `dispatch` must
    /// execute closures exclusively on its AppKit main thread, including drop.
    /// Never accept this pointer from IPC, a webview or an external caller.
    pub unsafe fn create(
        window: *mut c_void,
        config: &DesktopVideoConfig,
        software: bool,
        dispatch: MainThreadDispatch,
    ) -> Result<(Self, NativeViewerInput), Error> {
        config.validate()?;
        let (events, mut receive) = mpsc::channel::<Vec<u8>>(16);
        let (failure, mut failures) = watch::channel(None::<String>);
        let state = Arc::new(State {
            view: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            poll_pending: AtomicBool::new(false),
            displayed: AtomicU64::new(0),
            wake: Condvar::new(),
            gate: Mutex::new(()),
            dispatch,
            events,
            failure,
        });
        let weak = Arc::downgrade(&state);
        let (native_input, sink) = NativeViewerInput::from_callback(Arc::new(move |enabled| {
            let state = weak
                .upgrade()
                .ok_or_else(|| io::Error::other("native view is closed"))?;
            state.schedule(move |state| {
                let mut error = [0; 512];
                // SAFETY: schedule holds State and runs only on AppKit main.
                if !unsafe {
                    pab_mac_view_input(state.pointer(), enabled, error.as_mut_ptr(), error.len())
                } {
                    state.fail(message(&error));
                }
            })
        }));
        let context = Box::into_raw(Box::new(sink)).cast();
        let rotation = match config.screen.rotation {
            DesktopRotation::Identity => 0,
            DesktopRotation::Clockwise90 => 1,
            DesktopRotation::Clockwise180 => 2,
            DesktopRotation::Clockwise270 => 3,
        };
        let mut error = [0; 512];
        // SAFETY: caller guarantees the main thread and live NSWindow. Native
        // owns context immediately, including failed view construction.
        let view = unsafe {
            pab_mac_view_new(
                window,
                config.screen.pixel_width,
                config.screen.pixel_height,
                rotation,
                context,
                input,
                drop_input,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if view.is_null() {
            return Err(message(&error).into());
        }
        state.view.store(view as usize, Ordering::Release);
        let (mut writer, reader) = tokio::io::duplex(256);
        let renderer = Renderer::start(reader);
        let relay = tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    biased;
                    changed=failures.changed()=>{
                        if changed.is_err() { break; }
                        let error=failures.borrow_and_update().clone();
                        if let Some(error)=error {
                            let mut bytes=error.into_bytes(); bytes.truncate(512);
                            // Avoid truncating inside UTF-8; native errors are
                            // metadata, never a decoded frame or input text.
                            while std::str::from_utf8(&bytes).is_err() { bytes.pop(); }
                            if bytes.is_empty() { bytes.extend_from_slice(b"native view failed"); }
                            let _=writer.write_u8(5).await;
                            let _=writer.write_u16_le(bytes.len() as u16).await;
                            let _=writer.write_all(&bytes).await; break;
                        }
                        continue;
                    },
                    value=receive.recv()=>value,
                };
                let Some(bytes) = next else {
                    break;
                };
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });
        let (frames, mut requests) = mpsc::channel::<(u64, Vec<u8>)>(4);
        let weak = Arc::downgrade(&state);
        let info = config.clone();
        let decode = tokio::task::spawn_blocking(move || {
            let mut error = [0; 512];
            // SAFETY: valid config; this thread exclusively owns the decoder.
            let decoder = Decoder(unsafe {
                pab_mac_decoder_new(
                    info.screen.pixel_width,
                    info.screen.pixel_height,
                    u32::from(info.fps),
                    software,
                    error.as_mut_ptr(),
                    error.len(),
                )
            });
            if decoder.0.is_null() {
                if let Some(s) = weak.upgrade() {
                    s.fail(message(&error));
                }
                return;
            }
            while let Some((index, bytes)) = requests.blocking_recv() {
                if !weak
                    .upgrade()
                    .is_some_and(|s| !s.stopped.load(Ordering::Acquire))
                {
                    break;
                }
                let mut actual_software = false;
                // SAFETY: decoder is uniquely borrowed; packet buffer and all
                // out arguments live throughout the synchronous native call.
                let image = unsafe {
                    pab_mac_decoder_frame(
                        decoder.0,
                        bytes.as_ptr(),
                        bytes.len(),
                        index,
                        &mut actual_software,
                        error.as_mut_ptr(),
                        error.len(),
                    )
                };
                let surface = Surface(image as usize);
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if state.stopped.load(Ordering::Acquire) {
                    break;
                }
                if image.is_null() {
                    state.fail(message(&error));
                    break;
                }
                if index == 0 {
                    state.report(vec![4, if actual_software { 2 } else { 3 }]);
                }
                state.event(1, index);
                if let Err(error) = state.schedule(move |state| {
                    let mut error = [0; 512];
                    // SAFETY: retained surface and view stay alive for this
                    // main-thread draw. Native retains any pending GPU leases.
                    if !unsafe {
                        pab_mac_view_render(
                            state.pointer(),
                            surface.0 as *mut c_void,
                            index,
                            error.as_mut_ptr(),
                            error.len(),
                        )
                    } {
                        state.fail(message(&error));
                    }
                    drop(surface);
                }) {
                    state.fail(error.to_string());
                    break;
                }
                // Retain one pending native surface rather than accumulating
                // UI closures while hidden. The four-packet queue provides
                // backpressure; cancellation wakes this worker without UI work.
                if !state.wait_presented(index) {
                    break;
                }
            }
        });
        let weak = Arc::downgrade(&state);
        let timer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(8));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if state.stopped.load(Ordering::Acquire) {
                    break;
                }
                if state.poll_pending.swap(true, Ordering::AcqRel) {
                    continue;
                }
                if let Err(error) = state.schedule(|state| {
                    let mut error = [0; 512];
                    let mut count = 0;
                    // SAFETY: only the host main thread touches AppKit.
                    if !unsafe {
                        pab_mac_view_poll(
                            state.pointer(),
                            &mut count,
                            error.as_mut_ptr(),
                            error.len(),
                        )
                    } {
                        state.fail(message(&error));
                    } else {
                        let previous = state.displayed.load(Ordering::Acquire);
                        if count < previous || count > previous + 1 {
                            state.fail("native presentation sequence changed");
                        } else if count > previous {
                            state.event(2, count - 1);
                            state.displayed.store(count, Ordering::Release);
                            state.wake.notify_all();
                        }
                    }
                    state.poll_pending.store(false, Ordering::Release);
                }) {
                    state.fail(error.to_string());
                    break;
                }
            }
        });
        Ok((
            Self {
                state,
                config: config.clone(),
                next_frame: 1,
                frames,
                renderer,
                relay,
                timer,
                decode,
            },
            native_input,
        ))
    }
    pub fn presentations(&self) -> NativePresentations {
        self.renderer.presentations()
    }
    pub fn cursor_renderer(&self) -> Result<MacCursorRenderer, Error> {
        Ok(MacCursorRenderer {
            state: Some(Arc::downgrade(&self.state)),
            config: self.config.clone(),
            next_sequence: 1,
            image_id: 0,
            acknowledgements: self.renderer.cursor()?,
        })
    }
    pub async fn submit(&mut self, frame: &VideoFrame) -> Result<(), Error> {
        frame.header.validate(&self.config, self.next_frame)?;
        if frame.bytes.len() != frame.header.payload_bytes as usize {
            return Err("native frame payload length mismatch".into());
        }
        self.renderer.enqueue(frame.header.clone()).await?;
        self.frames
            .send((frame.header.frame_id - 1, frame.bytes.clone()))
            .await
            .map_err(|_| "native decoder ended")?;
        self.renderer.consumed(frame.header.frame_id).await?;
        self.next_frame = self
            .next_frame
            .checked_add(1)
            .ok_or("native frame sequence exhausted")?;
        Ok(())
    }
}
impl Drop for MacNativeViewer {
    fn drop(&mut self) {
        self.state.stop();
        self.timer.abort();
        self.relay.abort();
        self.decode.abort();
        // Running decode is synchronous; it owns no view. On return it sees
        // stopped and releases its surface. Closing frames wakes an idle decode.
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_cursor_retires_handle_and_stopped_generation_skips_queued_native_work() {
        let queue = Arc::new(Mutex::new(Vec::<Box<dyn FnOnce() + Send>>::new()));
        let dispatch_queue = queue.clone();
        let (events, _receive) = mpsc::channel(16);
        let (failure, _) = watch::channel(None);
        let state = Arc::new(State {
            // Deliberately no native view. Executing the closure after stop
            // would call native FFI with null and fail this test.
            view: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            poll_pending: AtomicBool::new(false),
            displayed: AtomicU64::new(0),
            wake: Condvar::new(),
            gate: Mutex::new(()),
            dispatch: Arc::new(move |work| {
                dispatch_queue.lock().unwrap().push(work);
                Ok(())
            }),
            events,
            failure,
        });
        let (writer, reader) = tokio::io::duplex(256);
        let renderer = Renderer::start(reader);
        let config = DesktopVideoConfig {
            session_id: pab_protocol::RequestId::new(),
            generation: 1,
            fps: 15,
            codec: pab_protocol::DesktopCodec::H264AnnexB,
            color: pab_protocol::DesktopColor::Bt709Limited,
            screen: pab_protocol::DesktopScreen {
                id: "generated".into(),
                name: "Generated".into(),
                x: 0,
                y: 0,
                desktop_width: 640,
                desktop_height: 480,
                pixel_width: 640,
                pixel_height: 480,
                rotation: DesktopRotation::Identity,
                scale_milli: 1000,
                primary: true,
            },
        };
        let weak = Arc::downgrade(&state);
        let mut cursor = MacCursorRenderer {
            state: Some(weak.clone()),
            config,
            next_sequence: 1,
            image_id: 0,
            acknowledgements: renderer.cursor().unwrap(),
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(10), cursor.clear())
                .await
                .is_err()
        );
        assert_eq!(queue.lock().unwrap().len(), 1);
        assert!(cursor.clear().await.is_err());
        assert_eq!(queue.lock().unwrap().len(), 1);
        state.stop();
        drop(state);
        let work = queue.lock().unwrap().pop().unwrap();
        assert!(weak.upgrade().is_some());
        work();
        assert!(
            weak.upgrade().is_none(),
            "cursor retained its stopped owner"
        );
        drop(writer);
    }
}
