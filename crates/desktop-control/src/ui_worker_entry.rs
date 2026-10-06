use crate::{
    ui_engine::{UiBackend, UiEngine, UiWindow},
    ui_registry::UiOwner,
    ui_worker::{WorkerFailure, read_frame, write_frame},
};
use pab_protocol::*;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMessage {
    pub request_id: RequestId,
    pub owner: UiOwner,
    pub command: WorkerCommand,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerCommand {
    Query {
        ticket: Option<UiWindow>,
        request: UiRequest,
    },
    Sample {
        ticket: Option<UiWindow>,
        scope: UiScope,
        selector: UiSelector,
        condition: UiCondition,
    },
    ReleaseOwner,
    ReleaseWindow {
        window_ref: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReply {
    pub request_id: RequestId,
    pub snapshot: Option<UiSnapshot>,
}

pub fn serve<B: UiBackend>(backend: B) -> Result<(), WorkerFailure> {
    let mut engine = UiEngine::new(backend);
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    loop {
        let message: WorkerMessage = serde_json::from_value(read_frame(&mut input)?)
            .map_err(|_| WorkerFailure::InvalidFrame)?;
        let snapshot = match message.command {
            WorkerCommand::Query { ticket, request } => {
                Some(engine.run(&message.owner, ticket.as_ref(), &request))
            }
            WorkerCommand::Sample {
                ticket,
                scope,
                selector,
                condition,
            } => {
                UiRequest::Wait {
                    scope: scope.clone(),
                    selector: selector.clone(),
                    condition: condition.clone(),
                    timeout_ms: 5000,
                    poll_ms: 250,
                }
                .validate()
                .map_err(|_| WorkerFailure::InvalidFrame)?;
                Some(engine.sample(
                    &message.owner,
                    ticket.as_ref(),
                    &scope,
                    &selector,
                    &condition,
                ))
            }
            WorkerCommand::ReleaseOwner => {
                engine.release_owner(&message.owner);
                None
            }
            WorkerCommand::ReleaseWindow { window_ref } => {
                engine.release_window(&window_ref);
                None
            }
        };
        write_frame(
            &mut output,
            &serde_json::to_value(WorkerReply {
                request_id: message.request_id,
                snapshot,
            })
            .map_err(|_| WorkerFailure::InvalidFrame)?,
        )?;
    }
}

pub fn run() -> Result<(), WorkerFailure> {
    #[cfg(windows)]
    {
        let apartment =
            crate::ui_windows::UiApartment::enter().map_err(|_| WorkerFailure::Unavailable)?;
        let backend = crate::ui_windows::WindowsUi::new(&apartment)
            .map_err(|_| WorkerFailure::Unavailable)?;
        serve(backend)
    }
    #[cfg(target_os = "macos")]
    {
        serve(crate::ui_macos::MacUi)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    Err(WorkerFailure::Unavailable)
}
