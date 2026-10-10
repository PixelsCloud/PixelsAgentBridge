use crate::{ServiceError, StoreError};
use axum::{
    Json,
    extract::Request,
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("{code}")]
pub struct WebError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
}
impl WebError {
    pub(crate) const fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
    pub(crate) const fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
    pub(crate) const fn invalid() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_input")
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"code": self.code}))).into_response()
    }
}
impl From<sqlx::Error> for WebError {
    fn from(error: sqlx::Error) -> Self {
        if matches!(error, sqlx::Error::RowNotFound) {
            return Self::new(StatusCode::NOT_FOUND, "not_found");
        }
        // Log only the class; database errors can carry values from user inputs.
        tracing::warn!("web database operation failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error")
    }
}
impl From<ServiceError> for WebError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::InvalidCredentials => Self::unauthorized(),
            ServiceError::Credential(_) => Self::invalid(),
            ServiceError::Store(StoreError::NotFound) => {
                Self::new(StatusCode::NOT_FOUND, "not_found")
            }
            ServiceError::Store(StoreError::PermissionDenied) => {
                Self::new(StatusCode::FORBIDDEN, "forbidden")
            }
            ServiceError::Store(StoreError::Conflict(_)) => {
                Self::new(StatusCode::CONFLICT, "conflict")
            }
            ServiceError::Store(StoreError::InvalidInput(_)) => Self::invalid(),
            _ => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        }
    }
}

pub(crate) fn same_origin(headers: &HeaderMap, configured: Option<&str>) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|s| s.to_str().ok()) else {
        return false;
    };
    let Ok(origin) = url::Url::parse(origin) else {
        return false;
    };
    if origin.scheme() != "https"
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
    {
        return false;
    }
    let expected = configured.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "https://{}",
            headers
                .get(header::HOST)
                .and_then(|s| s.to_str().ok())
                .unwrap_or("")
        )
    });
    url::Url::parse(&expected).is_ok_and(|expected| expected.origin() == origin.origin())
}

pub(crate) async fn browser_boundary(
    axum::extract::State(state): axum::extract::State<super::WebState>,
    request: Request,
    next: Next,
) -> Response {
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        && !same_origin(request.headers(), state.origin.as_deref())
    {
        return WebError::new(StatusCode::FORBIDDEN, "origin_rejected").into_response();
    }
    let mut response = next.run(request).await;
    if response.status().is_client_error()
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes().starts_with(b"application/json"))
    {
        let status = response.status();
        let code = match status {
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::PAYLOAD_TOO_LARGE => "invalid_input",
            _ => "invalid_input",
        };
        response = WebError::new(status, code).into_response();
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

// Native account calls never accept cookie authentication. Cross-origin browser
// writes are rejected separately from native requests, which have no Origin.
pub(crate) async fn native_boundary(
    axum::extract::State(state): axum::extract::State<super::WebState>,
    request: Request,
    next: Next,
) -> Response {
    if request.headers().contains_key(header::ORIGIN)
        && !same_origin(request.headers(), state.origin.as_deref())
    {
        return WebError::new(StatusCode::FORBIDDEN, "origin_rejected").into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

pub async fn response_headers(request: Request, next: Next) -> Response {
    let asset = request.uri().path().starts_with("/assets/");
    let mut response = next.run(request).await;
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"text/html"));
    if asset && html {
        response = StatusCode::NOT_FOUND.into_response();
    }
    let headers = response.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers
        .entry(header::REFERRER_POLICY)
        .or_insert("same-origin".parse().unwrap());
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    if html && !asset {
        headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    }
    headers.insert(
        header::CACHE_CONTROL,
        if asset && !html {
            "public, max-age=31536000, immutable"
        } else {
            "no-store"
        }
        .parse()
        .unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cross_site_missing_and_malformed_origins() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "web.example:8443".parse().unwrap());
        assert!(!same_origin(&headers, None));
        for bad in [
            "null",
            "http://web.example:8443",
            "https://other.example",
            "https://web.example:8443/path",
            "https://user@web.example:8443",
        ] {
            headers.insert(header::ORIGIN, bad.parse().unwrap());
            assert!(!same_origin(&headers, None), "{bad}");
        }
        headers.insert(header::ORIGIN, "https://web.example:8443".parse().unwrap());
        assert!(same_origin(&headers, None));
    }
    #[test]
    fn configured_origin_takes_precedence_over_proxy_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "backend:8443".parse().unwrap());
        headers.insert(header::ORIGIN, "https://public.example".parse().unwrap());
        assert!(same_origin(&headers, Some("https://public.example")));
        assert!(!same_origin(&headers, None));
        assert!(!same_origin(&headers, Some("https://another.example")));
    }
}
