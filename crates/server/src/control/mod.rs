use std::{sync::Arc, time::Duration};

use crate::ControlPlane;

mod relay_auth;
mod relay_session;
mod session;
mod session_error;
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
    pub(crate) control: Arc<ControlPlane>,
    pub(crate) config: ControlApiConfig,
    pub(super) relay_auth: RelayControlAuth,
    pub(crate) server_instance: uuid::Uuid,
    pub(crate) github: Option<Arc<crate::web::GithubConfig>>,
}

impl ControlApiState {
    pub fn new(
        control: ControlPlane,
        config: ControlApiConfig,
        relay_auth: RelayControlAuth,
    ) -> Self {
        Self {
            control: Arc::new(control),
            config,
            relay_auth,
            server_instance: uuid::Uuid::new_v4(),
            github: None,
        }
    }

    pub fn with_github(mut self, config: Option<crate::web::GithubConfig>) -> Self {
        self.github = config.map(Arc::new);
        self
    }
}
