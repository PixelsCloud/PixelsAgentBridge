use super::{WebState, session};
use crate::audit::{AuditAction, AuditRecord, AuditResource};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, Method},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

pub(super) fn request_id(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-pixels-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4)
}

fn action(method: &Method, route: &str) -> Option<AuditAction> {
    Some(match (method.as_str(), route) {
        ("PATCH", "/api/web/service") => AuditAction::RegistrationUpdated,
        ("PATCH", "/api/web/accounts/{id}") => AuditAction::AccountUpdated,
        ("PUT", "/api/web/accounts/{id}/traffic") => AuditAction::BandwidthUpdated,
        ("POST", "/api/web/password") => AuditAction::PasswordChanged,
        ("POST", "/api/web/accounts/{id}/password-reset") => AuditAction::PasswordReset,
        ("POST", "/api/web/accounts/{id}/sessions/revoke" | "/api/web/sessions/revoke") => {
            AuditAction::SessionsRevoked
        }
        ("PATCH", "/api/web/devices/{id}") => AuditAction::DeviceRenamed,
        ("PUT", "/api/account/devices/{id}/association") => AuditAction::DeviceAssociated,
        (
            "DELETE",
            "/api/web/devices/{id}/association" | "/api/account/devices/{id}/association",
        ) => AuditAction::DeviceDisassociated,
        ("PATCH", "/api/web/relays/{node}/state" | "/api/admin/relays/{node}/state") => {
            AuditAction::RelayStateChange
        }
        ("POST", "/api/web/relays/reload" | "/api/admin/relays/reload") => AuditAction::RelayReload,
        ("PATCH", "/api/web/relays/{node}/capacity" | "/api/admin/relays/{node}/capacity") => AuditAction::RelayCapacityUpdated,
        ("GET", "/api/web/usage/export") => AuditAction::UsageExport,
        ("GET", "/api/web/audit/export") => AuditAction::AuditExport,
        _ => return None,
    })
}

pub(super) async fn context(
    State(state): State<WebState>,
    mut request: Request,
    next: Next,
) -> Response {
    let id = Uuid::new_v4();
    request
        .headers_mut()
        .insert("x-pixels-request-id", id.to_string().parse().unwrap());
    let headers = request.headers().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_default();
    let path = request.uri().path().to_owned();
    let tracked = action(request.method(), &route);
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-pixels-request-id", id.to_string().parse().unwrap());
    if let Some(action) = tracked
        .filter(|_| response.status().is_client_error() || response.status().is_server_error())
    {
        let class = match response.status().as_u16() {
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not_found",
            409 => "conflict",
            429 => "rate_limited",
            500..=599 => "server_error",
            _ => "invalid_input",
        };
        let viewer = if path.starts_with("/api/account/") || path.starts_with("/api/admin/") {
            session::native_viewer(&state, &headers).await.ok()
        } else {
            session::viewer(&state, &headers).await.ok()
        };
        let parts = path.split('/').collect::<Vec<_>>();
        let resource = if route.contains("/accounts/{id}") {
            parts
                .get(4)
                .and_then(|id| Uuid::parse_str(id).ok())
                .map(AuditResource::Account)
        } else if route.contains("/devices/{id}") {
            parts
                .get(4)
                .and_then(|id| Uuid::parse_str(id).ok())
                .map(AuditResource::Device)
        } else if route.contains("/relays/{node}") {
            parts
                .get(4)
                .filter(|node| pab_protocol::valid_relay_node_id(node))
                .map(|node| AuditResource::Relay((*node).to_owned()))
        } else {
            None
        };
        let mut record = AuditRecord::new(
            viewer.map(|v| v.id),
            action,
            resource.unwrap_or(AuditResource::Unknown),
        );
        record.request_id = id;
        record.result = if matches!(class, "unauthorized" | "forbidden") {
            "denied"
        } else {
            "failed"
        };
        record.details.error_code = Some(
            response
                .extensions()
                .get::<super::support::AuditErrorCode>()
                .map(|code| code.0)
                .unwrap_or(class)
                .into(),
        );
        if state
            .control
            .store()
            .record_audit_failure(record, class)
            .await
            .is_err()
        {
            tracing::warn!("audit failure record unavailable");
        }
    }
    response
}
