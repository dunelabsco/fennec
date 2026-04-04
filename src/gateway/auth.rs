use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use super::routes::AppState;

/// Middleware that checks `Authorization: Bearer <token>` header.
///
/// If `auth_token` in state is `None` or empty, all requests pass through
/// (open access). Otherwise, the request must carry a matching Bearer token.
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let required = state
        .auth_token
        .as_deref()
        .unwrap_or("");

    if required.is_empty() {
        // Open access -- no auth required.
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let provided = &header["Bearer ".len()..];
            if provided == required {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
