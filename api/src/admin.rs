//! Admin console auth (ADMIN-01, commitment 2): stateless HMAC-SHA256 session
//! cookies, the login handler and the middleware guarding `/api/admin`.
//!
//! Why stateless: there is no session table to purge and no server-side state
//! to reconcile. A token carries its own `{sub, exp}` claims plus an HMAC-SHA256
//! signature over the payload bytes, so a stolen cookie cannot be forged or
//! extended — only replayed until it expires (`SESSION_TTL_SECS`). The signing
//! key (`SESSION_SECRET`) is env-only: it never enters the database, the
//! `settings` table or the admin console.
//!
//! Security shape of this module:
//! - login answers "invalid username or password" for BOTH a wrong username
//!   and a wrong password (no account enumeration), and a missing username row
//!   still runs one argon2id verify against a fixed dummy hash so the two
//!   failures cost the same (timing enumeration);
//! - the session cookie is `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` =
//!   the TTL, and `Secure` by default (config knob for plain-HTTP local dev);
//! - non-GET/HEAD/OPTIONS admin requests additionally require a non-empty
//!   `X-Requested-With` header (CSRF: `SameSite=Lax` alone does not cover
//!   top-level cross-site form POSTs);
//! - nothing here logs a password, a hash or the session secret.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{rejection::JsonRejection, Request, State},
    http::{header, HeaderName, Method, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::{error, info, warn};

use crate::config;
use crate::state::{self, ApiError, AppState, ErrorBody};

/// Name of the session cookie. Stable contract with the web tier.
pub const SESSION_COOKIE_NAME: &str = "admin_session";

/// Session lifetime: 7 days, used both as the token's `exp` offset and the
/// cookie's `Max-Age`, so the browser drops the cookie exactly when the
/// signature's claims expire.
pub const SESSION_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Maximum concurrent argon2id verifies during login (work admission). Each
/// verify costs ~19 MiB of memory and a hefty CPU slice; two is plenty for a
/// single-admin console while bounding memory use and keeping argon2 off the
/// async workers that also serve the storefront.
pub const LOGIN_MAX_CONCURRENT: usize = 2;

/// Bound on the boot-time admin upsert. `main` runs it off the serving path,
/// so this only stops a stuck query (e.g. a row-lock wait) from holding a
/// pool connection forever; the storefront is never blocked either way.
pub const BOOTSTRAP_UPSERT_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound on the admin-user lookup in login. The admission permit is held
/// across this query, so the HANDLER's occupancy is what gets bounded here:
/// a stalled DB fails the login closed (503) and releases the permit instead
/// of wedging the gate until the query returns. The dropped query's pooled
/// connection is released by sqlx's background path (what the D2c
/// scratch-Postgres harness proves is pool recovery after DATABASE LOSS —
/// DROP/recreate on the same pool object — plus the boot/login timeout caps
/// against a hung database; NOT query-cancellation recovery of an
/// established connection).
pub const LOGIN_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

type HmacSha256 = Hmac<Sha256>;

/// The signing key for session tokens. Built once at boot from
/// `config::session_secret()`; the bytes stay private so callers must go
/// through the sign/verify helpers rather than rolling their own MAC.
#[derive(Clone)]
pub struct SessionKey(Vec<u8>);

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted on purpose: the module's contract is that nothing logs the
        // session secret, and a derived Debug would print the raw bytes the
        // first time someone writes `dbg!(&key)` or `error!(?key)`.
        f.write_str("SessionKey(<redacted>)")
    }
}

impl SessionKey {
    pub fn new(secret: Vec<u8>) -> Self {
        Self(secret)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Claims carried inside the signed payload. Serialised as JSON
/// (`{"sub":...,"exp":...,"jti":...}`) and hashed verbatim, so the exact
/// payload bytes are what the signature authenticates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Subject: the admin username.
    pub sub: String,
    /// Expiry as a unix timestamp (seconds). Rejected when `exp <= now`.
    pub exp: i64,
    /// Token identity: `timestamp_micros` hex + a process-local monotonic
    /// counter, so two logins in the same second mint DISTINCT tokens (and
    /// therefore distinct revocation hashes). `#[serde(default)]` keeps
    /// pre-`jti` tokens verifiable.
    #[serde(default)]
    pub jti: String,
}

/// The authenticated admin behind a request, inserted into request extensions
/// by [`require_admin`] and read by the protected handlers.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub username: String,
}

// ---------------------------------------------------------------------------
// base64url (RFC 4648 §5, no padding) — hand-rolled on purpose: the `base64`
// crate is not a dependency and a 20-line codec beats pulling one in.
// ---------------------------------------------------------------------------

const B64URL_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Writes the top `count` 6-bit groups of `bits` as base64url characters.
fn push_b64url_chars(out: &mut String, bits: u32, count: usize) {
    for i in 0..count {
        let idx = ((bits >> (18 - 6 * i)) & 0x3f) as usize;
        out.push(B64URL_TABLE[idx] as char);
    }
}

/// Pure base64url encode without padding.
fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let (chunks, rest) = data.as_chunks::<3>();
    for chunk in chunks {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        push_b64url_chars(&mut out, bits, 4);
    }
    // Remainder: 1 byte -> 2 chars, 2 bytes -> 3 chars (no padding per §5).
    let (bits, chars) = match rest {
        [a] => (u32::from(*a) << 16, 2),
        [a, b] => ((u32::from(*a) << 16) | (u32::from(*b) << 8), 3),
        _ => (0, 0),
    };
    push_b64url_chars(&mut out, bits, chars);
    out
}

fn b64url_value(c: u8) -> Option<u8> {
    B64URL_TABLE.iter().position(|&t| t == c).map(|i| i as u8)
}

