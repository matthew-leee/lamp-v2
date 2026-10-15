//! D2c step 5 — admin resilience suite: the outage/timeout half of the admin
//! auth contract, plus the API leg of the inactive-SKU story.
//!
//! What these tests pin down (ALL against the actual code contracts, read
//! from `api/src/{admin,state,catalogue,admin_crud}.rs` before writing — the
//! exact bodies below are the guarantees, asserted byte-for-byte):
//!
//! * `login_outage_503_then_pool_recovery_restores_login` — the pool-recovery
//!   proof, ONE flow on ONE ephemeral database and ONE pool object: baseline
//!   login 200 -> `DROP DATABASE ... WITH (FORCE)` (severs the state pool's
//!   connections, a simulated outage) -> right-credential logins answer 503
//!   `{"error":"database unavailable"}` (the state layer's GENERIC body —
//!   never driver detail) -> CREATE DATABASE + migrations 0001..0004 on the
//!   SAME test pool + a re-run `bootstrap_with` -> login 200 + Set-Cookie
//!   again. Nothing restarts: sqlx reconnects lazily (see
//!   `common::ScratchDb::drop_database_with_force`), so the recovery leg is
//!   the SAME router, state and pool handle that saw the outage.
//! * `login_lookup_timeout_caps_a_hung_database` — `LOGIN_LOOKUP_TIMEOUT`
//!   (10s) bounds the credential lookup against a HUNG database (a
//!   test-spawned loopback "blackhole" TCP listener that accepts connections
//!   and never completes a Postgres handshake): the request must answer 503
//!   `{"error":"database unavailable"}` — the IDENTICAL body an errored
//!   lookup gets (a timed-out lookup and a dead database are deliberately
//!   indistinguishable to the client) — within ~one timeout, never hanging.
//! * `bootstrap_upsert_success_marks_admin_auth_ready` — the happy leg of the
//!   boot upsert, asserted explicitly: `bootstrap_with` + a fresh ready flag
//!   -> `admin_users` row exists (direct SQL) -> the stored hash passes the
//!   `validate_stored_phc` witness: a valid, VERIFIABLE argon2id PHC (the
//!   parse-only checks argon2's verifier needs — argon2id id, an explicit
//!   argon2-SUPPORTED version, coherent m/t/p params, a salt that genuinely
//!   Base64-decodes to argon2's minimum decoded length, output present;
//!   nothing hashes or verifies) -> `admin_ready` true.
//! * `bootstrap_upsert_error_fails_fast_and_login_stays_disabled` — the boot
//!   upsert against a LOST database fails FAST (a direct query through the
//!   same pool first witnesses the error CLASS — a sqlx DATABASE error with
//!   SQLSTATE 3D000, the immediate missing-database answer; the wall-clock
//!   bound alone cannot tell that from the scratch pool's own 5s acquire
//!   timeout — and then the upsert takes no 30s wait) -> `admin_ready`
//!   STAYS false
//!   (fail closed) -> login then answers the fail-closed 503
//!   `{"error":"admin auth not ready; retry later"}` (NOT a 401 — boot state,
//!   not a credential verdict; the ready-flag check runs BEFORE any pool
//!   use, which is exactly the fail-closed ordering).
//! * `bootstrap_upsert_timeout_is_capped_and_login_stays_disabled` — the boot
//!   upsert against the same hung-database shape is capped by
//!   `BOOTSTRAP_UPSERT_TIMEOUT` (30s): `bootstrap_with` resolves (no hang)
//!   -> `admin_ready` STAYS false -> login answers the same fail-closed 503.
//!   This test costs ~30s by design (one test, one cap).
//! * `inactive_sku_absent_from_public_catalogue_but_visible_to_admin` — the
//!   API leg of the inactive-SKU revalidation story: `PUT
//!   /api/admin/products/{id}` with a PARTIAL body (`{"active":false}` —
//!   the product update is a PUT-with-partial-body, never a PATCH) on a
//!   seeded product makes its SKU ABSENT from the public `GET /api/catalogue`
//!   feed (asserted by slug, with a
//!   non-vacuous control slug still served) while `GET /api/admin/products`
//!   still lists the row with `active=false` (the admin reader shows
//!   everything); `active=true` brings the SKU back. This pins the DB-layer
//!   half only — the storefront feed stops including it; the web-tier
//!   last-good-page and rebuild-retires legs are separately owned.
//! * `phc_witness_rejects_unsupported_version` /
//!   `phc_witness_rejects_salt_that_does_not_base64_decode` /
//!   `phc_witness_rejects_salt_below_argon2_minimum_decoded_length` /
//!   `phc_witness_accepts_structurally_valid_supported_phc` — network-free
//!   PURE drivers for `validate_stored_phc` (the witness behind the success
//!   leg above): the round-2 false positives are pinned as REJECTED
//!   (`$argon2id$v=999$...` — syntactically decimal but unsupported version;
//!   salt `....` — charset-legal but not Base64-decodable; salt `AAAAAAAA`
//!   — decodes to 6 bytes, under argon2's `MIN_SALT_LEN` of 8) and a
//!   hand-written valid v=19 argon2id PHC with a real 16-byte salt is
//!   accepted (it only has to PARSE — no hash is computed). Rejection
//!   messages are asserted to name their own gate (version vs salt).
//!
//! CONTRACT NOTES where the actual code is worth pinning next to the sketch:
//!
//! * `POST /api/admin/login` is mounted OUTSIDE the `require_admin` middleware
//!   (it mints the credential) but the HANDLER itself enforces the CSRF
//!   custom-header rule (`X-Requested-With`, 403 without it) — every request
//!   here carries it via `common::admin_request`.
//! * The product update route is `PUT /api/admin/products/{id}` with a
//!   PARTIAL body (`UpdateProduct`, `None` = unchanged) — there is no PATCH
//!   verb on products (`PATCH` exists only for `/api/admin/pieces/{piece_no}`).
//! * A dropped database is not "connection refused": the scratch SERVER is
//!   up, so the driver reports `database ... does not exist` (SQLSTATE
//!   3D000) — which sqlx's connect-phase retry does NOT cover (only
//!   ConnectionRefused and transient 53300/57P03 retry), hence the error leg
//!   fails FAST. The client-visible body is the generic
//!   `database unavailable` either way.
//! * "No audit rows from the failed logins" cannot be read DURING the
//!   outage — the outage destroys the audit table itself. The witness is
//!   therefore: audit_log is empty after a SUCCESSFUL login (the login path
//!   never audits), the recreated database's audit_log is empty (nothing from
//!   the outage could land), and it is STILL empty after the recovery login.
//!
//! Constraints honored: no env mutation (guards read env read-only), no live
//! services (the blackhole is a test-spawned loopback listener that speaks
//! no protocol at all), argon2 budget kept minimal (real logins only where
//! the login IS the subject: 2 verifies in the recovery flow; every boot/
//! re-boot costs one argon2 hash; the timeout tests reach no verify at all; the success-leg PHC witness
//! and the pure witness fixtures only parse).
//! Every test gates on `common::scratch_db_url()` — the two blackhole legs
//! and the four PHC-witness fixtures need no database (the fixtures are
//! pure string checks), but the binary is ONE suite and skips as a whole
//! without the scratch server armed (one loud banner, no partial runs), so
//! plain `cargo test` without the env stays uniformly quiet and green —
//! the pure fixtures gate with the binary anyway, for that uniformity.
//! Tests are plain `#[test]` fns driving the harness runtime through
//! `ScratchDb::block_on` (see `common` for why), except the two blackhole
//! tests which own no `ScratchDb` (no `Drop` to deadlock) and are plain
//! `#[tokio::test]`s.

