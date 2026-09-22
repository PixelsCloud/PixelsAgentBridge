#![forbid(unsafe_code)]

mod native_environment;

pub use native_environment::{PlatformDetectionError, detect_native_execution_context};