/// Pure base64url decode without padding. Rejects padding, characters outside
/// the RFC 4648 §5 alphabet and non-canonical tail bits (a leftover encoding
/// of the same bytes must not give a second valid spelling of one token).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let rem = bytes.len() % 4;
    if rem == 1 {
        return None; // one leftover char cannot encode a byte
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3 + 2);
    let (full, tail) = bytes.split_at(bytes.len() - rem);
    for chunk in full.as_chunks::<4>().0 {
        let a = b64url_value(chunk[0])?;
        let b = b64url_value(chunk[1])?;
        let c = b64url_value(chunk[2])?;
        let d = b64url_value(chunk[3])?;
        let bits = (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
        out.extend_from_slice(&[(bits >> 16) as u8, (bits >> 8) as u8, bits as u8]);
    }
    match tail {
        [] => {}
        [a, b] => {
            let a = b64url_value(*a)?;
            let b = b64url_value(*b)?;
            if b & 0x0f != 0 {
                return None; // non-canonical tail bits
            }
            out.push(((u32::from(a) << 2) | (u32::from(b) >> 4)) as u8);
        }
        [a, b, c] => {
            let a = b64url_value(*a)?;
            let b = b64url_value(*b)?;
            let c = b64url_value(*c)?;
            if c & 0x03 != 0 {
                return None; // non-canonical tail bits
            }
            let bits = (u32::from(a) << 10) | (u32::from(b) << 4) | (u32::from(c) >> 2);
            out.extend_from_slice(&[(bits >> 8) as u8, bits as u8]);
        }
        _ => return None,
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Session tokens (pure: no clock, no env — the caller passes `now`).
// ---------------------------------------------------------------------------

/// Signs `payload_bytes` as `base64url(payload) + "." + base64url(HMAC)`.
pub fn sign_session(payload_bytes: &[u8], key: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(payload_bytes);
    let signature = mac.finalize().into_bytes();
    format!(
        "{}.{}",
        base64url_encode(payload_bytes),
        base64url_encode(&signature)
    )
}

/// Verifies a token against `key` and the clock (`exp <= now_unix` is dead).
/// Returns `None` for every failure mode — malformed, bad signature,
/// unparseable claims, expired — without distinguishing them.
pub fn verify_session(token: &str, key: &[u8], now_unix: i64) -> Option<SessionClaims> {
    let (payload_b64, sig_b64) = token.split_once('.')?;
    let payload = base64url_decode(payload_b64)?;
    let claims: SessionClaims = serde_json::from_slice(&payload).ok()?;
    // `exp <= now` is rejected with zero skew tolerance (contract): at the
    // exact expiry second the token is already dead.
    if claims.exp <= now_unix {
        return None;
    }
    let signature = base64url_decode(sig_b64)?;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(&payload);
    // Constant-time comparison (hmac::Mac::verify_slice) over the exact
    // payload bytes that were signed.
    mac.verify_slice(&signature).ok()?;
    Some(claims)
}

/// Monotonic per-process counter behind [`next_session_jti`].
static SESSION_JTI_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Token identity for ONE login: micros-since-epoch hex + the process-local
/// counter. Two logins minted in the same second can therefore never produce
/// the same payload — before `jti`, both minted byte-identical tokens and a
/// logout of one silently revoked the other (and made both share one
/// revocation-ledger row).
fn next_session_jti() -> String {
    let micros = Utc::now().timestamp_micros();
    let seq = SESSION_JTI_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{micros:016x}{seq:04x}")
}

/// Pure builder for the cookie value: signs
/// `{"sub": username, "exp": now + SESSION_TTL_SECS, "jti": ...}`. Shared by
/// the login handler and the contract tests so both mint tokens the same way.
pub fn session_cookie_value(username: &str, key: &[u8], now_unix: i64) -> String {
    let claims = SessionClaims {
        sub: username.to_string(),
        exp: now_unix + SESSION_TTL_SECS,
        jti: next_session_jti(),
    };
    let payload = serde_json::to_vec(&claims).expect("SessionClaims always serialises");
    sign_session(&payload, key)
}

/// SHA-256 of a session token, lowercase hex — what the server-side
/// revocation ledger stores (`revoked_admin_sessions.token_hash`, 0008).
/// A HASH, never the token itself: a leaked ledger can deny sessions but can
/// never resurrect one, and equal tokens hash equal (the lookup is exact).
pub fn token_sha256_hex(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Pure charset/length rule mirroring the `admin_users` CHECK constraint in
/// `0004_admin.sql` (`^[A-Za-z0-9_-]{1,64}$`), so a bad `ADMIN_USERNAME` is
/// warned about at boot instead of surfacing as a SQL error.
pub fn validate_username(username: &str) -> bool {
    (1..=64).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Builds the `Set-Cookie` value: `HttpOnly` (no JS access), `SameSite=Lax`
/// (no cross-site send on subresource/form POST), `Path=/`, `Max-Age` = the
/// TTL, and `Secure` per the config knob (HTTPS-only by default).
fn session_set_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_TTL_SECS}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Builds the LOGOUT `Set-Cookie`: same attribute contract as
/// [`session_set_cookie`] but an empty value and `Max-Age=0`, so the browser
/// drops the cookie exactly the way it drops it at natural expiry. Clearing
/// the cookie is the browser half of logout; the server half is the
/// [`logout`] handler's revocation row.
fn session_clear_cookie(secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Finds `name` in one `Cookie` header value. Pure so the parser is testable
/// without building a request.
fn find_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------
// DTOs and error bodies
// ---------------------------------------------------------------------------

/// `POST /api/admin/login` request. `#[serde(default)]` so a missing field
/// lands in the blank-credentials 400 instead of a serde rejection.
///
/// No derived `Debug`: the struct carries the submitted password, and a
/// derived Debug would print it on the first stray `dbg!` or `?log` (the
/// same hazard `SessionKey`'s redacted Debug exists to close).
#[derive(Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

/// `POST /api/admin/login` success body: `{"ok":true}` (plus the cookie).
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub ok: bool,
}

/// `GET /api/admin/whoami` body: `{"username":...}` from [`CurrentUser`].
#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    pub username: String,
}

fn error_body(status: StatusCode, error: &str) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: error.to_string(),
        }),
    )
}

/// 400: blank or missing credentials — a form problem, not an auth verdict.
fn missing_credentials() -> ApiError {
    error_body(StatusCode::BAD_REQUEST, "username and password required")
}

/// 401: deliberately generic — the same body for a wrong username and a wrong
/// password, so the endpoint cannot be used to enumerate accounts.
fn invalid_credentials() -> ApiError {
    error_body(StatusCode::UNAUTHORIZED, "invalid username or password")
}

