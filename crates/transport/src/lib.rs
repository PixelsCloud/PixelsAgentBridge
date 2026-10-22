#![forbid(unsafe_code)]

mod connection;
mod endpoint;
mod traffic;
pub use traffic::DirectTraffic;

pub use connection::{
    ConnectionPath, MAX_BINARY_FRAME_BYTES, MAX_PAB_MESSAGE_BYTES, PabBiStream, PabConnection,
    PabConnectionError, PabFrameReader, PabFrameWriter,
};

pub use endpoint::{
    ConnectionMode, PAB_ALPN, PabEndpoint, PabEndpointAddress, PabEndpointConfig, PabEndpointError,
};
