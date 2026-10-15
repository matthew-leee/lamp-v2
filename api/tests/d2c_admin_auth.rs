//! D2c step 3 — admin auth + mutation round-trip against the scratch Postgres
//! harness (`common::ScratchDb`).
//!
//! What these tests pin down (the D2b unit-contract claims deliberately
//! deferred to D2c, now proven at the DB layer against real Postgres):
//!
//! * `login_wrong_credentials_are_generic_401_without_set_cookie`: a failed
//!   login (wrong password for the bootstrapped admin, and an unknown
//!   username) answers 401 with the API's existing GENERIC body — the same
//!   byte-for-byte body for both verdicts, so the endpoint never discloses
//!   WHICH part failed (no account enumeration) — and never sets a cookie.
//! * `login_success_sets_session_cookie_that_authenticates_whoami`: the right
//!   credentials answer 200 + a `Set-Cookie` carrying the session cookie under
//!   [`admin::SESSION_COOKIE_NAME`], spelled with the exact attribute contract
//!   (`HttpOnly; SameSite=Lax; Path=/; Max-Age` = the TTL, `Secure` per the
//!   config knob), and that exact cookie authenticates the protected whoami
//!   (200 + the username). Session-token semantics (sub, TTL, HMAC) stay
//!   D2b-unit-pinned; here the cookie's presence and its live effect are
//!   asserted.
//! * `whoami_without_cookie_is_401_not_authenticated`: no cookie, no session.
//! * `mutation_round_trip_writes_product_row_and_audit_row`: with a session
//!   cookie, `POST /api/admin/products` lands the EXACT posted values in
//!   Postgres (verified by direct SQL, not by echoing the response; the
//!   response's `created_at` is also cross-checked against the row's Postgres
//!   timestamp within 1s) and writes
//!   the commitment-2 `audit_log` row for the acting admin (`create` /
//!   `products` / the new id, with the exact change detail) — also readable
//!   through `GET /api/admin/audit-log`.
//! * `duplicate_product_slug_is_409_and_writes_no_second_row` and
//!   `duplicate_piece_no_is_409_and_leaves_seeded_pieces_untouched`: the
//!   uniqueness-conflict contract (409 + the handler's exact body), with the
//!   rolled-back attempt leaving no product/piece/audit debris.
//! * `update_product_on_missing_id_is_404_and_writes_no_audit_row`: the
//!   missing-target contract of the update handler.
//! * `unauthenticated_and_csrfless_mutations_are_401_and_403`: server-side
//!   authorization on every state-changing request (commitment 2 / ADMIN-01) —
//!   no cookie is a 401 before the handler runs, and the CSRF custom-header
//!   half of the same middleware refuses a cookie-authenticated mutation that
//!   lacks `X-Requested-With` (403); neither refusal writes anything, and a
//!   counting observer mounted between the auth middleware and the handlers
//!   witnesses that the handler stack is never reached (with a positive
//!   control proving the observer itself observes).
//!
//! Each DB-backed test gates on `common::scratch_db_url()` (skip on unset, loud banner),
//! owns its ephemeral `d2c_test_*` database, replays migrations 0001..0004,
//! boots the admin auth stack exactly like `main` does and drives the routes
//! with `tower` oneshot requests. The shared boot / router / cookie / request
//! machinery lives in `common` (extracted there in D2c step 4 so the admin
//! suites share ONE definition; step-4 kept these tests' semantics identical);
//! the router mounts the admin surface at the real paths and the full route
//! wiring (CORS, tracing, storefront routes) stays pinned by the bin tests in
//! `api/src/main.rs`.
//!
//! Argon2 budget: every login verify costs ~19 MiB + a heavy CPU slice, so
//! only the two login tests perform real logins (one wrong + one right verdict
//! per test, never a loop); the mutation tests mint their cookie through the
//! same signer the login handler uses (see `common::minted_session_cookie`).
//! Each
//! test's boot still hashes one argon2 password (the bootstrap upsert) — that
//! is the cost of the real boot path.
//!
//! Tests are plain `#[test]` fns driving the harness runtime through
//! `ScratchDb::block_on` — see `common` for why (`Drop` cannot block a
//! `#[tokio::test]` executor).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use chrono::{DateTime, Utc};
use lamp_api::admin;
use lamp_api::config;
use serde_json::{json, Value};