/// 503: `SESSION_SECRET` missing or under 32 bytes; admin login is disabled
/// until the operator configures it.
fn sessions_not_configured() -> ApiError {
    error_body(
        StatusCode::SERVICE_UNAVAILABLE,
        "admin sessions not configured: set SESSION_SECRET (>= 32 bytes)",
    )
}

/// 401: no valid session cookie — shared by middleware and handlers.
fn not_authenticated() -> ApiError {
    error_body(StatusCode::UNAUTHORIZED, "not authenticated")
}

/// 403: CSRF guard for non-safe methods (see module docs).
fn missing_x_requested_with() -> ApiError {
    error_body(StatusCode::FORBIDDEN, "missing X-Requested-With header")
}

/// 503: admin auth is not ready — either the boot bootstrap has not resolved
/// yet, or a requested credential rotation FAILED and login stays disabled
/// until a successful boot sync (see `bootstrap_with`). Distinct from "not
/// configured" and from a 401: this is boot state, not a verdict on the
/// credentials.
fn admin_auth_not_ready() -> ApiError {
    error_body(
        StatusCode::SERVICE_UNAVAILABLE,
        "admin auth not ready; retry later",
    )
}

/// 429: the bounded login-verify gate is saturated (see [`login`]); the
/// credentials are not judged — retry shortly.
fn too_many_login_attempts() -> ApiError {
    error_body(
        StatusCode::TOO_MANY_REQUESTS,
        "too many concurrent login attempts; retry shortly",
    )
}

// ---------------------------------------------------------------------------
// Password hashing (argon2id PHC strings)
// ---------------------------------------------------------------------------

/// A fixed argon2id PHC hash (default `Argon2::default()` params) used only to
/// burn the same CPU as a real verify when the username does not exist. The
/// hashed value is meaningless; what matters is that the verify costs the same
/// as one against a real row.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$4laJtqOytMAiWlwIolFtZQ$2RrkElj7e+bgoOaczNdIVmLTy6WFSkBiVBCjCLmB55M";

/// Hashes `password` to an argon2id PHC string with a fresh random salt.
fn hash_password(password: &str) -> argon2::password_hash::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verifies `password` against an argon2id PHC string. A malformed hash is
/// logged (it means data corruption, not a wrong password) and fails closed.
fn verify_password(password: &str, password_hash: &str) -> bool {
    match PasswordHash::new(password_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(e) => {
            error!(error = %e, "stored password hash is not a valid PHC string");
            false
        }
    }
}

