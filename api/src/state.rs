//! Shared HTTP-layer plumbing: Axum state, pool gating and the standard error
//! bodies. Query failures are logged and mapped to a generic client message —
//! the raw driver error (which can embed connection strings, usernames and
//! schema detail) never reaches the client.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::{http::StatusCode, Json};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::error;

use crate::admin::SessionKey;

#[derive(Clone)]
pub struct AppState {
    /// `None` when `DATABASE_URL` is not set: the server boots regardless and
    /// the `/api/*` data endpoints report 503 instead of taking the whole
    /// process down.
    pub pool: Option<PgPool>,
    /// `None` when `SESSION_SECRET` is missing or under 32 bytes: the admin
    /// login then reports 503 instead of minting cookies under a weak key.
    pub session_key: Option<SessionKey>,
    /// Bounds concurrent argon2id verifies in `/api/admin/login` (work
    /// admission: each verify costs ~19 MiB + a heavy CPU slice). Per-app so
    /// tests get isolated gates; sized by `admin::LOGIN_MAX_CONCURRENT` at
    /// boot, and a saturated gate answers 429 instead of queuing.
    pub login_gate: Arc<Semaphore>,
    /// Admin-auth readiness, set by `admin::bootstrap_admin` ONLY when no
    /// credential rotation was requested or the requested rotation was
    /// applied. A requested-but-FAILED rotation leaves it false: login stays
    /// disabled until a successful boot sync (fail closed against the stale
    /// credential the rotation was meant to replace). Storefront routes never
    /// read this.
    pub admin_ready: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Standard endpoint error: status code plus JSON error body.
pub type ApiError = (StatusCode, Json<ErrorBody>);

/// Body of the 503 returned when the process booted without `DATABASE_URL`.
pub const DATABASE_NOT_CONFIGURED: &str = "database not configured: set DATABASE_URL";

/// Body of the 503 returned when a query fails; deliberately generic.
pub const DATABASE_UNAVAILABLE: &str = "database unavailable";

/// 503 used when the process was booted without `DATABASE_URL`.
pub fn database_not_configured() -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorBody {
            error: DATABASE_NOT_CONFIGURED.to_string(),
        }),
    )
}

/// 503 used when a query fails. The driver detail is logged (`context` says
/// which read failed) and the client gets only [`DATABASE_UNAVAILABLE`].
pub fn database_unavailable(context: &str, e: &sqlx::Error) -> ApiError {
    error!(error = %e, context = %context, "database query failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorBody {
            error: DATABASE_UNAVAILABLE.to_string(),
        }),
    )
}

/// Extracts the pool from app state, or the 503 "not configured" response.
pub fn require_pool(app: &AppState) -> Result<&PgPool, ApiError> {
    app.pool.as_ref().ok_or_else(database_not_configured)
}
