use std::{sync::Arc, time::Duration};

use pab_protocol::DeploymentId;

use crate::ControlPlane;

mod relay_auth;
mod relay_session;
mod session;
mod transport;

pub use relay_auth::{RelayControlAuth, RelayControlAuthError};
pub use session::ControlSession;
pub use transport::{control_router, serve_tls};

#[derive(Debug, Clone)]
pub struct ControlApiConfig {
    pub registration_enabled: bool,
    pub relay_policy_validity: Duration,
}

impl Default for ControlApiConfig {
    fn default() -> Self {
        Self {
            registration_enabled: true,
            relay_policy_validity: Duration::from_secs(60),
        }
    }
}

#[derive(Clone)]
pub struct ControlApiState {
    pub(super) control: Arc<ControlPlane>,
    pub(super) deployment_id: DeploymentId,
    pub(super) config: ControlApiConfig,
    pub(super) relay_auth: RelayControlAuth,
}

impl ControlApiState {
    pub fn new(
        control: ControlPlane,
        deployment_id: DeploymentId,
        config: ControlApiConfig,
        relay_auth: RelayControlAuth,
    ) -> Self {
        Self {
            control: Arc::new(control),
            deployment_id,
            config,
            relay_auth,
        }
    }
}
