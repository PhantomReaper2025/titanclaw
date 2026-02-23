//! Bearer token authentication middleware for the web gateway.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;
use url::form_urlencoded;

/// Shared auth state injected via axum middleware state.
#[derive(Clone)]
pub struct AuthState {
    pub token: String,
}

/// Auth middleware that validates bearer token from header or query param.
///
/// SSE connections can't set headers from `EventSource`, so we also accept
/// `?token=xxx` as a query parameter.
pub async fn auth_middleware(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    // Try Authorization header first (constant-time comparison)
    if let Some(auth_header) = headers.get("authorization")
        && let Ok(value) = auth_header.to_str()
        && let Some(token) = value.strip_prefix("Bearer ")
        && bool::from(token.as_bytes().ct_eq(auth.token.as_bytes()))
    {
        return next.run(request).await;
    }

    // Fall back to query parameter for SSE EventSource (constant-time comparison)
    if let Some(query) = request.uri().query()
        && query_token_matches(query, &auth.token)
    {
        return next.run(request).await;
    }

    (StatusCode::UNAUTHORIZED, "Invalid or missing auth token").into_response()
}

fn query_token_matches(query: &str, expected: &str) -> bool {
    form_urlencoded::parse(query.as_bytes()).any(|(key, value)| {
        key == "token" && bool::from(value.as_bytes().ct_eq(expected.as_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_clone() {
        let state = AuthState {
            token: "test-token".to_string(),
        };
        let cloned = state.clone();
        assert_eq!(cloned.token, "test-token");
    }

    #[test]
    fn test_query_token_matches_plain() {
        assert!(query_token_matches("token=test-token", "test-token"));
    }

    #[test]
    fn test_query_token_matches_url_encoded() {
        let expected = "abc+/=xyz";
        assert!(query_token_matches("token=abc%2B%2F%3Dxyz", expected));
    }

    #[test]
    fn test_query_token_matches_rejects_wrong_value() {
        assert!(!query_token_matches("token=wrong", "test-token"));
    }

    #[test]
    fn test_query_token_matches_ignores_other_params() {
        assert!(query_token_matches(
            "foo=1&token=test-token&bar=2",
            "test-token"
        ));
    }
}
