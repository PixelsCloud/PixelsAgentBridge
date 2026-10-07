use super::*;
use pab_protocol::{
    AppQuery, AppSnapshot, ExecutionIdentity, RequestId, SystemQueryData, SystemQueryReply,
};

/// Frozen at durable acceptance; reconnection must not silently select a new helper.
pub(crate) struct ApplicationRoute {
    sender: mpsc::Sender<HelperRequest>,
    helper: helper_identity::VerifiedHelper,
}
impl ApplicationRoute {
    pub fn select(
        expected: &ExecutionIdentity,
        required_version: u16,
    ) -> Result<Self, LocalIpcError> {
        let candidates = window_providers()
            .lock()
            .map_err(|_| LocalIpcError::Protocol)?
            .iter()
            .rev()
            .filter(|p| p.application_schema_version.is_some_and(|v| v >= 1))
            .filter_map(|p| {
                p.identity.clone().map(|i| {
                    (
                        p.sender.clone(),
                        i,
                        p.application_schema_version.unwrap_or(0),
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut upgrade_required = false;
        for (sender, helper, version) in candidates {
            if &helper.identity == expected && helper.current() && !sender.is_closed() {
                if version < required_version {
                    upgrade_required = true;
                    continue;
                }
                return Ok(Self { sender, helper });
            }
        }
        if upgrade_required {
            return Err(LocalIpcError::Remote("application_helper_upgrade_required: install the matching desktop helper before using new_instance; no action dispatched".into()));
        }
        Err(LocalIpcError::WindowHelperUnavailable)
    }
    pub async fn execute(
        self,
        id: RequestId,
        query: AppQuery,
    ) -> Result<SystemQueryReply, LocalIpcError> {
        if !self.helper.current() {
            return Err(LocalIpcError::WindowHelperUnavailable);
        }
        let expected = self.helper.identity;
        let mutation = query.is_mutation();
        let (reply, receive) = oneshot::channel();
        // A failed queue reservation is known not to have dispatched anything.
        let permit = tokio::time::timeout(Duration::from_secs(5), self.sender.reserve())
            .await
            .map_err(|_| LocalIpcError::WindowHelperUnavailable)?
            .map_err(|_| LocalIpcError::WindowHelperUnavailable)?;
        permit.send(HelperRequest::ApplicationQuery(
            id,
            query,
            expected.clone(),
            reply,
        ));
        let result = tokio::time::timeout(Duration::from_secs(25), receive)
            .await
            .map_err(|_| LocalIpcError::Timeout)?
            .map_err(|_| {
                LocalIpcError::Remote(
                    "application helper disconnected after dispatch; outcome unconfirmed".into(),
                )
            })??;
        let observed = match result.data.as_ref() {
            Some(SystemQueryData::Applications {
                snapshot: AppSnapshot::List { snapshot },
            }) if !mutation => Some(&snapshot.execution_identity),
            Some(SystemQueryData::Applications {
                snapshot: AppSnapshot::Action { result },
            }) if mutation && result.request_accepted => Some(&result.execution_identity),
            _ => None,
        };
        if observed.is_some_and(|i| i != &expected)
            || ((result.state == "completed" || result.data.is_some()) && observed.is_none())
        {
            return Err(LocalIpcError::Protocol);
        }
        Ok(result)
    }
}

pub(crate) use super::helper_identity::desktop_identities;