use common::{
    admin_request, admin_router, admin_router_with_observer, assert_no_revalidate_webhook_env,
    audit_rows, boot_admin, first_product_id, minted_session_cookie, pieces_snapshot,
    product_count, product_created_at, product_row, send, ScratchDb, ADMIN_PASSWORD,
    ADMIN_USERNAME, MISSING_PRODUCT_ID, PRODUCT_NOT_FOUND_404,
};

// The exact error bodies the handlers guarantee (D2b wording, asserted here
// byte-for-byte: an exact string is checkable, so a fuzzy contains would only
// hide drift).
const GENERIC_LOGIN_401: &str = r#"{"error":"invalid username or password"}"#;
const NOT_AUTHENTICATED_401: &str = r#"{"error":"not authenticated"}"#;
const MISSING_CSRF_403: &str = r#"{"error":"missing X-Requested-With header"}"#;
const SLUG_CONFLICT_409: &str = r#"{"error":"slug already in use"}"#;
const PIECE_CONFLICT_409: &str = r#"{"error":"piece_no already exists"}"#;
const LOGIN_OK: &str = r#"{"ok":true}"#;

// The product every create-path test posts: every field fixed so the DB row
// and the audit detail can be compared to EXACT expected values.
const PRODUCT_SLUG: &str = "d2c-auth-probe-pendant";
const PRODUCT_BRAND: &str = "Louis Poulsen";
const PRODUCT_MODEL: &str = "PH Artichoke";
const PRODUCT_PRICE_CENTS: i64 = 123_456;
const PRODUCT_FAMILY: &str = "ph-artichoke";
const PRODUCT_ART_VARIANT: &str = "dome";
const PRODUCT_ART_HUE: i32 = 210;
const PRODUCT_ACTIVE: bool = true;

/// The posted product body (one JSON object for the request and the expected
/// audit detail share these literals via [`expected_audit_detail`]).
fn product_json_body() -> String {
    json!({
        "slug": PRODUCT_SLUG,
        "brand": PRODUCT_BRAND,
        "model": PRODUCT_MODEL,
        "price_cents": PRODUCT_PRICE_CENTS,
        "family": PRODUCT_FAMILY,
        "art_variant": PRODUCT_ART_VARIANT,
        "art_hue": PRODUCT_ART_HUE,
        "active": PRODUCT_ACTIVE,
    })
    .to_string()
}

/// The `audit_log.detail` a product create must write: exactly the fields
/// that landed, nothing else.
fn expected_audit_detail() -> Value {
    json!({
        "slug": PRODUCT_SLUG,
        "brand": PRODUCT_BRAND,
        "model": PRODUCT_MODEL,
        "price_cents": PRODUCT_PRICE_CENTS,
        "family": PRODUCT_FAMILY,
        "art_variant": PRODUCT_ART_VARIANT,
        "art_hue": PRODUCT_ART_HUE,
        "active": PRODUCT_ACTIVE,
    })
}

// ---------------------------------------------------------------------------
// 1 + 2 + 3: the login / cookie / whoami round-trip
// ---------------------------------------------------------------------------

/// Wrong credentials (an existing admin with a wrong password, and an unknown
/// username) are 401 with ONE generic body — byte-identical across both
/// verdicts, so the endpoint cannot be used to enumerate accounts — and never
/// mint a cookie. Two verifies total (one per verdict branch), never a loop.
#[test]
fn login_wrong_credentials_are_generic_401_without_set_cookie() {
    let Some(url) = common::scratch_db_url() else {
        // Skip: the helper has already printed the loud banner.
        return;
    };
    let db = ScratchDb::new(&url, "auth_login_wrong");
    db.apply_migrations();
    let router = admin_router(boot_admin(&db));

    for (username, password) in [
        (ADMIN_USERNAME, "wrong-password-fake-9999"),
        ("ghost-admin", ADMIN_PASSWORD),
    ] {
        let body = json!({ "username": username, "password": password }).to_string();
        let (status, headers, response_body) = db.block_on(send(
            &router,
            admin_request(Method::POST, "/api/admin/login", None, &body),
        ));
        assert_eq!(status, StatusCode::UNAUTHORIZED, "login for {username}");
        assert_eq!(
            response_body, GENERIC_LOGIN_401,
            "the 401 must be the generic body for {username} — never which \
             part of the credentials failed"
        );
        assert!(
            !headers.contains_key(header::SET_COOKIE),
            "a failed login must never mint a session cookie (username {username})"
        );
    }
}

