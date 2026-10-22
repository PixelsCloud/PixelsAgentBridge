#![forbid(unsafe_code)]
//! Remote desktop session and reliable video framing. Native capture/rendering
//! are adapters; the shared connection, authentication and legacy streams stay intact.
mod wire;
pub use wire::{DesktopVideoReader, DesktopVideoWriter, VideoFrame};
mod cursor_wire;
pub use cursor_wire::{CursorUpdate, DesktopCursorReader, DesktopCursorWriter};
mod flow;
mod quality;
mod host;
pub use host::{
    DesktopBackend, DesktopConnectionGuard, DesktopCursorSource, DesktopHost, DesktopInputSink,
    DesktopSource, MediaFrame, MediaFuture,
};
mod client;
pub use client::{
    RemoteDesktopClipboardControl,
    RemoteDesktopCursor, RemoteDesktopInputControl, RemoteDesktopPresentationControl,
    RemoteDesktopSession,
    RemoteDesktopQualityControl,
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
            // Native helper/session retirement can race a control request.
            // Preserve its lifecycle classification across the wire so the
            // viewer may reopen a generation within its existing retry budget.
            Self::Closed => DesktopErrorCode::StaleGeneration,
            Self::Timeout => DesktopErrorCode::MediaUnavailable,
            _ => DesktopErrorCode::InvalidRequest,
        }
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn lifecycle_errors_remain_recoverable_after_a_wire_reply() {
        for error in [DesktopError::Closed, DesktopError::Timeout] {
            let reply = pab_protocol::RemoteDesktopReply::Error {
                code: error.code(),
                message: error.to_string(),
            };
            let encoded = serde_json::to_vec(&reply).unwrap();
            let decoded: pab_protocol::RemoteDesktopReply =
                serde_json::from_slice(&encoded).unwrap();
            let pab_protocol::RemoteDesktopReply::Error { code, message } = decoded else {
                panic!("expected a media failure reply");
            };
            assert_ne!(code, DesktopErrorCode::InvalidRequest);
            assert!(DesktopError::reject(code, message).is_recoverable_media());
        }
        assert_eq!(
            DesktopError::Protocol("invalid frame").code(),
            DesktopErrorCode::InvalidRequest
        );
    }

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