mod common;

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use argon2::password_hash::Salt;
use argon2::{Algorithm, Params, PasswordHash, Version, MIN_SALT_LEN};
use axum::http::{header, Method, StatusCode};
use axum::routing::get;
use axum::Router;
use lamp_api::admin::{self, Rotation, SessionKey};
use lamp_api::catalogue;
use lamp_api::state::AppState;
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
use tokio::sync::Semaphore;

use common::{
    admin_request, admin_router, audit_rows, boot_admin, minted_session_cookie, product_row, send,
    ScratchDb, ADMIN_PASSWORD, ADMIN_USERNAME, SESSION_SECRET,
};

// The exact bodies the handlers guarantee (D2b/D2c wording, asserted
// byte-for-byte — a fuzzy `contains` would hide drift):
//
// * `state::DATABASE_UNAVAILABLE` — the generic 503 body for ANY database
//   failure, error or lookup timeout alike.
// * `admin::admin_auth_not_ready()` — the fail-closed 503 for login while
//   boot state is unresolved.
// * the login success body.
const DATABASE_UNAVAILABLE_503: &str = r#"{"error":"database unavailable"}"#;
const ADMIN_AUTH_NOT_READY_503: &str = r#"{"error":"admin auth not ready; retry later"}"#;
const LOGIN_OK: &str = r#"{"ok":true}"#;

/// A seeded SKU (0003) that the catalogue-proof test deactivates and
/// reactivates.
const SEEDED_SLUG: &str = "panthella-160-v3-beige";
/// A second seeded SKU: the non-vacuous control — the public feed must keep
/// serving the REST of the drop while `SEEDED_SLUG` is absent.
const OTHER_SLUG: &str = "ph-2-1-table-chrome";

/// Timing policy (CI-jitter-proof): the cap tests bound the waited time from
/// BOTH sides and never pin exact seconds — the EVENT (bounded wait ->
/// generic 503) is what is asserted. The LOWER bound (lookup 5s ≈ half its
/// 10s cap; bootstrap 25s, just under its 30s cap because that measurement
/// also includes one argon2 hash before the capped upsert) proves THE CAP
/// FIRED — it separates "waited out the code's own bound" from "the
/// blackhole broke and the request failed instantly". The UPPER bound
/// (≈ 1.5x the cap: 15s / 45s) proves the request NEVER RAN PAST the cap.
const LOOKUP_CAP: Duration = admin::LOGIN_LOOKUP_TIMEOUT; // 10s
const LOOKUP_LOWER: Duration = Duration::from_secs(5);
const LOOKUP_UPPER: Duration = Duration::from_secs(15);
const BOOTSTRAP_CAP: Duration = admin::BOOTSTRAP_UPSERT_TIMEOUT; // 30s
const BOOTSTRAP_LOWER: Duration = Duration::from_secs(25);
const BOOTSTRAP_UPPER: Duration = Duration::from_secs(45);
/// The error leg must fail FAST (connection-level error, not a cap): well
/// under `BOOTSTRAP_UPSERT_TIMEOUT`, generous for slow CI.
const BOOTSTRAP_ERROR_UPPER: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Hung-database ("blackhole") fixture
// ---------------------------------------------------------------------------

/// Spawns the hung-database fixture: a loopback TCP listener that ACCEPTS
/// connections and then never reads or writes — a Postgres handshake that
/// never completes, which is exactly what a wedged database looks like to a
/// client. Each accepted socket is HELD open (dropping it would answer the
/// handshake with EOF and turn the fixture into a fast failure).
///
/// Lifetime is RAII, not detached: the returned [`Blackhole`] guard owns the
/// fixture for the test's lifetime, and its `Drop` stops the accept thread
/// and closes the listener and every held socket before the test ends (a
/// detached thread + held sockets would otherwise outlive the test and live
/// until process exit — harmless, but unbounded). Bind the guard FIRST in a
/// test so it drops LAST: it must outlive the pool/state built on
/// [`Blackhole::addr`]. No live service is involved — the listener is
/// spawned by this test binary on `127.0.0.1:0`.
fn spawn_blackhole() -> Blackhole {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("blackhole listener must bind a loopback port");
    let addr = listener
        .local_addr()
        .expect("blackhole listener has an address");
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    let accept_thread = std::thread::spawn(move || {
        let mut held: Vec<TcpStream> = Vec::new();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if thread_shutdown.load(Ordering::Acquire) {
                        // The `Drop` wake connection: stop the loop and hold
                        // nothing (this socket is teardown plumbing, not a
                        // probe the fixture must keep open).
                        break;
                    }
                    held.push(stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // A broken listener ends accepting; the timing bounds of the
                // blackhole tests would catch the degraded fixture.
                Err(_) => break,
            }
        }
        // Hand the resources back so `Blackhole::drop` can dispose of them.
        (listener, held)
    });
    Blackhole {
        addr,
        shutdown,
        accept_thread: Some(accept_thread),
    }
}