/// Right credentials: 200 `{"ok":true}` plus the `Set-Cookie` header, spelled
/// with the exact cookie-attribute contract, and that exact cookie forwards
/// through `require_admin` into whoami (200 + the username).
#[test]
fn login_success_sets_session_cookie_that_authenticates_whoami() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_login_ok");
    db.apply_migrations();
    let router = admin_router(boot_admin(&db));

    let body = json!({ "username": ADMIN_USERNAME, "password": ADMIN_PASSWORD }).to_string();
    let (status, headers, response_body) = db.block_on(send(
        &router,
        admin_request(Method::POST, "/api/admin/login", None, &body),
    ));
    assert_eq!(status, StatusCode::OK, "login with the right credentials");
    assert_eq!(response_body, LOGIN_OK);

    let set_cookie = headers
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("a successful login must carry a Set-Cookie header");
    let pair = set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie must start with a cookie pair");
    let (cookie_name, token) = pair
        .split_once('=')
        .expect("Set-Cookie must start with name=value");
    assert_eq!(cookie_name, admin::SESSION_COOKIE_NAME);
    assert!(!token.is_empty(), "the session cookie must carry a token");

    // The WHOLE header, exactly as the handler must spell it: HttpOnly (no
    // JS access), SameSite=Lax, Path=/, Max-Age = the token TTL, and Secure
    // per the same config knob the handler reads.
    let secure = if config::session_cookie_secure() {
        "; Secure"
    } else {
        ""
    };
    let expected_cookie = format!(
        "{cookie_name}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{secure}",
        admin::SESSION_TTL_SECS
    );
    assert_eq!(set_cookie, expected_cookie);

    // Cookie forward: the minted cookie authenticates the protected whoami.
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/whoami", Some(pair), ""),
    ));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, format!(r#"{{"username":"{ADMIN_USERNAME}"}}"#));
}

/// The negative half of the cookie-forward contract: whoami without a cookie
/// is the shared generic 401 — no session, no identity.
#[test]
fn whoami_without_cookie_is_401_not_authenticated() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_whoami");
    db.apply_migrations();
    let router = admin_router(boot_admin(&db));

    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/whoami", None, ""),
    ));
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, NOT_AUTHENTICATED_401);
}

// ---------------------------------------------------------------------------
// 4 + 5 + 6 + 7: the authenticated mutation path at the DB layer
// ---------------------------------------------------------------------------

