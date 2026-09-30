#![forbid(unsafe_code)]

mod native_environment;

pub use native_environment::{PlatformDetectionError, detect_native_execution_context};

mod system_query;
pub use system_query::SystemCollector;
