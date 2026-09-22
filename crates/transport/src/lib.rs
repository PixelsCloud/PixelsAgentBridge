#![forbid(unsafe_code)]

mod connection;
mod endpoint;

pub use connection::{MAX_PAB_MESSAGE_BYTES, PabBiStream, PabConnection, PabConnectionError};

pub use endpoint::{
    PAB_ALPN, PabEndpoint, PabEndpointAddress, PabEndpointConfig, PabEndpointError,
};