/// What the blackhole accept thread returns when it stops: the listener and
/// every socket it held, so [`Blackhole`] can close/drop them in order.
type AcceptThread = JoinHandle<(TcpListener, Vec<TcpStream>)>;

/// The hung-database fixture as an RAII guard (returned by
/// [`spawn_blackhole`]): it OWNS the fixture's resources across the accept
/// thread's lifetime, and `Drop` tears the whole fixture down — stop the
/// accept loop, join the thread, drop the held sockets, close the listener —
/// so nothing outlives the test.
struct Blackhole {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<AcceptThread>,
}

impl Blackhole {
    /// The loopback address the fixture's pool must target.
    fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Blackhole {
    fn drop(&mut self) {
        // 1. Tell the accept loop to stop.
        self.shutdown.store(true, Ordering::Release);
        // 2. End the blocking `accept()`: closing the listener from ANOTHER
        //    thread does not reliably unblock a pending `accept()` (POSIX
        //    leaves it blocked; Winsock is racy), so the loop is woken with
        //    one loopback connection instead — the flag above makes it exit
        //    without holding that wake socket.
        let _ = TcpStream::connect(self.addr);
        // 3. Join the thread (it is GONE before the test ends), then drop
        //    the fixture's resources explicitly and in order: every held
        //    socket first, the listener last (it closes here).
        let (listener, held) = self
            .accept_thread
            .take()
            .expect("blackhole accept thread is joined exactly once")
            .join()
            .expect("blackhole accept thread must not panic");
        drop(held);
        drop(listener);
    }
}

/// A hand-built `PgPool` pointed at a [`spawn_blackhole`] address.
///
/// WHY THE `ScratchDb` ALLOWLIST IS NOT INVOLVED: the harness allowlist
/// guards the identity of DATABASE SERVER targets (it exists so D2c tests
/// can never race the shared verify DB). This pool targets a
/// TEST-SPAWNED loopback socket that speaks no database protocol at all —
/// there is no database server behind it to protect or race, so the
/// allowlist has nothing to say here; the harness gates (and its PGOPTIONS
/// guard) apply to `ScratchDb`, which this fixture never constructs.
///
/// The connection fields that decide WHERE and HOW the handshake goes are
/// PINNED (built from `new_without_pgpass`, so even a passfile cannot reach
/// in): host, port, username, password, database and `ssl_mode(Disable)` —
/// ambient `PGHOST`/`PGPORT`/`PGUSER`/`PGDATABASE`/`PGSSLMODE` can neither
/// redirect nor TLS-negotiate this pool. What REMAINS inherited is only
/// `PGOPTIONS`/`PGAPPNAME` (they arrive as startup-packet
/// `options`/`application_name`); both are harmless against a silent
/// listener that never parses the startup packet. `acquire_timeout` is
/// deliberately huge so that
/// ONLY the code's own caps (`LOGIN_LOOKUP_TIMEOUT` / `BOOTSTRAP_UPSERT_TIMEOUT`)
/// can end the wait — a default 30s pool acquire would race the 30s
/// bootstrap cap and blur which bound fired.
fn blackhole_pool(addr: SocketAddr) -> PgPool {
    let opts = PgConnectOptions::new_without_pgpass()
        .host(&addr.ip().to_string())
        .port(addr.port())
        .username("blackhole")
        .password("blackhole")
        .database("blackhole")
        .ssl_mode(PgSslMode::Disable);
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(300))
        .connect_lazy_with(opts)
}

/// The hung-database `AppState`: the blackhole pool, a real session key,
/// and the GIVEN ready flag — the caller owns the flag so the boot tests can
/// share it with `bootstrap_with` (the same wiring `main` does: the flag the
/// bootstrap fills is the flag login reads).
fn blackhole_state_with_ready(addr: SocketAddr, admin_ready: Arc<AtomicBool>) -> AppState {
    AppState {
        pool: Some(blackhole_pool(addr)),
        session_key: Some(SessionKey::new(SESSION_SECRET.to_vec())),
        login_gate: Arc::new(Semaphore::new(admin::LOGIN_MAX_CONCURRENT)),
        admin_ready,
    }
}

/// [`blackhole_state_with_ready`] with `admin_ready` FORCED true.
///
/// The forced flag is the fail-closed check's complement on purpose: login
/// answers 503 "not ready" BEFORE touching the pool when the flag is false,
/// so leaving it false would test the ready-flag path, not the lookup
/// timeout. Forcing it true isolates `LOGIN_LOOKUP_TIMEOUT` as the only
/// thing that can end the request. (`AtomicBool::new(true)` is the same
/// shape `main.rs`'s test states use.)
fn blackhole_state(addr: SocketAddr) -> AppState {
    blackhole_state_with_ready(addr, Arc::new(AtomicBool::new(true)))
}

// ---------------------------------------------------------------------------
// Request / assertion helpers
// ---------------------------------------------------------------------------

/// The right-credential login request (CSRF header included by
/// `common::admin_request` — the login handler enforces the same
/// custom-header rule as the rest of `/api/admin`).
fn login_request() -> axum::http::Request<axum::body::Body> {
    admin_request(
        Method::POST,
        "/api/admin/login",
        None,
        &json!({ "username": ADMIN_USERNAME, "password": ADMIN_PASSWORD }).to_string(),
    )
}

