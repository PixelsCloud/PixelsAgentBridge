#![forbid(unsafe_code)]

mod limiter;

pub use limiter::{Acquire, AggregateLimiter, LimitKey, Rate};
