#![forbid(unsafe_code)]

mod aggregate;
mod validation;
mod execution;
pub use execution::{ExecutionCaller, ExecutionContextRegistry, ExecutionResolveError, ResolvedExecution};

pub use aggregate::{AcceptedTask, EventApply, TaskAggregate, TaskRuntimeError};
pub use validation::validate_execution_context;