/// Asserts the login success contract: 200, the exact `{"ok":true}` body and
/// a `Set-Cookie` for [`admin::SESSION_COOKIE_NAME`].
fn assert_login_ok(status: StatusCode, headers: &axum::http::HeaderMap, body: &str, context: &str) {
    assert_eq!(status, StatusCode::OK, "{context}: login must answer 200");
    assert_eq!(body, LOGIN_OK, "{context}: exact login success body");
    let cookie = headers
        .get(header::SET_COOKIE)
        .unwrap_or_else(|| panic!("{context}: login must set the session cookie"))
        .to_str()
        .expect("Set-Cookie is ASCII");
    assert!(
        cookie.starts_with(&format!("{}=", admin::SESSION_COOKIE_NAME)),
        "{context}: Set-Cookie must carry the session cookie, got {cookie:?}"
    );
}

/// Asserts the generic 503 contract: 503 with EXACTLY the state layer's
/// generic body — byte-for-byte, so no driver detail can leak past it — and
/// no cookie.
fn assert_generic_unavailable_503(
    status: StatusCode,
    headers: &axum::http::HeaderMap,
    body: &str,
    context: &str,
) {
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{context}: must answer 503, got {status} with {body}"
    );
    assert_eq!(
        body, DATABASE_UNAVAILABLE_503,
        "{context}: exact generic unavailable body (no driver detail)"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "{context}: a refused login must never set a cookie"
    );
}

/// `admin_users` rows for one username, read with direct SQL (the DB is the
/// witness for what the bootstrap upsert wrote, never the handler's word).
fn admin_user_count(db: &ScratchDb, username: &str) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_users WHERE username = $1")
            .bind(username)
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("admin_users lookup failed for {username}: {e}"))
    })
}

/// The stored `admin_users.password_hash` for one username (direct SQL).
fn admin_user_hash(db: &ScratchDb, username: &str) -> Option<String> {
    db.block_on(async {
        sqlx::query_scalar::<_, String>("SELECT password_hash FROM admin_users WHERE username = $1")
            .bind(username)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("admin_users hash lookup failed for {username}: {e}"))
    })
}

/// Parse-only witness that a stored `admin_users.password_hash` is a USABLE,
/// VERIFIABLE argon2id PHC string — the strictest validation the pinned
/// argon2 0.5.3 / password-hash 0.5.0 API supports WITHOUT computing anything
/// (the argon2 budget). Every gate mirrors a failure `Argon2::verify_password`
/// would otherwise hit at verification time (its blanket `PasswordVerifier`
/// re-hashes with the PARSED version/params and `Salt::decode_b64`'d bytes),
/// so a record that passes here can actually be verified later:
///
/// * PHC parse (`PasswordHash::new`) + the `<id>` naming `Argon2id`
///   specifically (`Algorithm::try_from` re-validates the identifier).
/// * VERSION — present AND accepted by `argon2::Version::try_from` (16/V0x10
///   or 19/V0x13). POLICY: accept any argon2-SUPPORTED version, not only the
///   19/V0x13 the bootstrap writes (the default `PasswordHasher` stamps
///   `Some(self.version.into())` from `Version::default()` = V0x13). Why not
///   pin to 19: the witness's claim is "verifiable by the pinned argon2",
///   and the verifier HONORS the parsed version (`hash_password_customized`
///   runs `Version::try_from`) — a v=999 record can NEVER verify and must be
///   rejected, while a v=16 record verifies fine; pinning to 19 would assert
///   an admin policy stricter than the code contract. An ABSENT version is
///   rejected too: the verifier would silently fall back to its default,
///   pinning semantics the string never stated.
/// * m/t/p params PRESENT (the typed `Params::try_from` would silently
///   DEFAULT a missing one) and coherent (its range checks, e.g. m >= 8*p).
/// * SALT — present, genuinely Base64-DECODES, and decodes to at least
///   `argon2::MIN_SALT_LEN` (8) bytes. THE gap this closes:
///   `password_hash::Salt::from_b64` (what `PasswordHash::new` runs) checks
///   only LENGTH (4..=64 CHARS) and the PHC charset `[a-zA-Z0-9/+.-]` — its
///   source carries `TODO(tarcieri): full B64 decoding check?` — so
///   charset-legal-but-not-base64ct `....` "parses". The decode surface used
///   here is `Salt::decode_b64(&mut [u8]) -> Result<&[u8]>` (password-hash
///   0.5.0's public B64 decoder, base64ct `Base64Unpadded` — the same call
///   argon2 makes on the salt at hash/verify time). The bound is argon2's
///   `MIN_SALT_LEN` = 8 DECODED bytes (NOT `Params::MIN_SALT_LEN` — no such
///   constant exists in argon2 0.5.3; `Salt::MIN_LENGTH` = 4 is a CHARS
///   bound): `hash_password_into` rejects shorter salts (argon2's own
///   `decoded_salt_too_short` test pins this seam), so a shorter record
///   could never be re-verified.
/// * output (`hash`) present — without the digest there is nothing to
///   verify against.
///
/// Pure and total: no DB, no hashing. Every error names its gate
/// (`version: ...` / `salt: ...`), so the pure fixtures (Test 5) can assert
/// WHICH gate rejected.
fn validate_stored_phc(hash: &str) -> Result<(), String> {
    let parsed = PasswordHash::new(hash).map_err(|e| {
        format!("parse: the stored hash must parse as a PHC string: {e}, got {hash:?}")
    })?;
    // The PHC `<id>` must name argon2id specifically: `argon2::Algorithm`
    // re-validates the identifier, so argon2d/argon2i/anything else fails
    // here instead of passing as "some argon2 PHC".
    let algorithm = Algorithm::try_from(parsed.algorithm).map_err(|e| {
        format!("algorithm: the PHC id must name a known argon2 algorithm: {e}, got {hash:?}")
    })?;
    if algorithm != Algorithm::Argon2id {
        return Err(format!(
            "algorithm: the stored hash must be argon2id, got {hash:?}"
        ));
    }
    // Version gate — see the fn docs for the accept-any-supported policy.
    let version = parsed.version.ok_or_else(|| {
        format!("version: the PHC must carry an explicit version (v=...), got {hash:?}")
    })?;
    Version::try_from(version).map_err(|_| {
        format!(
            "version: v={version} is not an argon2-supported version \
             (only 16/V0x10 and 19/V0x13), got {hash:?}"
        )
    })?;
    // The argon2id cost params m/t/p must be PRESENT in the string (the
    // typed conversion below would silently DEFAULT a missing one), so each
    // is read out of the parsed param list explicitly.
    for param in ["m", "t", "p"] {
        if parsed.params.get_decimal(param).is_none() {
            return Err(format!(
                "params: the PHC must carry the argon2id {param} param, got {hash:?}"
            ));
        }
    }
    // ...and the set must form coherent argon2 costs (the typed parse
    // validates the ranges, e.g. m >= 8*p) — what `argon2::Params` accepts.
    Params::try_from(&parsed).map_err(|e| {
        format!("params: the PHC m/t/p params must form valid argon2 params: {e}, got {hash:?}")
    })?;
    // A verification record without a salt, or without the digest output,
    // is not a usable argon2id hash at all (PasswordHash exposes both as
    // parsed fields).
    let salt = parsed
        .salt
        .ok_or_else(|| format!("salt: the PHC must carry a salt, got {hash:?}"))?;
    // `Salt::from_b64` (already run by `PasswordHash::new`) is charset/
    // length ONLY — the real Base64 decode is `Salt::decode_b64`, exactly
    // what argon2 runs on the salt at hash/verify time.
    let mut salt_buf = [0u8; Salt::MAX_LENGTH];
    let decoded = salt.decode_b64(&mut salt_buf).map_err(|e| {
        format!("salt: the PHC salt must Base64-decode through Salt::decode_b64: {e}, got {hash:?}")
    })?;
    if decoded.len() < MIN_SALT_LEN {
        return Err(format!(
            "salt: the PHC salt decodes to {} bytes, below argon2::MIN_SALT_LEN \
             ({MIN_SALT_LEN}), got {hash:?}",
            decoded.len()
        ));
    }
    if parsed.hash.is_none() {
        return Err(format!(
            "output: the PHC must carry the hash output, got {hash:?}"
        ));
    }
    Ok(())
}