/// The full mutation round-trip: create a product with a session cookie, then
/// prove with direct SQL that the EXACT posted values landed in `products`
/// and that the commitment-2 `audit_log` row names the acting admin with the
/// `create` / `products` action-entity pair and the exact change detail —
/// which `GET /api/admin/audit-log` then serves back.
#[test]
fn mutation_round_trip_writes_product_row_and_audit_row() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_mutation");
    db.apply_migrations();
    let state = boot_admin(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::POST,
            "/api/admin/products",
            Some(&cookie),
            &product_json_body(),
        ),
    ));
    assert_eq!(status, StatusCode::OK);
    let created: Value = serde_json::from_str(&body).expect("create response must be JSON");
    let created_id = created["id"]
        .as_i64()
        .expect("create response must carry the numeric id");
    // Response row: every checkable field exactly as posted, stock starting
    // at 0. `created_at` is a clock value, so it cannot be compared to a
    // literal — but it is not merely type-checked either: below it is parsed
    // and cross-checked against the row's Postgres timestamp (within 1s).
    let expected_response = json!({
        "id": created_id,
        "slug": PRODUCT_SLUG,
        "brand": PRODUCT_BRAND,
        "model": PRODUCT_MODEL,
        "price_cents": PRODUCT_PRICE_CENTS,
        "family": PRODUCT_FAMILY,
        "art_variant": PRODUCT_ART_VARIANT,
        "art_hue": PRODUCT_ART_HUE,
        "active": PRODUCT_ACTIVE,
        "stock_total": 0,
        "stock_reserved": 0,
        "created_at": created["created_at"],
    });
    assert_eq!(
        created, expected_response,
        "the create response must match the posted values exactly"
    );

    // The DB layer is the witness: the row carries exactly what was posted.
    let row = product_row(&db, PRODUCT_SLUG).expect("created product row must exist in Postgres");
    assert_eq!(
        row,
        (
            created_id,
            PRODUCT_SLUG.to_string(),
            PRODUCT_BRAND.to_string(),
            PRODUCT_MODEL.to_string(),
            PRODUCT_PRICE_CENTS,
            PRODUCT_FAMILY.to_string(),
            PRODUCT_ART_VARIANT.to_string(),
            PRODUCT_ART_HUE,
            PRODUCT_ACTIVE,
            0,
            0,
        ),
        "the products row must carry exactly the posted values (stock starts at 0)"
    );

    // `created_at` cross-check: the response string and the row's Postgres
    // timestamp must parse as the same instant (sub-second precision may
    // differ, so 1s of slack) and both must be recent — a defaulted or
    // mangled timestamp passes neither check. A response timestamp that does
    // not parse as RFC3339 fails loudly with the raw string in the message;
    // the contract is never silently relaxed.
    let response_created_at_raw = created["created_at"].as_str().unwrap_or_else(|| {
        panic!(
            "create response must carry created_at as a string, got {}",
            created["created_at"]
        )
    });
    let response_created_at = DateTime::parse_from_rfc3339(response_created_at_raw)
        .unwrap_or_else(|e| {
            panic!(
                "create response created_at must be RFC3339: {e}; raw value \
                 kept in this message on purpose: {response_created_at_raw:?}"
            )
        })
        .with_timezone(&Utc);
    let db_created_at = product_created_at(&db, PRODUCT_SLUG);
    let skew_ms = (response_created_at - db_created_at)
        .num_milliseconds()
        .abs();
    assert!(
        skew_ms <= 1_000,
        "response created_at and the Postgres row timestamp must agree \
         within 1s (sub-second precision differences allowed): response \
         {response_created_at_raw:?} ({response_created_at:?}) vs db \
         {db_created_at:?} — |delta| = {skew_ms}ms"
    );
    let now = Utc::now();
    for (label, timestamp) in [("response", response_created_at), ("db", db_created_at)] {
        let age_s = (now - timestamp).num_seconds().abs();
        assert!(
            age_s <= 300,
            "{label} created_at must be within the last few minutes of now \
             (sanity): {timestamp:?} is {age_s}s away from {now:?}"
        );
    }

    // Commitment 2: exactly one audit row, for the acting admin, in the same
    // transaction as the create.
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "create".to_string(),
            "products".to_string(),
            Some(created_id.to_string()),
            expected_audit_detail(),
        )],
        "the create must write exactly one audit_log row for the acting admin"
    );

    // And the audit endpoint serves that same row back to the admin.
    let (status, headers, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/audit-log", Some(&cookie), ""),
    ));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "audit rows are accountability data, never cacheable"
    );
    let entries: Value = serde_json::from_str(&body).expect("audit-log response must be JSON");
    let entries = entries.as_array().expect("audit-log response is an array");
    assert_eq!(entries.len(), 1, "exactly one audit row");
    let entry = &entries[0];
    let mut keys: Vec<&str> = entry
        .as_object()
        .expect("audit entry is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "action",
            "actor",
            "created_at",
            "detail",
            "entity",
            "entity_id",
            "id"
        ]
    );
    assert_eq!(entry["id"], json!(1), "first audit row of a fresh database");
    assert_eq!(entry["actor"], json!(ADMIN_USERNAME));
    assert_eq!(entry["action"], json!("create"));
    assert_eq!(entry["entity"], json!("products"));
    assert_eq!(entry["entity_id"], json!(created_id.to_string()));
    assert_eq!(entry["detail"], expected_audit_detail());
    assert!(entry["created_at"].is_string(), "created_at is a timestamp");
}

