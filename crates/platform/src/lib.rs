#![forbid(unsafe_code)]

mod native_environment;

pub use native_environment::{PlatformDetectionError, detect_native_execution_context};

mod system_query;
pub use system_query::SystemCollector;

mod system_query_c2;
pub use system_query_c2::query_async;

mod system_query_c3;