/// `GET /api/catalogue` -> parsed products array.
fn catalogue_products(body: &str) -> Vec<Value> {
    let parsed: Value = serde_json::from_str(body).expect("catalogue response must be JSON");
    parsed["products"]
        .as_array()
        .expect("catalogue response must carry a products array")
        .clone()
}

/// Whether the catalogue feed serves one slug (asserted BY SLUG, per the
/// inactive-SKU contract).
fn catalogue_serves(body: &str, slug: &str) -> bool {
    catalogue_products(body)
        .iter()
        .any(|product| product["slug"] == slug)
}

/// `GET /api/admin/products` -> the one row for a slug, or None (the admin
/// reader lists ALL rows, inactive included).
fn admin_product_row(body: &str, slug: &str) -> Option<Value> {
    let rows: Vec<Value> = serde_json::from_str(body).expect("admin products must be JSON");
    rows.into_iter().find(|row| row["slug"] == slug)
}

// ---------------------------------------------------------------------------
// Test 1: outage -> 503 -> pool recovery -> 200 (ONE flow, one pool object)
// ---------------------------------------------------------------------------

/// The pool-recovery proof. The `ScratchDb` pool IS the state pool
/// (`AppState.pool` is a handle clone of it — one shared connection pool),
/// and the router is built ONCE and driven through all three phases.
#[test]
fn login_outage_503_then_pool_recovery_restores_login() {
    let Some(url) = common::scratch_db_url() else {
        // Skip: the helper has already printed the loud banner.
        return;
    };
    let db = ScratchDb::new(&url, "resilience_outage_recovery");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());

    // Baseline: right credentials against a healthy database.
    let (status, headers, body) = db.block_on(send(&router, login_request()));
    assert_login_ok(status, &headers, &body, "baseline login");
    // The login path never audits — witnessed BEFORE the outage so it is not
    // an artifact of the recreated (empty) table below.
    assert!(
        audit_rows(&db).is_empty(),
        "a successful login must write no audit rows"
    );

    // Outage: the database is dropped WITH (FORCE), severing every connection
    // the state pool holds. Nothing restarts — the same router and pool
    // handle now face a dead database.
    db.drop_database_with_force();

    for attempt in 1..=2 {
        let (status, headers, body) = db.block_on(send(&router, login_request()));
        assert_generic_unavailable_503(status, &headers, &body, &format!("outage login {attempt}"));
    }

    // Recovery: the database comes back (CREATE + migrations 0001..0004 on
    // the SAME test pool — sqlx reconnects lazily, see
    // `ScratchDb::drop_database_with_force`), the boot upsert runs again and
    // login works on the SAME router.
    db.recreate_database();
    db.apply_migrations();
    assert_eq!(
        admin_user_count(&db, ADMIN_USERNAME),
        0,
        "the recreated database must start with no admin row"
    );

    // `bootstrap_with` is one-shot per ready flag (it only ever stores true),
    // so "ready again" is proven with a FRESH false flag; the original boot
    // flag stays true through the outage (it is boot state, not a DB probe).
    let recovery_ready = Arc::new(AtomicBool::new(false));
    db.block_on(admin::bootstrap_with(
        state.pool.as_ref(),
        state.session_key.as_ref(),
        Rotation::Requested {
            username: ADMIN_USERNAME.to_string(),
            password: ADMIN_PASSWORD.to_string(),
        },
        recovery_ready.clone(),
    ));
    assert!(
        recovery_ready.load(Ordering::Acquire),
        "the re-run bootstrap must mark admin auth ready again"
    );
    assert!(
        state.admin_ready.load(Ordering::Acquire),
        "the original boot flag must remain true across the outage"
    );
    assert_eq!(
        admin_user_count(&db, ADMIN_USERNAME),
        1,
        "the re-run bootstrap must upsert the admin row into the recreated database"
    );

    let (status, headers, body) = db.block_on(send(&router, login_request()));
    assert_login_ok(status, &headers, &body, "recovery login");

    // Final outage-witness (see the module docs): the failed logins during
    // the outage wrote no audit rows — the login path never audits, and the
    // recreated audit_log is still empty after four logins (one success + two
    // failures + one success) and one bootstrap.
    assert!(
        audit_rows(&db).is_empty(),
        "no login or bootstrap may ever write an audit row"
    );
}