/// The same product slug twice: the second create is 409 with the handler's
/// exact conflict body (the UNIQUE slug constraint maps to a client-fixable
/// conflict), and the rolled-back attempt leaves no second row and no audit
/// debris.
#[test]
fn duplicate_product_slug_is_409_and_writes_no_second_row() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_dup_slug");
    db.apply_migrations();
    let state = boot_admin(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let (status, _, _) = db.block_on(send(
        &router,
        admin_request(
            Method::POST,
            "/api/admin/products",
            Some(&cookie),
            &product_json_body(),
        ),
    ));
    assert_eq!(status, StatusCode::OK, "the first create must succeed");

    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::POST,
            "/api/admin/products",
            Some(&cookie),
            &product_json_body(),
        ),
    ));
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body, SLUG_CONFLICT_409);
    assert_eq!(product_count(&db, Some(PRODUCT_SLUG)), 1);
    assert_eq!(
        audit_rows(&db).len(),
        1,
        "the rolled-back duplicate create must leave no audit row"
    );
}

/// An already-taken piece number (piece 1 is seeded, DROP-01): 409 with the
/// handler's exact conflict body, and neither the seeded piece rows nor the
/// audit log are touched.
#[test]
fn duplicate_piece_no_is_409_and_leaves_seeded_pieces_untouched() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_dup_piece");
    db.apply_migrations();
    let state = boot_admin(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let pieces_before = pieces_snapshot(&db);
    assert_eq!(
        pieces_before.len(),
        16,
        "the seed numbers pieces 1-16 (DROP-01)"
    );
    // A real product id: create_piece answers 404 for a missing product
    // before it reaches the uniqueness check, so the conflict path needs a
    // product that exists.
    let product_id = first_product_id(&db);
    let body = json!({ "piece_no": 1, "product_id": product_id }).to_string();
    let (status, _, response_body) = db.block_on(send(
        &router,
        admin_request(Method::POST, "/api/admin/pieces", Some(&cookie), &body),
    ));
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(response_body, PIECE_CONFLICT_409);
    assert_eq!(
        pieces_snapshot(&db),
        pieces_before,
        "the failed insert must leave every seeded piece row untouched"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rolled-back conflict must write no audit row"
    );
}

/// Updating a product id that does not exist: 404 with the handler's exact
/// body, no audit row, no product row touched.
#[test]
fn update_product_on_missing_id_is_404_and_writes_no_audit_row() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_update_404");
    db.apply_migrations();
    let state = boot_admin(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::PUT,
            &format!("/api/admin/products/{MISSING_PRODUCT_ID}"),
            Some(&cookie),
            r#"{"active":false}"#,
        ),
    ));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, PRODUCT_NOT_FOUND_404);
    assert!(
        audit_rows(&db).is_empty(),
        "the failed update must write no audit row"
    );
    assert_eq!(
        product_count(&db, None),
        8,
        "the failed update must leave the seeded products untouched"
    );
}

