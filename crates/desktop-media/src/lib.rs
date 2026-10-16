//! Platform media workers. The authenticated session/transport crate contains
//! no native FFI. Only compressed packets cross the private worker pipes.
mod input;
mod process;
mod renderer;
pub mod software;
#[cfg(windows)]
mod software_ffi;
mod viewer_input;
pub use process::{NativeBackend, NativeViewer};
pub use renderer::NativePresentations;
pub use viewer_input::NativeViewerInput;

/// Invoke only in a dedicated child, before starting a GUI/runtime. Arguments
/// start with the operation (inventory, capture or render), without argv[0].
pub fn worker_main(arguments: impl IntoIterator<Item = String>) -> i32 {
    #[cfg(windows)]
    {
        use std::ffi::{CString, c_char};
        unsafe extern "C" {
            fn pab_media_main(argc: i32, argv: *const *const c_char) -> i32;
        }
        let arguments = std::iter::once("pab-media-worker".to_string()).chain(arguments);
        let Ok(arguments) = arguments.map(CString::new).collect::<Result<Vec<_>, _>>() else {
            return 1;
        };
        let pointers = arguments.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
        // SAFETY: all strings/pointers live for this synchronous call; native
        // catches C++ exceptions and retains no argument after returning.
        unsafe { pab_media_main(pointers.len() as i32, pointers.as_ptr()) }
    }
    #[cfg(not(windows))]
    {
        let _ = arguments;
        eprintln!("native desktop media is not available on this platform yet");
        1
    }
}