// ---------------------------------------------------------------------------
// Test 2: LOGIN_LOOKUP_TIMEOUT caps a hung database
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_lookup_timeout_caps_a_hung_database() {
    // Suite gate (see the module docs): the blackhole needs no database, but
    // the binary skips as a whole without the scratch server armed.
    if common::scratch_db_url().is_none() {
        // Skip: the helper has already printed the loud banner.
        return;
    }
    // The target is a test-spawned blackhole socket, not a database server
    // (see `blackhole_pool`). The revalidation guard still runs because the
    // mounted router carries the mutation handlers.
    common::assert_no_revalidate_webhook_env(|name| {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    });
    // The guard is bound FIRST so it drops LAST: it must outlive the pool
    // and state built on its address.
    let blackhole = spawn_blackhole();
    let addr = blackhole.addr();
    let state = blackhole_state(addr);
    let router = admin_router(state);

    let start = Instant::now();
    let (status, headers, body) = send(&router, login_request()).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed >= LOOKUP_LOWER && elapsed < LOOKUP_UPPER,
        "the lookup must be capped by LOGIN_LOOKUP_TIMEOUT ({LOOKUP_CAP:?}), \
         waited {elapsed:?}"
    );
    // Same generic body as an errored lookup: a timed-out lookup is
    // deliberately indistinguishable from a dead database (pinned here as the
    // exact 503 wording the code guarantees on the timeout branch).
    assert_generic_unavailable_503(status, &headers, &body, "hung-database login");
}

// ---------------------------------------------------------------------------
// Test 3: bootstrap upsert outcomes (success / error / timeout)
// ---------------------------------------------------------------------------

/// Success leg, asserted explicitly even though every other boot covers it.
#[test]
fn bootstrap_upsert_success_marks_admin_auth_ready() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "resilience_bootstrap_ok");
    db.apply_migrations();
    // `boot_admin` = `bootstrap_with` with an injected `Rotation::Requested`
    // (the boot path minus the env read) and asserts the ready flag; the
    // DB-side outcomes are then witnessed with direct SQL.
    let state = boot_admin(&db);
    assert!(state.admin_ready.load(Ordering::Acquire));
    assert_eq!(
        admin_user_count(&db, ADMIN_USERNAME),
        1,
        "the upsert must land exactly one admin_users row"
    );
    let hash = admin_user_hash(&db, ADMIN_USERNAME).expect("the admin row must carry a hash");
    // Real PHC parsing, not a prefix sniff (the module's claim is a "valid,
    // verifiable argon2id PHC"): `validate_stored_phc` is the pure witness —
    // parse + argon2id id + an explicit argon2-SUPPORTED version + coherent
    // m/t/p + a salt that genuinely Base64-decodes to argon2's minimum
    // decoded length + the output (every check argon2's verifier would
    // otherwise fail on; nothing hashes or verifies — the argon2 budget is
    // unchanged). The pure fixtures in Test 5 drive the SAME witness and
    // pin why each gate exists.
    validate_stored_phc(&hash)
        .unwrap_or_else(|e| panic!("the stored hash must be a usable argon2id PHC: {e}"));
}

/// Error leg: a LOST database fails the upsert FAST (a direct query first
/// witnesses the connection-level error CLASS — SQLSTATE 3D000 — so this is
/// the error path, not a 30s-cap timeout), `admin_ready` stays false (fail
/// closed) and login answers
/// the fail-closed 503 — BEFORE any pool use, so the body is the not-ready
/// one, not a database error.
#[test]
fn bootstrap_upsert_error_fails_fast_and_login_stays_disabled() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "resilience_bootstrap_err");
    db.apply_migrations();
    db.drop_database_with_force();

    // DIRECT witness of the failure class, BEFORE the timed boot below: a
    // query through the very pool the upsert will use must fail with the
    // immediate missing-database error — a sqlx DATABASE error whose SQLSTATE
    // is exactly 3D000. Why this beats the wall-clock bound: the <10s bound
    // below ALONE cannot tell an instant 3D000 error apart from the scratch
    // pool's own 5s acquire timeout (both "fail fast"); the error
    // CLASSIFICATION is what proves the driver answered `database ... does
    // not exist` right away (sqlx's connect-phase retry covers only
    // ConnectionRefused and transient 53300/57P03 — not 3D000).
    let witness = db.block_on(async {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(db.pool())
            .await
            .expect_err("SELECT 1 must fail: the database has been dropped")
    });
    match &witness {
        sqlx::Error::Database(db_err) => assert_eq!(
            db_err.code().as_deref(),
            Some("3D000"),
            "the lost database must surface as SQLSTATE 3D000 (invalid catalog \
             name: database does not exist), got: {witness}"
        ),
        other => panic!(
            "the lost database must surface as a sqlx DATABASE error with SQLSTATE \
             3D000 (the immediate missing-database answer), got: {other:?}"
        ),
    }

    // Fresh boot-shaped state with the fail-closed flag still false (the
    // shape `main` starts with).
    let state = AppState {
        pool: Some(db.pool().clone()),
        session_key: Some(SessionKey::new(SESSION_SECRET.to_vec())),
        login_gate: Arc::new(Semaphore::new(admin::LOGIN_MAX_CONCURRENT)),
        admin_ready: Arc::new(AtomicBool::new(false)),
    };
    let ready = state.admin_ready.clone();
    let start = Instant::now();
    db.block_on(admin::bootstrap_with(
        state.pool.as_ref(),
        state.session_key.as_ref(),
        Rotation::Requested {
            username: ADMIN_USERNAME.to_string(),
            password: ADMIN_PASSWORD.to_string(),
        },
        ready.clone(),
    ));
    let elapsed = start.elapsed();

    // SECONDARY sanity bound (the primary witness is the 3D000
    // classification above): the failure must stay on the error path, well
    // under `BOOTSTRAP_UPSERT_TIMEOUT` — never reaching the 30s cap.
    assert!(
        elapsed < BOOTSTRAP_ERROR_UPPER,
        "the failed upsert must fail fast (error path), waited {elapsed:?}"
    );
    assert!(
        !ready.load(Ordering::Acquire),
        "a failed boot upsert must leave admin_ready false (fail closed)"
    );

    let router = admin_router(state.clone());
    let (status, headers, body) = db.block_on(send(&router, login_request()));
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "login while boot state is unresolved must answer 503, got {status} with {body}"
    );
    assert_eq!(
        body, ADMIN_AUTH_NOT_READY_503,
        "the fail-closed login body is the not-ready 503, never a 401 verdict"
    );
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "fail-closed login must never set a cookie"
    );
}

