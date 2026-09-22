use std::process::ExitCode;

use pab_executor::{ExecutorConfig, run_executor};

#[tokio::main]
async fn main() -> ExitCode {
    let result = match ExecutorConfig::from_env() {
        Ok(config) => run_executor(config).await,
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pab-executor: {error}");
            ExitCode::FAILURE
        }
    }
}
