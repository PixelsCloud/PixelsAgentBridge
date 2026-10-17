#![forbid(unsafe_code)]
//! Remote desktop session and reliable video framing. Native capture/rendering
//! are adapters; the shared connection, authentication and legacy streams stay intact.
mod wire;
pub use wire::{DesktopVideoReader, DesktopVideoWriter, VideoFrame};
mod cursor_wire;
pub use cursor_wire::{CursorUpdate, DesktopCursorReader, DesktopCursorWriter};
mod host;
pub use host::{
    DesktopBackend, DesktopConnectionGuard, DesktopCursorSource, DesktopHost, DesktopInputSink,
    DesktopSource, MediaFrame, MediaFuture,
};
mod client;
pub use client::{
    RemoteDesktopCursor, RemoteDesktopInputControl, RemoteDesktopPresentationControl,
    RemoteDesktopSession,
};

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
    /// Only media lifecycle failures may reopen a viewer automatically. Never
    /// retry ownership, capability, malformed protocol or occupied sessions.
    pub fn is_recoverable_media(&self) -> bool {
        matches!(
            self,
            Self::Closed
                | Self::Timeout
                | Self::Transport(_)
                | Self::Rejected {
                    code: DesktopErrorCode::MediaUnavailable | DesktopErrorCode::StaleGeneration,
                    ..
                }
        )
    }
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

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn recovery_never_bypasses_ownership_or_protocol_rejections() {
        for code in [
            DesktopErrorCode::Unsupported,
            DesktopErrorCode::InvalidRequest,
            DesktopErrorCode::Busy,
            DesktopErrorCode::NotFound,
            DesktopErrorCode::WrongOwner,
        ] {
            assert!(!DesktopError::reject(code, "test").is_recoverable_media());
        }
        for code in [
            DesktopErrorCode::MediaUnavailable,
            DesktopErrorCode::StaleGeneration,
        ] {
            assert!(DesktopError::reject(code, "test").is_recoverable_media());
        }
        assert!(DesktopError::Closed.is_recoverable_media());
        assert!(DesktopError::Timeout.is_recoverable_media());
        assert!(!DesktopError::Protocol("invalid frame").is_recoverable_media());
    }
}