/// Commitment 2 / ADMIN-01 at the server side: a state-changing request
/// without a session cookie is 401 before any handler runs (even with the
/// CSRF header present), and the CSRF half of the same middleware refuses a
/// cookie-authenticated mutation without `X-Requested-With` (403). Neither
/// refusal writes a product or audit row.
///
/// The router here carries a counting observer BETWEEN `require_admin` and
/// the handlers (see [`admin_router_with_observer`]): the handlers' own
/// `actor()` check answers the identical 401, so the status codes alone
/// cannot prove the AUTH MIDDLEWARE refused before the handler stack ran.
/// Both refusals must show 0 observer hits, and the positive control at the
/// end (a fully authenticated create) must show exactly 1 — so a broken
/// observer can never make the 0-hit assertions vacuous.
#[test]
fn unauthenticated_and_csrfless_mutations_are_401_and_403() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "auth_unauth_mutation");
    db.apply_migrations();
    let state = boot_admin(&db);
    let cookie = minted_session_cookie(&state);
    let handler_hits = Arc::new(AtomicUsize::new(0));
    let router = admin_router_with_observer(state, Some(Arc::clone(&handler_hits)));

    // No cookie (CSRF header present): refused by the middleware, the
    // handler stack must not be reached.
    let (status, _, body) = db.block_on(send(
        &router,
        admin_request(
            Method::POST,
            "/api/admin/products",
            None,
            &product_json_body(),
        ),
    ));
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, NOT_AUTHENTICATED_401);
    assert_eq!(
        handler_hits.load(Ordering::SeqCst),
        0,
        "the unauthenticated mutation must be refused BEFORE the handler stack"
    );

    // Valid cookie, but no CSRF custom header: still refused by the
    // middleware, handler never reached.
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/products")
        .header("content-type", "application/json")
        .header(header::COOKIE, cookie.as_str())
        .body(Body::from(product_json_body()))
        .expect("test request must build");
    let (status, _, body) = db.block_on(send(&router, request));
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, MISSING_CSRF_403);
    assert_eq!(
        handler_hits.load(Ordering::SeqCst),
        0,
        "the CSRF-less mutation must be refused BEFORE the handler stack"
    );

    assert_eq!(
        product_count(&db, Some(PRODUCT_SLUG)),
        0,
        "neither refusal may write a product row"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "neither refusal may write an audit row"
    );

    // POSITIVE CONTROL: the fully authenticated create runs the handler
    // exactly once — so the observer really observes the handler path and the
    // 0-hit assertions above are meaningful (never vacuous).
    let (status, _, _) = db.block_on(send(
        &router,
        admin_request(
            Method::POST,
            "/api/admin/products",
            Some(&cookie),
            &product_json_body(),
        ),
    ));
    assert_eq!(
        status,
        StatusCode::OK,
        "the fully authenticated create must reach the handler and succeed"
    );
    assert_eq!(
        handler_hits.load(Ordering::SeqCst),
        1,
        "the observer must have seen exactly the one request auth let through"
    );
    assert_eq!(
        product_count(&db, Some(PRODUCT_SLUG)),
        1,
        "the positive-control create must land a product row (the handler \
         really ran)"
    );
}

// ---------------------------------------------------------------------------
// Network-free webhook-env policy regressions (mock lookup, no DB, no env)
// ---------------------------------------------------------------------------

/// NETWORK-FREE ambient-env policy regression: [`common::assert_no_revalidate_webhook_env`]
/// is pure over its lookup, so the tests drive a MOCK environment — mutating
/// the process env from a test would race the parallel test threads, and
/// libtest captures each test's panic, so `should_panic` on a mock is the
/// honest way to pin the refusal. A set `REVALIDATE_SECRET` must refuse the
/// run; the live wiring (one std::env closure in [`common::boot_admin`]) is exercised
/// by the architect's live gate, not by these.
#[test]
#[should_panic(expected = "REVALIDATE_SECRET")]
fn revalidate_webhook_env_set_refuses_the_run() {
    assert_no_revalidate_webhook_env(|name| {
        (name == "REVALIDATE_SECRET").then(|| "fake-secret-value-0001".to_string())
    });
}

/// The refusal must name the VARIABLE and never echo the VALUE: the knob can
/// be a live credential, so the panic message itself is part of the
/// secret-hygiene contract. `should_panic` only checks a substring, so this
/// drives the pure fn under `catch_unwind` and inspects the FULL panic
/// payload for both properties.
#[test]
fn revalidate_webhook_refusal_names_variable_but_never_value() {
    const FAKE_SECRET: &str = "fake-secret-value-0001";
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_no_revalidate_webhook_env(|name| {
            (name == "REVALIDATE_SECRET").then(|| FAKE_SECRET.to_string())
        });
    }));
    let payload = outcome.expect_err("a set REVALIDATE_SECRET must panic");
    let message = if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else {
        panic!("the panic payload must carry a string message");
    };
    assert!(
        message.contains("REVALIDATE_SECRET"),
        "the refusal must name the variable to unset: {message}"
    );
    assert!(
        !message.contains(FAKE_SECRET),
        "the refusal must never echo the secret value: {message}"
    );
}

/// The clean half: a mock lookup that finds no REVALIDATE_* variable passes —
/// the live wiring counts empty values as unset.
#[test]
fn revalidate_webhook_env_clean_passes() {
    assert_no_revalidate_webhook_env(|_| None);
}
