use super::system_query::{bound_reply, bounded, failed, now, push_bounded};
use pab_protocol::*;

pub(super) async fn query_async(id: RequestId, query: &SystemQuery) -> Option<SystemQueryReply> {
    if !matches!(
        query,
        SystemQuery::TerminateProcess { .. }
            | SystemQuery::Services { .. }
            | SystemQuery::Service { .. }
            | SystemQuery::ServiceControl { .. }
    ) {
        return None;
    }
    let mut r = SystemQueryReply::pending(id, query);
    r.sampled_from_unix_ms = Some(now());
    let mut data = match pab_os_control::execute(query).await {
        Ok(d) => d,
        Err(e) => return Some(failed(r, &bounded(&e, 1024))),
    };
    if let (
        SystemQuery::Services { name, state, limit },
        SystemQueryData::Services { entries, .. },
    ) = (query, &mut data)
    {
        r.truncated = pab_os_control::filter_services(
            entries,
            name.as_deref(),
            state.as_deref(),
            *limit as usize,
        );
        if r.truncated {
            r.stop_reason = Some("entry_limit".into());
        }
        let mut selected = vec![];
        let mut bytes = 0;
        for entry in std::mem::take(entries) {
            if !push_bounded(&mut selected, entry, &mut bytes, &mut r) {
                break;
            }
        }
        *entries = selected;
    }
    r.state = "completed".into();
    let outcome = match &data {
        SystemQueryData::ProcessTermination { result } => Some((&result.outcome, &result.error)),
        SystemQueryData::ServiceControl { result } => Some((&result.outcome, &result.error)),
        _ => None,
    };
    if let Some((outcome, error)) = outcome {
        if outcome != "completed" {
            r.state = "failed".into();
            r.error = error.clone().or_else(|| Some(outcome.clone()));
        }
    }
    r.data = Some(data);
    r.sampled_at_unix_ms = Some(now());
    bound_reply(&mut r);
    Some(r)
}

#[cfg(test)]
#[path = "system_query_c3_tests.rs"]
mod tests;
