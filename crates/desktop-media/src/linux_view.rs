//! In-process GTK view. The host owns the main loop; decoding never re-enters
//! GTK. One uploaded frame waits for after-paint before the next submission.
use crate::{
    NativePresentations, NativeViewerInput, renderer::Renderer, software::SoftwareDecoder,
    viewer_input::InputSink,
};
use pab_protocol::{DesktopRotation, DesktopVideoConfig};
use pab_remote_desktop::VideoFrame;
use std::{
    ffi::{CStr, c_char, c_void},
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc as blocking,
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

/// Repair the affected GTK 3 button accessibility state callback, if needed.
/// Returns whether the process-local compatibility override is installed.
///
/// # Safety
/// Call on the GUI main thread before constructing GTK widgets or starting
/// other threads that use GTK. Subsequent calls must remain on that thread.
pub unsafe fn initialize_linux_accessibility() -> bool {
    // SAFETY: the caller guarantees initialization precedes concurrent GTK use.
    unsafe { pab_linux_accessibility_initialize() }
}
unsafe extern "C" {
    fn pab_linux_accessibility_initialize() -> bool;
    fn pab_linux_view_new(
        host: *mut c_void,
        width: u32,
        height: u32,
        rotation: u32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn pab_linux_view_submit(
        view: *mut c_void,
        nv12: *const u8,
        size: usize,
        index: u64,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_linux_view_poll(
        view: *mut c_void,
        presented: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_linux_view_cursor(
        view: *mut c_void,
        bytes: *const u8,
        size: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_linux_view_drop(view: *mut c_void);
    fn pab_linux_view_set_input(
        view: *mut c_void,
        context: *mut c_void,
        callback: extern "C" fn(*mut c_void, *const u8) -> bool,
        drop: extern "C" fn(*mut c_void),
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
    fn pab_linux_view_input(
        view: *mut c_void,
        enabled: bool,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
}
extern "C" fn input(context: *mut c_void, bytes: *const u8) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || bytes.is_null() {
            return false;
        }
        // SAFETY: the native sink owns the boxed Arc and borrows eight bytes
        // only during this call. Typed contents are never retained in logs.
        let sink = unsafe { &*context.cast::<Arc<InputSink>>() };
        sink.emit(unsafe { *bytes.cast::<[u8; 8]>() })
    }))
    .unwrap_or(false)
}
extern "C" fn drop_input(context: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: native takes ownership once, including registration failure.
        let sink = unsafe { Box::from_raw(context.cast::<Arc<InputSink>>()) };
        sink.fail();
    }));
}
fn message(bytes: &[c_char; 512]) -> String {
    // SAFETY: the buffer is zero initialized and the native ABI NUL-terminates.
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_string_lossy()
        .into_owned()
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
    input_failure: Mutex<Option<Weak<InputSink>>>,
}
impl State {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
        if let Ok(sink) = self.input_failure.lock() {
            if let Some(sink) = sink.as_ref().and_then(Weak::upgrade) {
                sink.fail();
            }
        }
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
    fn schedule(self: &Arc<Self>, work: impl FnOnce(&Self) + Send + 'static) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(io::Error::other("native GTK view is closed"));
        }
        let state = self.clone();
        (self.dispatch)(Box::new(move || {
            if !state.stopped.load(Ordering::Acquire) {
                work(&state);
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
            // SAFETY: UI closures hold State, so none can still borrow the
            // native object. Its unique destructor runs on the host GTK thread.
            let _ = (self.dispatch)(Box::new(move || unsafe {
                pab_linux_view_drop(view as *mut c_void)
            }));
        }
    }
}

pub struct LinuxNativeViewer {
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
pub struct LinuxCursorRenderer {
    state: Option<std::sync::Weak<State>>,
    config: DesktopVideoConfig,
    next_sequence: u64,
    image_id: u64,
    acknowledgements: crate::renderer::CursorAcknowledgements,
}
impl LinuxCursorRenderer {
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
            // SAFETY: schedule retains the view on GTK main; packet remains
            // alive for the synchronous upload. The native side retains no bytes.
            if unsafe {
                pab_linux_view_cursor(
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
impl LinuxNativeViewer {
    /// Create the GTK view and its existing bounded input bridge.
    ///
    /// # Safety
    /// Same GTK main-thread, host pointer and dispatch contract as `create`.
    pub unsafe fn create_interactive(
        window: *mut c_void,
        config: &DesktopVideoConfig,
        dispatch: MainThreadDispatch,
    ) -> Result<(Self, NativeViewerInput), Error> {
        // SAFETY: forwarded unchanged from this constructor's caller.
        let view = unsafe { Self::create(window, config, dispatch) }?;
        let weak = Arc::downgrade(&view.state);
        let (native_input, sink) = NativeViewerInput::from_callback(Arc::new(move |enabled| {
            let state = weak
                .upgrade()
                .ok_or_else(|| io::Error::other("native GTK view closed"))?;
            state.schedule(move |state| {
                let mut error = [0; 512];
                // SAFETY: schedule holds State and uses the GTK main thread.
                if !unsafe {
                    pab_linux_view_input(state.pointer(), enabled, error.as_mut_ptr(), error.len())
                } {
                    state.fail(message(&error));
                }
            })
        }));
        *view
            .state
            .input_failure
            .lock()
            .map_err(|_| "native input state failed")? = Some(Arc::downgrade(&sink));
        let context = Box::into_raw(Box::new(sink)).cast();
        let mut error = [0; 512];
        // SAFETY: construction is on GTK main. Native owns context immediately,
        // including failure; its destructor releases the boxed input sink.
        if !unsafe {
            pab_linux_view_set_input(
                view.state.pointer(),
                context,
                input,
                drop_input,
                error.as_mut_ptr(),
                error.len(),
            )
        } {
            return Err(message(&error).into());
        }
        Ok((view, native_input))
    }
    /// Create on the host GTK main thread within an entered Tokio runtime.
    ///
    /// # Safety
    /// `window` must be a live, realized GtkWindow owned by this host; dispatch
    /// must execute closures exclusively on the same GTK thread, including drop.
    /// Never accept a window pointer from IPC, a webview, or a remote peer.
    pub unsafe fn create(
        window: *mut c_void,
        config: &DesktopVideoConfig,
        dispatch: MainThreadDispatch,
    ) -> Result<Self, Error> {
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
            input_failure: Mutex::new(None),
        });
        let rotation = match config.screen.rotation {
            DesktopRotation::Identity => 0,
            DesktopRotation::Clockwise90 => 1,
            DesktopRotation::Clockwise180 => 2,
            DesktopRotation::Clockwise270 => 3,
        };
        let mut error = [0; 512];
        // SAFETY: caller guarantees host pointer lifetime and the GTK thread.
        let view = unsafe {
            pab_linux_view_new(
                window,
                config.screen.pixel_width,
                config.screen.pixel_height,
                rotation,
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
                            while std::str::from_utf8(&bytes).is_err() { bytes.pop(); }
                            if bytes.is_empty() { bytes.extend_from_slice(b"native GTK view failed"); }
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
            let mut decoder = match SoftwareDecoder::new(
                info.screen.pixel_width as usize,
                info.screen.pixel_height as usize,
            ) {
                Ok(value) => value,
                Err(error) => {
                    if let Some(state) = weak.upgrade() {
                        state.fail(error.to_string());
                    }
                    return;
                }
            };
            let mut recycled = Vec::new();
            while let Some((index, bytes)) = requests.blocking_recv() {
                let Some(state) = weak.upgrade() else {
                    break;
                };
                if state.stopped.load(Ordering::Acquire) {
                    break;
                }
                let frame = match decoder.decode_owned(&bytes, recycled) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        state.fail("native software decoder has no output");
                        break;
                    }
                    Err(error) => {
                        state.fail(error.to_string());
                        break;
                    }
                };
                if state.stopped.load(Ordering::Acquire) {
                    break;
                }
                if index == 0 {
                    state.report(vec![4, 2]);
                }
                state.event(1, index);
                let (returned, buffer) = blocking::sync_channel(1);
                if let Err(error) = state.schedule(move |state| {
                    let mut error = [0; 512];
                    // SAFETY: host GTK thread, live view, owned NV12 storage.
                    // Native upload completes before this allocation is reused;
                    // GTK painting happens later and only uses GL textures.
                    let good = unsafe {
                        pab_linux_view_submit(
                            state.pointer(),
                            frame.as_ptr(),
                            frame.len(),
                            index,
                            error.as_mut_ptr(),
                            error.len(),
                        )
                    };
                    if !good {
                        state.fail(message(&error));
                    }
                    let _ = returned.send(frame);
                }) {
                    state.fail(error.to_string());
                    break;
                }
                recycled = loop {
                    if state.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    match buffer.recv_timeout(Duration::from_millis(50)) {
                        Ok(frame) => break frame,
                        Err(blocking::RecvTimeoutError::Timeout) => continue,
                        Err(blocking::RecvTimeoutError::Disconnected) => {
                            state.fail("native upload cancelled");
                            return;
                        }
                    }
                };
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
                    // SAFETY: the host main thread owns all GTK calls. This poll
                    // does not pump/re-enter GTK and only observes after-paint.
                    if !unsafe {
                        pab_linux_view_poll(
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
        Ok(Self {
            state,
            config: config.clone(),
            next_frame: 1,
            frames,
            renderer,
            relay,
            timer,
            decode,
        })
    }
    pub fn presentations(&self) -> NativePresentations {
        self.renderer.presentations()
    }
    pub fn cursor_renderer(&self) -> Result<LinuxCursorRenderer, Error> {
        Ok(LinuxCursorRenderer {
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
impl Drop for LinuxNativeViewer {
    fn drop(&mut self) {
        self.state.stop();
        self.timer.abort();
        self.relay.abort();
        self.decode.abort();
        // Running decode wakes on stop; idle decode wakes when frames closes.
        // Queued UI closures retain State but observe stopped before touching GTK.
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
            input_failure: Mutex::new(None),
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
        let mut cursor = LinuxCursorRenderer {
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
