#![forbid(unsafe_code)]

mod aggregate;
mod validation;

pub use aggregate::{AcceptedTask, EventApply, TaskAggregate, TaskRuntimeError};
