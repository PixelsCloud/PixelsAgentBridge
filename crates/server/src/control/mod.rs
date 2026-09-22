use std::sync::Arc;

use pab_protocol::DeploymentId;

use crate::ControlPlane;

mod session;
mod transport;

pub use session::ControlSession;
pub use transport::{control_router, serve_tls};

#[derive(Debug, Clone)]
pub struct ControlApiConfig {
    pub registration_enabled: bool,
}

impl Default for ControlApiConfig {
    fn default() -> Self {
        Self {
            registration_enabled: true,
        }
    }
}

#[derive(Clone)]
pub struct ControlApiState {
    pub(super) control: Arc<ControlPlane>,
    pub(super) deployment_id: DeploymentId,
    pub(super) config: ControlApiConfig,
}

impl ControlApiState {
    pub fn new(
        control: ControlPlane,
        deployment_id: DeploymentId,
        config: ControlApiConfig,
    ) -> Self {
        Self {
            control: Arc::new(control),
            deployment_id,
            config,
        }
    }
}