/// Timeout leg: the same hung-database shape as test 2, at boot: the upsert
/// is capped by `BOOTSTRAP_UPSERT_TIMEOUT` (the code DOES wrap it — pinned
/// here), `admin_ready` stays false and login fails closed. ~30s by design.
#[tokio::test]
async fn bootstrap_upsert_timeout_is_capped_and_login_stays_disabled() {
    // Suite gate (see the module docs): the blackhole needs no database, but
    // the binary skips as a whole without the scratch server armed.
    if common::scratch_db_url().is_none() {
        // Skip: the helper has already printed the loud banner.
        return;
    }
    common::assert_no_revalidate_webhook_env(|name| {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    });
    // The guard is bound FIRST so it drops LAST: it must outlive the state
    // built on its address.
    let blackhole = spawn_blackhole();
    let addr = blackhole.addr();
    // ONE fresh fail-closed flag, shared by the bootstrap and the state (the
    // exact `main` wiring): the bootstrap fills it only on success, and the
    // login below must see it FALSE — that is the fail-closed path under
    // test. (Test 2 forces the flag true instead, to isolate the lookup
    // timeout from this path.)
    let ready = Arc::new(AtomicBool::new(false));
    let state = blackhole_state_with_ready(addr, ready.clone());

    let start = Instant::now();
    admin::bootstrap_with(
        state.pool.as_ref(),
        state.session_key.as_ref(),
        Rotation::Requested {
            username: ADMIN_USERNAME.to_string(),
            password: ADMIN_PASSWORD.to_string(),
        },
        ready.clone(),
    )
    .await;
    let elapsed = start.elapsed();

    assert!(
        elapsed >= BOOTSTRAP_LOWER && elapsed < BOOTSTRAP_UPPER,
        "the upsert must be capped by BOOTSTRAP_UPSERT_TIMEOUT ({BOOTSTRAP_CAP:?}), \
         waited {elapsed:?}"
    );
    assert!(
        !ready.load(Ordering::Acquire),
        "a timed-out boot upsert must leave admin_ready false (fail closed)"
    );

    let router = admin_router(state);
    let (status, headers, body) = send(&router, login_request()).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "login while boot state is unresolved must answer 503, got {status} with {body}"
    );
    assert_eq!(body, ADMIN_AUTH_NOT_READY_503);
    assert!(headers.get(header::SET_COOKIE).is_none());
}

// ---------------------------------------------------------------------------
// Test 4: inactive-SKU catalogue proof (API leg)
// ---------------------------------------------------------------------------

/// Deactivate a seeded SKU via the admin update route: absent from the
/// PUBLIC catalogue feed (by slug, with a non-vacuous control), still listed
/// for the admin reader with `active=false`, and back in the feed after
/// `active=true`. The DB row is witnessed with direct SQL throughout.
#[test]
fn inactive_sku_absent_from_public_catalogue_but_visible_to_admin() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "resilience_inactive_sku");
    db.apply_migrations();
    let state = boot_admin(&db);
    // The catalogue leg mounts `GET /api/catalogue` (the public feed) beside
    // the shared admin router — the shared helper mounts only the admin
    // surface (see `common::admin_router`).
    let router: Router = Router::new()
        .route("/api/catalogue", get(catalogue::get_catalogue))
        .with_state(state.clone())
        .merge(admin_router(state.clone()));
    let cookie = minted_session_cookie(&state);

    let row = product_row(&db, SEEDED_SLUG).expect("the seeded product must exist");
    let (id, _, _, _, _, _, _, _, seeded_active, _, _) = row;
    assert!(seeded_active, "the seeded SKU starts active");

    // Baseline: the public feed serves both slugs.
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/catalogue", None, ""),
    ));
    assert_eq!(status, StatusCode::OK, "GET /api/catalogue must answer 200");
    assert!(
        catalogue_serves(&body, SEEDED_SLUG),
        "baseline feed serves the SKU"
    );
    assert!(
        catalogue_serves(&body, OTHER_SLUG),
        "baseline feed serves the control"
    );

    // Deactivate via the admin update route (PUT with a PARTIAL body).
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::PUT,
            &format!("/api/admin/products/{id}"),
            Some(&cookie),
            &json!({ "active": false }).to_string(),
        ),
    ));
    assert_eq!(
        status,
        StatusCode::OK,
        "deactivate must be accepted: {body}"
    );
    let updated: Value = serde_json::from_str(&body).expect("update response must be JSON");
    assert_eq!(
        updated["active"],
        Value::Bool(false),
        "response shows active=false"
    );
    assert_eq!(
        updated["slug"], SEEDED_SLUG,
        "the update targets the seeded SKU"
    );
    let (_, _, _, _, _, _, _, _, db_active, _, _) =
        product_row(&db, SEEDED_SLUG).expect("the row must still exist");
    assert!(!db_active, "the DB row is inactive after the patch");

    // The PUBLIC feed: the SKU is ABSENT (by slug), the drop itself is
    // unaffected (non-vacuous control), and the slug leaks NOWHERE in the
    // payload (content/reference-price/piece data is dropped with the
    // product).
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/catalogue", None, ""),
    ));
    assert_eq!(status, StatusCode::OK, "GET /api/catalogue must answer 200");
    assert!(
        !catalogue_serves(&body, SEEDED_SLUG),
        "an inactive SKU must be absent from the public feed"
    );
    assert!(
        !body.contains(SEEDED_SLUG),
        "an inactive SKU must not leak anywhere in the catalogue payload"
    );
    assert!(
        catalogue_serves(&body, OTHER_SLUG),
        "the rest of the drop must still be served (non-vacuous absence)"
    );

    // The ADMIN reader: ALL rows, inactive included.
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/products", Some(&cookie), ""),
    ));
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/admin/products must answer 200"
    );
    let admin_row = admin_product_row(&body, SEEDED_SLUG)
        .expect("the admin list must still carry the inactive SKU");
    assert_eq!(
        admin_row["active"],
        Value::Bool(false),
        "the admin list shows the row with active=false"
    );

    // Reactivate: the catalogue serves it again.
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::PUT,
            &format!("/api/admin/products/{id}"),
            Some(&cookie),
            &json!({ "active": true }).to_string(),
        ),
    ));
    assert_eq!(
        status,
        StatusCode::OK,
        "reactivate must be accepted: {body}"
    );
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/catalogue", None, ""),
    ));
    assert_eq!(status, StatusCode::OK, "GET /api/catalogue must answer 200");
    assert!(
        catalogue_serves(&body, SEEDED_SLUG),
        "the reactivated SKU must be back in the public feed"
    );
    let (_, _, _, _, _, _, _, _, db_active, _, _) =
        product_row(&db, SEEDED_SLUG).expect("the row must still exist");
    assert!(db_active, "the DB row is active again after the patch");
}

