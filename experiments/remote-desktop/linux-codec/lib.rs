// Compile the exact production codec and private native ABI on Linux. Do not
// maintain a separate experimental encoder/conversion implementation.
#[path = "../../../crates/desktop-media/src/software.rs"]
pub mod software;
#[path = "../../../crates/desktop-media/src/software_ffi.rs"]
mod software_ffi;