/// Runs one login verify on the blocking pool, under its admission permit.
///
/// The permit MOVES INTO the blocking task (reviewer finding): returning it
/// when the handler future is dropped — while the verify still runs — would
/// let cancellation escape the concurrency bound and admit extra argon2
/// work. Owned by the closure, the permit lives exactly as long as the
/// verify.
fn spawn_bounded_verify(
    permit: tokio::sync::OwnedSemaphorePermit,
    password: String,
    stored_hash: Option<String>,
) -> tokio::task::JoinHandle<bool> {
    tokio::task::spawn_blocking(move || {
        // Held for the verify's lifetime, dropped with the closure.
        let _permit = permit;
        match stored_hash.as_deref() {
            Some(stored) => verify_password(&password, stored),
            // Timing equalization: hashing dominates login cost, so an
            // unknown username must burn the same CPU as a wrong password.
            None => {
                let _ = verify_password(&password, DUMMY_PASSWORD_HASH);
                false
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Handlers and middleware
// ---------------------------------------------------------------------------

/// `POST /api/admin/login` — verifies credentials against `admin_users` and,
/// on success, sets the signed `admin_session` cookie. Pool/state failures use
/// the standard endpoint error bodies; credential failures are generic 401s.
pub async fn login(
    State(app): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<([(HeaderName, String); 1], Json<LoginResponse>), ApiError> {
    // CSRF: login mints a bearer credential, so it carries the same custom-
    // header requirement as every other /api/admin state change — a cross-site
    // form cannot set the header, and a cross-site fetch that does is stopped
    // by the CORS origin lock (login has no session middleware to do this).
    if !headers
        .get("x-requested-with")
        .is_some_and(|value| !value.is_empty())
    {
        return Err(missing_x_requested_with());
    }
    let Ok(Json(LoginRequest { username, password })) = body else {
        // Malformed JSON or a bad body type is a form problem, not an auth
        // verdict — same 400 as blank credentials.
        return Err(missing_credentials());
    };
    if username.trim().is_empty() || password.trim().is_empty() {
        return Err(missing_credentials());
    }
    let Some(key) = app.session_key.as_ref() else {
        return Err(sessions_not_configured());
    };
    // Boot race (reviewer finding): until bootstrap marks auth ready — which
    // it does ONLY when no rotation was requested or the requested rotation
    // was applied — no session may be minted against a possibly-stale row.
    // 503, not a 401: this is boot state, not a verdict on the credentials.
    if !app.admin_ready.load(std::sync::atomic::Ordering::Acquire) {
        return Err(admin_auth_not_ready());
    }

    // Work admission BEFORE the pool (a saturated gate must not also hold a
    // DB connection): at most LOGIN_MAX_CONCURRENT argon2 verifies run at
    // once; saturation answers 429 — the credentials are not judged. The
    // permit then MOVES INTO the blocking task (see `spawn_bounded_verify`):
    // dropping the handler future must not return it while the verify still
    // runs, or cancellation would escape the bound.
    let Ok(verify_permit) = app.login_gate.clone().try_acquire_owned() else {
        warn!("admin login saturated (concurrent argon2 verifies at the cap)");
        return Err(too_many_login_attempts());
    };
    let pool = state::require_pool(&app)?;

    // The admission permit is held across this lookup, so the wait is
    // bounded: a stalled DB fails the login closed (503) and releases the
    // permit instead of wedging the gate until the query returns.
    let lookup = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM admin_users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(pool);
    let stored_hash = match tokio::time::timeout(LOGIN_LOOKUP_TIMEOUT, lookup).await {
        Ok(stored_hash) => stored_hash
            .map_err(|e| state::database_unavailable("failed to look up admin user", &e))?,
        Err(_) => {
            error!(
                timeout_secs = LOGIN_LOOKUP_TIMEOUT.as_secs(),
                "admin user lookup timed out"
            );
            return Err(error_body(
                StatusCode::SERVICE_UNAVAILABLE,
                state::DATABASE_UNAVAILABLE,
            ));
        }
    };

    let verify_task = spawn_bounded_verify(verify_permit, password, stored_hash);
    let verified = match verify_task.await {
        Ok(verified) => verified,
        Err(join) => {
            error!(error = %join, "argon2 verify task panicked; failing closed");
            false
        }
    };
    if !verified {
        return Err(invalid_credentials());
    }

    let token = session_cookie_value(&username, key.as_bytes(), Utc::now().timestamp());
    let cookie = session_set_cookie(&token, config::session_cookie_secure());
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(LoginResponse { ok: true }),
    ))
}

/// `GET /api/admin/whoami` — probe proving the middleware chain works:
/// returns the [`CurrentUser`] the session cookie authenticated.
pub async fn whoami(req: Request) -> Result<Json<WhoamiResponse>, ApiError> {
    let user = req
        .extensions()
        .get::<CurrentUser>()
        .ok_or_else(not_authenticated)?;
    Ok(Json(WhoamiResponse {
        username: user.username.clone(),
    }))
}

/// `POST /api/admin/logout` — server-side session revocation (findings G17).
/// The browser-local logout (cookie deletion) is not enough for a bearer
/// token: without this, a stolen cookie keeps working until `exp` (7 days).
/// So the token's SHA-256 is recorded in `revoked_admin_sessions` (0008) —
/// with the token's own expiry — and [`require_admin`] refuses any listed
/// hash from that moment on. The deny-list row is written BEFORE the cookie
/// is cleared; a failed write answers 503 so the operator is never told
/// "logged out" while the key still turns.
///
/// Mounted inside the `admin_protected` router: like every admin mutation it
/// runs behind [`require_admin`] + the CSRF guard.
pub async fn logout(
    State(app): State<AppState>,
    req: Request,
) -> Result<([(HeaderName, String); 1], Json<LoginResponse>), ApiError> {
    let user = req
        .extensions()
        .get::<CurrentUser>()
        .ok_or_else(not_authenticated)?;
    let Some(token) = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| find_cookie(value, SESSION_COOKIE_NAME))
        .map(str::to_owned)
    else {
        return Err(not_authenticated());
    };
    // Re-verify for the `exp` claim: the deny-list row's lifetime IS the
    // token's expiry (after it, the signature alone rejects the token), and
    // `require_admin` has just accepted this exact token, so a `None` here
    // means it crossed `exp` mid-request — an honest 401, not a fake logout.
    let claims = match app.session_key.as_ref() {
        Some(key) => verify_session(&token, key.as_bytes(), Utc::now().timestamp()),
        None => None,
    };
    let Some(claims) = claims else {
        return Err(not_authenticated());
    };
    // Fail closed when the DB pool is absent: without the deny-list write the
    // token still turns, so "logged out" must never be reported — a 503 tells
    // the operator the revocation could NOT be recorded (same contract as
    // every other admin mutation: `require_pool`, never a silent skip).
    let pool = state::require_pool(&app)?;
    // Prune rows that can no longer reject anything (their token's `exp`
    // has passed) — the logout path is the natural sweeper — then record
    // THIS token. `ON CONFLICT (token_hash) DO NOTHING`: logging out
    // twice must keep the first revocation moment.
    sqlx::query("DELETE FROM revoked_admin_sessions WHERE expires_at <= now()")
        .execute(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to prune revoked sessions", &e))?;
    sqlx::query(
        "INSERT INTO revoked_admin_sessions (token_hash, username, expires_at) \
         VALUES ($1, $2, to_timestamp($3)) ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(token_sha256_hex(&token))
    .bind(&user.username)
    .bind(claims.exp as f64)
    .execute(pool)
    .await
    .map_err(|e| state::database_unavailable("failed to revoke session", &e))?;
    info!(username = %user.username, "admin session revoked server-side");
    Ok((
        [(
            header::SET_COOKIE,
            session_clear_cookie(config::session_cookie_secure()),
        )],
        Json(LoginResponse { ok: true }),
    ))
}

/// Middleware for every `/api/admin` route except login: requires a valid
/// `admin_session` cookie (fresh clock, [`verify_session`]), inserts
/// [`CurrentUser`] into request extensions, and for non-GET/HEAD/OPTIONS
/// requests additionally requires a non-empty `X-Requested-With` header.
pub async fn require_admin(
    State(app): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| find_cookie(value, SESSION_COOKIE_NAME))
        .map(str::to_owned);
    let claims = match (token.as_deref(), app.session_key.as_ref()) {
        (Some(token), Some(key)) => verify_session(token, key.as_bytes(), Utc::now().timestamp()),
        _ => None,
    };
    let Some(claims) = claims else {
        return Err(not_authenticated());
    };

    // CSRF: SameSite=Lax keeps cross-site subresource/form sends off the
    // cookie, but top-level navigations still attach it — so state changes
    // additionally demand a header a cross-site form cannot set.
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let has_header = req
            .headers()
            .get("x-requested-with")
            .is_some_and(|value| !value.is_empty());
        if !has_header {
            return Err(missing_x_requested_with());
        }
    }

    // Server-side revocation (findings G17): a logged-out token is dead even
    // while its signature still verifies. Lookup is by HASH — the ledger
    // never stores tokens. Fail-closed on EVERY database error — including a
    // missing `revoked_admin_sessions` table (migration 0008 not applied): a
    // deployment that cannot check revocation must never pretend it checked
    // (a 503, never an admin page built from a possibly-revoked session).
    if let (Some(pool), Some(token)) = (app.pool.as_ref(), token.as_deref()) {
        let revoked = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM revoked_admin_sessions \
             WHERE token_hash = $1 AND expires_at > now())",
        )
        .bind(token_sha256_hex(token))
        .fetch_one(pool)
        .await;
        match revoked {
            Ok(true) => return Err(not_authenticated()),
            Ok(false) => {}
            Err(e) => {
                return Err(state::database_unavailable(
                    "failed to check session revocation",
                    &e,
                ));
            }
        }
    }

    req.extensions_mut().insert(CurrentUser {
        username: claims.sub,
    });
    Ok(next.run(req).await)
}

/// A boot-time credential-rotation request, parsed from the environment by
/// [`bootstrap_admin`].
pub enum Rotation {
    /// No rotation requested: the existing `admin_users` row (if any) IS the
    /// intended credential, and login may open once bootstrap resolves.
    None,
    /// Both `ADMIN_USERNAME` and `ADMIN_PASSWORD` set: upsert requested.
    Requested { username: String, password: String },
    /// Only one set (or a blank password): unusable. Rotation was intended,
    /// so login must stay disabled — never fall back to the stale row the
    /// operator was trying to replace.
    Blocked,
}

/// Boot-time admin bootstrap (called once from `main`, off the serving path).
///
/// Reads `ADMIN_USERNAME`/`ADMIN_PASSWORD`; both set means a credential
/// rotation is requested and argon2id-hashed into `admin_users` (`ON
/// CONFLICT` updates the hash, so a changed password takes effect on
/// restart). The username is logged; the password and its hash never are.
/// `SESSION_SECRET` missing/weak warns (login is disabled by its own check).
///
/// `ready` marks login open ONLY when no rotation was requested or the
/// requested rotation was applied (reviewer finding): a requested-but-FAILED
/// rotation must leave login disabled — falling back to the stale credential
/// the rotation was meant to replace is exactly the risk rotation closes.
/// Nothing here ever blocks the storefront.
///
/// Boundary, kept explicit: an applied rotation upserts the NAMED row only —
/// changing `ADMIN_USERNAME` from A to B leaves A's row in place. Revoking a
/// previous username is a manual SQL / future admin-UI step (D2b).
pub async fn bootstrap_admin(
    pool: Option<&PgPool>,
    session_key: Option<&SessionKey>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let rotation = parse_rotation(
        std::env::var("ADMIN_USERNAME"),
        std::env::var("ADMIN_PASSWORD"),
    );
    bootstrap_with(pool, session_key, rotation, ready).await;
}

/// Classifies the raw `ADMIN_USERNAME`/`ADMIN_PASSWORD` env results into a
/// [`Rotation`]. Presence is kept distinct from validity (reviewer finding):
/// `.ok()` + `filter` would collapse "unset" with "set but blank or
/// non-Unicode" and silently treat a botched rotation as NO rotation —
/// re-enabling the stale credential the rotation was meant to replace.
/// Any present-but-unusable value classifies as [`Rotation::Blocked`]: a
/// rotation was intended, login stays disabled this boot.
fn parse_rotation(
    username: Result<String, std::env::VarError>,
    password: Result<String, std::env::VarError>,
) -> Rotation {
    let username = match username {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        // Non-Unicode is present-but-unusable, NOT absent.
        Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
    };
    let password = match password {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
    };
    match (username, password) {
        // Genuinely unset: the existing row (if any) IS the credential.
        (None, None) => Rotation::None,
        (Some(username), Some(password))
            if !username.trim().is_empty() && !password.trim().is_empty() =>
        {
            Rotation::Requested { username, password }
        }
        _ => Rotation::Blocked,
    }
}

/// The actual bootstrap with an injected rotation request (pure, unit-testable
/// without process env — env reads race parallel tests). Warns on the missing
/// session key, applies the rotation, and marks `ready` per the contract in
/// [`bootstrap_admin`]. One-shot boot wiring: this function only ever stores
/// `true`, so callers must pass a fresh `false` flag.
pub async fn bootstrap_with(
    pool: Option<&PgPool>,
    session_key: Option<&SessionKey>,
    rotation: Rotation,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    if session_key.is_none() {
        warn!("SESSION_SECRET missing or under 32 bytes; admin login is disabled");
    }
    if rotate_admin_credentials(pool, rotation).await {
        ready.store(true, std::sync::atomic::Ordering::Release);
    } else {
        error!(
            "admin credential rotation requested but NOT applied; admin login \
             stays disabled until a successful boot sync (fix the cause and restart)"
        );
    }
}

/// Applies one rotation request; `true` when login may open (no rotation was
/// requested, or the requested upsert succeeded). Never logs the password or
/// its hash.
async fn rotate_admin_credentials(pool: Option<&PgPool>, rotation: Rotation) -> bool {
    let (username, password) = match rotation {
        // The existing row IS the credential: login may open.
        Rotation::None => return true,
        Rotation::Blocked => {
            warn!(
                "ADMIN_USERNAME and ADMIN_PASSWORD must both be set (nonblank) \
                 for a credential rotation; admin login stays disabled this boot"
            );
            return false;
        }
        Rotation::Requested { username, password } => (username, password),
    };
    if !validate_username(&username) {
        warn!(
            username = %username,
            "ADMIN_USERNAME does not match [A-Za-z0-9_-]{{1,64}}; admin login stays disabled this boot"
        );
        return false;
    }
    let Some(pool) = pool else {
        warn!(
            "ADMIN_USERNAME/ADMIN_PASSWORD set but DATABASE_URL is not; admin login stays disabled this boot"
        );
        return false;
    };
    // Hashing is blocking argon2 work: the blocking pool keeps it off the
    // async workers that also serve the storefront.
    let hash_result = tokio::task::spawn_blocking(move || hash_password(&password)).await;
    let password_hash = match hash_result {
        Ok(Ok(hash)) => hash,
        Ok(Err(e)) => {
            error!(error = %e, "failed to hash the admin password");
            return false;
        }
        Err(join) => {
            error!(error = %join, "password hash task panicked");
            return false;
        }
    };

    let upsert = sqlx::query(
        "INSERT INTO admin_users (username, password_hash) VALUES ($1, $2) \
         ON CONFLICT (username) DO UPDATE SET password_hash = excluded.password_hash",
    )
    .bind(&username)
    .bind(&password_hash)
    .execute(pool);
    // `main` spawns this whole path off the serving path (see there); the
    // timeout just stops a stuck query from holding a pool connection forever.
    match tokio::time::timeout(BOOTSTRAP_UPSERT_TIMEOUT, upsert).await {
        Ok(Ok(_)) => {
            info!(
                username = %username,
                "admin user upserted from ADMIN_USERNAME/ADMIN_PASSWORD"
            );
            true
        }
        Ok(Err(e)) => {
            error!(
                error = %e,
                "failed to upsert the admin user; admin login stays disabled this boot"
            );
            false
        }
        Err(_) => {
            error!(
                timeout_secs = BOOTSTRAP_UPSERT_TIMEOUT.as_secs(),
                "admin user upsert timed out; the storefront is unaffected, \
                 admin login stays disabled this boot"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &[u8] = b"unit-test-session-secret-0123456789abcdef";
    const NOW: i64 = 1_700_000_000;

    // -- base64url ---------------------------------------------------------

    #[test]
    fn base64url_matches_rfc4648_vectors() {
        // RFC 4648 §10 vectors; none exercise chars 62/63 so the URL-safe
        // encoding is identical here.
        assert_eq!(base64url_encode(b""), "");
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
        assert_eq!(base64url_encode(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64url_uses_the_url_safe_alphabet() {
        // "hello world" -> standard base64 "aGVsbG8gd29ybGQ=" -> unpadded,
        // no chars 62/63, so the spec vector holds verbatim.
        assert_eq!(base64url_encode(b"hello world"), "aGVsbG8gd29ybGQ");
        // 0xfb 0xff 0xbf exercises chars 62/63 (bit groups 62,63,62,63):
        // standard base64 is "+/+/", base64url swaps '+'->'-' and '/'->'_'.
        assert_eq!(base64url_encode(&[0xfb, 0xff, 0xbf]), "-_-_");
    }

    #[test]
    fn base64url_decode_roundtrips_and_rejects_junk() {
        assert_eq!(base64url_decode("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
        assert_eq!(base64url_decode("-_-_").unwrap(), [0xfb, 0xff, 0xbf]);
        assert_eq!(base64url_decode("Zg").unwrap(), b"f");
        assert_eq!(base64url_decode("").unwrap(), Vec::<u8>::new());
        assert!(base64url_decode("a").is_none()); // 1 leftover char
        assert!(base64url_decode("Zg=").is_none()); // padding is not in the alphabet
        assert!(base64url_decode("Zg==").is_none());
        assert!(base64url_decode("+/7+").is_none()); // standard, not url-safe
        assert!(base64url_decode("Zm9v!").is_none()); // junk char
                                                      // Arbitrary bytes roundtrip through both directions.
        for data in [
            &b""[..],
            b"a",
            b"ab",
            b"abc",
            &[0xfb, 0xff, 0xbf],
            &[0, 0, 0],
        ] {
            assert_eq!(base64url_decode(&base64url_encode(data)).unwrap(), data);
        }
    }

    #[test]
    fn base64url_rejects_non_canonical_tail_bits() {
        // Same decoded byte, different (non-canonical) spelling must not be a
        // second valid encoding of one token.
        assert!(base64url_decode("Zh").is_none()); // "Zg" is canonical for b"f"
        assert!(base64url_decode("Zm9").is_none()); // "Zm8" is canonical for b"fo"
    }

    // -- session tokens ----------------------------------------------------

    #[test]
    fn signed_session_roundtrips_sub_and_exp() {
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let claims = verify_session(&token, TEST_KEY, NOW).expect("valid token");
        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.exp, NOW + SESSION_TTL_SECS);
        // Token shape: payload.signature, both base64url.
        assert_eq!(token.matches('.').count(), 1);
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let (payload, sig) = token.split_once('.').unwrap();
        // Flip one payload character (still base64url) without re-signing.
        let flipped = match payload.chars().next().unwrap() {
            'A' => format!("B{}", &payload[1..]),
            _ => format!("A{}", &payload[1..]),
        };
        let forged = format!("{flipped}.{sig}");
        assert!(verify_session(&forged, TEST_KEY, NOW).is_none());
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let (payload, sig) = token.split_once('.').unwrap();
        let flipped = match sig.chars().next().unwrap() {
            'A' => format!("B{}", &sig[1..]),
            _ => format!("A{}", &sig[1..]),
        };
        assert!(verify_session(&format!("{payload}.{flipped}"), TEST_KEY, NOW).is_none());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let other = b"unit-test-session-secret-ZYXWVUTSRQPONMLK";
        assert!(verify_session(&token, other, NOW).is_none());
    }

    #[test]
    fn expired_session_is_rejected() {
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        assert!(verify_session(&token, TEST_KEY, NOW + SESSION_TTL_SECS + 1).is_none());
    }

    #[test]
    fn exp_equal_to_now_is_rejected() {
        // Boundary: `exp <= now` is dead, zero skew tolerance.
        let payload = serde_json::to_vec(&SessionClaims {
            sub: "admin".to_string(),
            exp: NOW,
            jti: String::new(),
        })
        .unwrap();
        let token = sign_session(&payload, TEST_KEY);
        assert!(verify_session(&token, TEST_KEY, NOW).is_none());
    }

    #[test]
    fn future_exp_is_accepted() {
        let payload = serde_json::to_vec(&SessionClaims {
            sub: "admin".to_string(),
            exp: NOW + 1,
            jti: String::new(),
        })
        .unwrap();
        let token = sign_session(&payload, TEST_KEY);
        assert_eq!(verify_session(&token, TEST_KEY, NOW).unwrap().exp, NOW + 1);
    }

    #[test]
    fn session_cookie_value_carries_sub_and_seven_day_ttl() {
        let token = session_cookie_value("ops_user-1", TEST_KEY, NOW);
        let claims = verify_session(&token, TEST_KEY, NOW).unwrap();
        assert_eq!(claims.sub, "ops_user-1");
        assert_eq!(claims.exp - NOW, 7 * 24 * 60 * 60);
        assert_eq!(SESSION_TTL_SECS, 7 * 24 * 60 * 60);
        assert!(!claims.jti.is_empty(), "every fresh token carries a jti");
    }

    #[test]
    fn two_logins_in_the_same_second_mint_distinct_tokens() {
        // Before `jti`, two logins in the same second produced byte-identical
        // tokens — logging one out revoked the other, and both shared one
        // revocation-ledger row.
        let a = session_cookie_value("admin", TEST_KEY, NOW);
        let b = session_cookie_value("admin", TEST_KEY, NOW);
        assert_ne!(a, b, "same-second logins must never share one token");
        let claims_a = verify_session(&a, TEST_KEY, NOW).unwrap();
        let claims_b = verify_session(&b, TEST_KEY, NOW).unwrap();
        assert_ne!(claims_a.jti, claims_b.jti);
    }

    #[test]
    fn pre_jti_tokens_still_verify() {
        // Tokens minted before the `jti` claim existed must stay valid until
        // their own `exp` (the claim is identity, never an auth gate).
        let payload = format!("{{\"sub\":\"admin\",\"exp\":{}}}", NOW + 60).into_bytes();
        let token = sign_session(&payload, TEST_KEY);
        let claims = verify_session(&token, TEST_KEY, NOW).expect("legacy token must verify");
        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.jti, "");
    }

    // -- usernames ---------------------------------------------------------

    #[test]
    fn valid_usernames() {
        // Mirrors the 0004 CHECK `^[A-Za-z0-9_-]{1,64}$`.
        assert!(validate_username("admin"));
        assert!(validate_username("a"));
        assert!(validate_username("9"));
        assert!(validate_username("Ops_User-1"));
        assert!(validate_username(&"x".repeat(64))); // max length
    }

    #[test]
    fn invalid_usernames() {
        assert!(!validate_username(""));
        assert!(!validate_username(&"x".repeat(65))); // too long
        assert!(!validate_username("has space"));
        assert!(!validate_username("dot.name"));
        assert!(!validate_username("admin!"));
        assert!(!validate_username("phé5")); // non-ASCII
    }

    // -- password hashing --------------------------------------------------

    #[test]
    fn password_hash_roundtrips_and_rejects_wrong_password() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(phc.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &phc));
        assert!(!verify_password("correct horse battery stapl", &phc));
        assert!(!verify_password("", &phc));
        // Garbage hashes fail closed instead of panicking.
        assert!(!verify_password("anything", "not-a-phc-string"));
    }

    #[test]
    fn dummy_password_hash_is_a_valid_phc_string() {
        // The timing-equalization verify must actually parse and cost real
        // argon2 work; a typo here would make it a cheap no-op.
        let parsed = PasswordHash::new(DUMMY_PASSWORD_HASH).expect("dummy hash parses");
        assert_eq!(parsed.algorithm.as_str(), "argon2id");
        // Equalization assumes a DEFAULT-COST argon2id row (verify honours
        // the stored PHC parameters): pin version, default costs
        // (m=19456 KiB, t=2, p=1) and present salt/output, so the dummy verify
        // burns exactly the work of one against a bootstrap-hashed row.
        assert_eq!(parsed.version, Some(19));
        let params = argon2::Params::try_from(&parsed).expect("argon2 params parse");
        assert_eq!(params.m_cost(), 19_456);
        assert_eq!(params.t_cost(), 2);
        assert_eq!(params.p_cost(), 1);
        assert!(parsed.salt.expect("dummy hash has a salt").as_str().len() >= 16);
        assert_eq!(parsed.hash.expect("dummy hash has output").len(), 32);
        assert!(!verify_password(
            "a password that never matches",
            DUMMY_PASSWORD_HASH
        ));
    }

    // -- cookie header parsing ---------------------------------------------

    #[test]
    fn set_cookie_has_the_contract_flags() {
        let cookie = session_set_cookie("TOKEN", true);
        assert!(cookie.starts_with("admin_session=TOKEN; "));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("Max-Age=604800"));
        assert!(cookie.ends_with("; Secure"));
        assert!(!session_set_cookie("TOKEN", false).contains("Secure"));
    }

    #[test]
    fn clear_cookie_drops_the_session_with_the_contract_flags() {
        // Logout must drop the cookie under the SAME attributes it was set
        // with (an attribute-mismatched clear leaves the cookie in place).
        let cookie = session_clear_cookie(true);
        assert_eq!(
            cookie,
            "admin_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0; Secure"
        );
        assert_eq!(
            session_clear_cookie(false),
            "admin_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
        );
    }

    #[test]
    fn token_sha256_hex_matches_known_answers() {
        // Known-answer vectors (FIPS 180 / python hashlib) pin the exact
        // digest + hex spelling — a self-consistent round trip could not
        // catch a consistently wrong construction that the DB lookup would
        // then share with the writer.
        assert_eq!(
            token_sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            token_sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // The revocation lookup is exact: distinct tokens hash distinctly.
        assert_ne!(token_sha256_hex("abc"), token_sha256_hex("abd"));
    }

    #[test]
    fn find_cookie_finds_the_session_cookie() {
        assert_eq!(
            find_cookie("admin_session=abc.def; other=1", SESSION_COOKIE_NAME),
            Some("abc.def")
        );
        assert_eq!(
            find_cookie("a=1; admin_session=xyz; b=2", SESSION_COOKIE_NAME),
            Some("xyz")
        );
        assert_eq!(
            find_cookie("  admin_session = spaced ", SESSION_COOKIE_NAME),
            Some("spaced")
        );
        assert_eq!(find_cookie("a=1; b=2", SESSION_COOKIE_NAME), None);
        assert_eq!(find_cookie("admin_session=", SESSION_COOKIE_NAME), None);
        // A cookie named with a prefix must not match.
        assert_eq!(find_cookie("xadmin_session=abc", SESSION_COOKIE_NAME), None);
    }

    #[test]
    fn independently_computed_token_verifies() {
        // Interop vector minted with Node's crypto (NOT this code) over the
        // same key and payload bytes — a second implementation pinning the
        // token format, including the URL-safe alphabet (- _) in the
        // signature half.
        const NODE_TOKEN: &str = "eyJzdWIiOiJhZG1pbiIsImV4cCI6MTcwMDYwNDgwMH0.7lDDC7W6DeVTXp0jb-xZvdcHyyDd2Cwn-MBJO0pOIfY";
        let claims = verify_session(NODE_TOKEN, TEST_KEY, NOW).expect("node-minted token verifies");
        assert_eq!(claims.sub, "admin");
        assert_eq!(claims.exp, NOW + SESSION_TTL_SECS);
        assert!(verify_session(
            NODE_TOKEN,
            b"wrong-key-0123456789abcdef0123456789abcdef",
            NOW
        )
        .is_none());
    }

    #[test]
    fn forged_claims_with_original_signature_rejected() {
        // Valid JSON, DIFFERENT claims, original signature: the HMAC covers
        // the exact payload bytes, so parsable-but-altered claims are still a
        // forgery. (The existing tamper test flips a byte into invalid JSON —
        // that pins the claims parse, not the MAC.)
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let (_, sig) = token.split_once('.').unwrap();
        let forged = format!(
            "{}.{sig}",
            base64url_encode(br#"{"sub":"attacker","exp":4102444800}"#)
        );
        assert!(verify_session(&forged, TEST_KEY, NOW).is_none());
    }

    #[test]
    fn reformatted_payload_with_original_signature_rejected() {
        // Same claims, different JSON spelling (reordered keys): rejection
        // pins exact-byte signing rather than claims equality.
        let token = session_cookie_value("admin", TEST_KEY, NOW);
        let (_, sig) = token.split_once('.').unwrap();
        let alt = format!("{{\"exp\":{},\"sub\":\"admin\"}}", NOW + SESSION_TTL_SECS);
        let reformatted = format!("{}.{sig}", base64url_encode(alt.as_bytes()));
        assert!(verify_session(&reformatted, TEST_KEY, NOW).is_none());
    }

    #[tokio::test]
    async fn cancelled_login_does_not_release_the_verify_permit_early() {
        // Cancellation cannot escape the concurrency bound (reviewer
        // finding): dropping the caller (here: the JoinHandle) must NOT
        // return the gate permit while the blocking verify still runs — the
        // permit moved into the task at spawn time and lives exactly as long
        // as the verify.
        //
        // A high-cost PHC (t=16, 8x the default t=2) stretches the verify to
        // ~half a second, so the post-drop saturation assertion has a margin
        // of orders of magnitude over scheduling noise — a true start/end
        // barrier would need test hooks in this security-sensitive path.
        // (The hash is bogus; argon2 pays the t=16 cost regardless of the
        // verdict.)
        const SLOW_HASH: &str = "$argon2id$v=19$m=19456,t=16,p=1$4laJtqOytMAiWlwIolFtZQ$2RrkElj7e+bgoOaczNdIVmLTy6WFSkBiVBCjCLmB55M";
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = gate
            .clone()
            .try_acquire_owned()
            .expect("one permit is free");
        let task = spawn_bounded_verify(
            permit,
            "correct horse battery staple".into(),
            Some(SLOW_HASH.to_string()),
        );
        drop(task); // "handler" cancelled before awaiting
                    // The closure owns the permit from spawn_blocking time; the slow
                    // verify keeps it in flight well past this line.
        assert!(
            gate.clone().try_acquire_owned().is_err(),
            "in-flight verify holds the permit after caller drop"
        );
        // The permit returns when the verify ends: poll (up to 30s) rather
        // than guess a sleep on loaded machines.
        let mut released = false;
        for _ in 0..3000 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if gate.clone().try_acquire_owned().is_ok() {
                released = true;
                break;
            }
        }
        assert!(released, "permit released once the verify completes");
    }

    // -- bootstrap readiness: a requested-but-FAILED rotation must NOT
    //    re-enable the stale credential it was meant to replace
    //    (DB-backed success/error/timeout outcomes land with the D2c
    //    scratch-Postgres migration harness.) ---------------------------

    async fn ready_after(rotation: Rotation, pool: Option<&PgPool>) -> bool {
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        bootstrap_with(pool, None, rotation, ready.clone()).await;
        ready.load(std::sync::atomic::Ordering::Acquire)
    }

    #[tokio::test]
    async fn no_rotation_requested_opens_login() {
        // Both env vars unset: the existing row is the credential; ready.
        assert!(ready_after(Rotation::None, None).await);
    }

    #[tokio::test]
    async fn partial_rotation_request_blocks_login() {
        // One var set / blank password: rotation intended but unusable —
        // fail closed, never fall back to the row being replaced.
        assert!(!ready_after(Rotation::Blocked, None).await);
    }

    #[tokio::test]
    async fn rotation_without_database_blocks_login() {
        // Rotation requested but no DATABASE_URL: not applied, login stays
        // disabled this boot (fail closed).
        assert!(
            !ready_after(
                Rotation::Requested {
                    username: "admin".to_string(),
                    password: "correct horse battery staple".to_string(),
                },
                None,
            )
            .await
        );
    }

    #[tokio::test]
    async fn rotation_with_invalid_username_blocks_login() {
        // The rotation cannot name a legal row: not applied, login stays
        // disabled this boot.
        assert!(
            !ready_after(
                Rotation::Requested {
                    username: "has space".to_string(),
                    password: "correct horse battery staple".to_string(),
                },
                None,
            )
            .await
        );
    }

    #[test]
    fn parse_rotation_keeps_presence_distinct_from_validity() {
        // The raw env classifier (reviewer finding): `.ok()` + filter would
        // collapse unset-with-blank into "no rotation" and re-enable a stale
        // credential. Every present-but-unusable spelling must classify as
        // Blocked — a rotation was intended, login stays disabled this boot.
        use std::env::VarError;
        use std::ffi::OsString;
        let unset = Err(VarError::NotPresent);
        // Genuinely unset: the existing row IS the credential.
        assert!(matches!(
            parse_rotation(unset.clone(), unset.clone()),
            Rotation::None
        ));
        // Both usable: rotation requested.
        assert!(matches!(
            parse_rotation(Ok("admin".to_string()), Ok("correct horse".to_string())),
            Rotation::Requested { .. }
        ));
        // Present-but-unusable combinations must NOT classify as None.
        for (label, username, password) in [
            ("empty username", Ok(String::new()), unset.clone()),
            ("blank username", Ok("   ".to_string()), unset.clone()),
            ("empty password", unset.clone(), Ok(String::new())),
            ("blank password", unset.clone(), Ok("   ".to_string())),
            (
                "empty username + valid password",
                Ok(String::new()),
                Ok("pw".to_string()),
            ),
            (
                "valid username + blank password",
                Ok("admin".to_string()),
                Ok(String::new()),
            ),
            (
                "non-Unicode username",
                Err(VarError::NotUnicode(OsString::new())),
                unset.clone(),
            ),
            (
                "non-Unicode password",
                unset.clone(),
                Err(VarError::NotUnicode(OsString::new())),
            ),
        ] {
            assert!(
                matches!(parse_rotation(username, password), Rotation::Blocked),
                "{label}: present-but-unusable env must classify as Blocked"
            );
        }
    }
}
