#![forbid(unsafe_code)]
//! Remote desktop session and reliable video framing. Native capture/rendering
//! are adapters; the shared connection, authentication and legacy streams stay intact.
mod wire;
pub use wire::{DesktopVideoReader, DesktopVideoWriter, VideoFrame};
mod host;
pub use host::{
    DesktopBackend, DesktopConnectionGuard, DesktopHost, DesktopInputSink, DesktopSource,
    MediaFrame, MediaFuture,
};
mod client;
pub use client::{RemoteDesktopInputControl, RemoteDesktopSession};

use pab_protocol::DesktopErrorCode;
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("remote desktop {code:?}: {message}")]
    Rejected {
        code: DesktopErrorCode,
        message: String,
    },
    #[error("remote desktop protocol: {0}")]
    Protocol(&'static str),
    #[error("remote desktop stream ended or was cancelled; open a new generation")]
    Closed,
    #[error("remote desktop operation timed out")]
    Timeout,
    #[error(transparent)]
    Transport(#[from] pab_transport::PabConnectionError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, DesktopError>;
impl DesktopError {
    pub(crate) fn reject(code: DesktopErrorCode, message: impl Into<String>) -> Self {
        Self::Rejected {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn code(&self) -> DesktopErrorCode {
        match self {
            Self::Rejected { code, .. } => *code,
            _ => DesktopErrorCode::InvalidRequest,
        }
    }
}