// ---------------------------------------------------------------------------
// Test 5: the stored-PHC witness — pure fixtures (no DB, no network)
// ---------------------------------------------------------------------------

/// (d) Positive control: a HAND-WRITTEN structurally valid argon2id PHC —
/// v=19 (the bootstrap's version), the argon2 default costs
/// (m=19456, t=2, p=1), a real 16-byte salt (ASCII `0123456789abcdef`,
/// correctly B64-encoded) and a parse-valid 32-byte placeholder digest
/// (43 `A` chars decode to the argon2 default output length — placeholder
/// BYTES, nothing computed: the witness only parses, the string does not
/// need to verify).
const VALID_PHC: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// (a) Same shape as [`VALID_PHC`] but `v=999`: a syntactically decimal PHC
/// version argon2 does NOT support (only 16/V0x10 and 19/V0x13).
const UNSUPPORTED_VERSION_PHC: &str =
    "$argon2id$v=999$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// (b) Same shape as [`VALID_PHC`] but the salt is `....`: charset-legal for
/// `Salt::from_b64` (`[a-zA-Z0-9/+.-]`, length 4) yet NOT base64ct-decodable
/// — the round-2 false positive this witness closes.
const UNDECODABLE_SALT_PHC: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$....$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// (c) Same shape as [`VALID_PHC`] but the salt is `AAAAAAAA`: genuinely
/// decodes (8 B64 chars -> 6 bytes) yet falls below argon2's `MIN_SALT_LEN`
/// (8 DECODED bytes) — the decoded-length false positive.
const SHORT_SALT_PHC: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// (d) Positive control: a structurally valid, argon2-supported PHC passes
/// the witness.
#[test]
fn phc_witness_accepts_structurally_valid_supported_phc() {
    // Suite gate (see the module docs): pure string checks, but the binary
    // skips as a whole without the scratch server armed.
    if common::scratch_db_url().is_none() {
        return;
    }
    validate_stored_phc(VALID_PHC)
        .expect("a structurally valid v=19 argon2id PHC with a real 16-byte salt must pass");
}

/// (a) `v=999` parses as a decimal PHC version but is unsupported by argon2:
/// the VERSION gate must reject it, and the message must name the version —
/// never blame the salt.
#[test]
fn phc_witness_rejects_unsupported_version() {
    if common::scratch_db_url().is_none() {
        return;
    }
    let err = validate_stored_phc(UNSUPPORTED_VERSION_PHC)
        .expect_err("v=999 is decimal-shaped but argon2 only supports 16/19");
    assert!(
        err.contains("version"),
        "the version gate must name itself, got: {err}"
    );
    assert!(
        !err.contains("salt"),
        "a version failure must not read as a salt failure, got: {err}"
    );
}

/// (b) Salt `....` is charset-legal but does not Base64-decode: the DECODE
/// gate must reject it, naming the decode step and not the version or the
/// length gate.
#[test]
fn phc_witness_rejects_salt_that_does_not_base64_decode() {
    if common::scratch_db_url().is_none() {
        return;
    }
    let err = validate_stored_phc(UNDECODABLE_SALT_PHC)
        .expect_err("`....` is charset-legal for Salt::from_b64 but not Base64-decodable");
    assert!(
        err.contains("salt"),
        "the salt gate must name itself, got: {err}"
    );
    assert!(
        err.contains("decode_b64"),
        "the decode gate must name the decode step, got: {err}"
    );
    assert!(
        !err.contains("version"),
        "a salt failure must not read as a version failure, got: {err}"
    );
    assert!(
        !err.contains("MIN_SALT_LEN"),
        "the decode gate must not blame the length gate, got: {err}"
    );
}

/// (c) Salt `AAAAAAAA` decodes (to 6 bytes) but stays under argon2's
/// `MIN_SALT_LEN` (8): the decoded-LENGTH gate must reject it — a different
/// message from the decode gate (b).
#[test]
fn phc_witness_rejects_salt_below_argon2_minimum_decoded_length() {
    if common::scratch_db_url().is_none() {
        return;
    }
    let err = validate_stored_phc(SHORT_SALT_PHC)
        .expect_err("8 B64 chars decode to 6 bytes, below argon2::MIN_SALT_LEN (8)");
    assert!(
        err.contains("salt"),
        "the salt gate must name itself, got: {err}"
    );
    assert!(
        err.contains("MIN_SALT_LEN"),
        "the length gate must name the bound, got: {err}"
    );
    assert!(
        !err.contains("version"),
        "a salt failure must not read as a version failure, got: {err}"
    );
    assert!(
        !err.contains("decode_b64"),
        "the length gate must not blame the decode gate, got: {err}"
    );
}
