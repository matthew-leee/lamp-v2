//! R5a — the M1 money path against the scratch Postgres harness
//! (`common::ScratchDb`): atomic reservations (STOCK-01), the Stripe webhook
//! as payment authority with a replay-safe event ledger (PAY-01), the
//! late-payment rule (R2-5) and returned-unit quarantine (STOCK-01 / R2-11).
//!
//! What these tests pin down (the `orders` unit suites pin the pure rules —
//! validation, signature verification, state transitions — this suite pins
//! what lands in Postgres):
//!
//! * `reservation_reserves_stock_atomically_and_refuses_oversell`: a
//!   reservation moves units into `stock_reserved` and the availability check
//!   (`stock_total - stock_reserved - stock_quarantined`) refuses the next
//!   order at exactly 0 — no oversell of the seeded units. The response's
//!   price is the DB's (a client-sent `unit_price_cents`/`total_cents` is
//!   ignored), the public status read carries `Cache-Control: no-store` and
//!   no customer PII, an unknown/inactive SKU is 404, and the `settings`
//!   `currency` override wins over the env/code default.
//! * `stripe_webhook_settles_a_reservation_once_and_replays_are_no_ops`:
//!   a correctly signed event with matching (amount, currency) moves
//!   `reserved -> paid` and consumes `stock_total` + `stock_reserved`; the
//!   same `event_id` replayed is `idempotent: true` with NOTHING changed
//!   (PAY-01); a NEW event on the same already-paid order records the event
//!   but never deducts stock twice; a bad signature is the exact 401 and an
//!   amount mismatch the exact 400, neither writing a `payment_events` row.
//! * `expired_reservation_is_released_and_late_payment_is_flagged`: a lapsed
//!   reservation is released (units back to the saleable pool) by the NEXT
//!   order touch — a reservation (and also a public read) — and a verified
//!   payment against it transitions to `late_payment_review` WITHOUT
//!   touching stock (R2-5: reviewed/refunded, never silently fulfilled).
//! * `quarantine_return_quarantines_units_without_increasing_availability`:
//!   a returned unit lands in `stock_quarantined` together with
//!   `stock_total`, so the SALEABLE count is exactly unchanged (R2-11), the
//!   order ends `returned_quarantined`, and the operation is audited.
//! * `fps_confirmation_settles_live_orders_and_flags_late_transfers`: the
//!   admin FPS confirmation consumes units for a live reservation
//!   (`confirm_fps` audit) and flags a lapsed one (`late_fps_review` audit)
//!   with no stock change; bad input is a 400, a missing order a 404.
//! * `release_cancels_a_reservation_and_frees_its_units`: releasing an unpaid
//!   reservation cancels it and returns its units (`release` audit); a second
//!   release is the exact 409 and writes no audit.
//!
//! Determinism rules:
//! * Currency assertions are relative (`config::default_currency()`) or use
//!   an explicit `settings` override — never an ambient absolute.
//! * The Stripe signing key is armed ONCE per test binary with a fixed test
//!   value (see [`arm_webhook_secret`]); no test reads ambient configuration
//!   it does not own.
//! * Expiry is provoked by moving `orders.expires_at` into the past with
//!   direct SQL (the TTL knobs are clamped to >= 1 minute and must not be
//!   short-circuited by a machine clock).
//!
//! Argon2 budget: no live login anywhere — the admin identity is a minted
//! session cookie (`common::minted_session_cookie`); each boot still hashes
//! one argon2 password through the shared bootstrap, exactly like the other
//! DB-backed suites.
//!
//! Tests are plain `#[test]` fns driving the harness runtime through
//! `ScratchDb::block_on` (see `common` for why `Drop` cannot block a
//! `#[tokio::test]` executor).

mod common;

use std::sync::Once;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use lamp_api::config;
use lamp_api::orders;
use lamp_api::state::AppState;
use serde_json::{json, Value};

use common::{
    admin_request, admin_router, audit_rows, boot_admin, minted_session_cookie, send, ScratchDb,
    PRODUCT_NOT_FOUND_404,
};

// ---------------------------------------------------------------------------
// Exact bodies the handlers guarantee (asserted byte-for-byte).
// ---------------------------------------------------------------------------

const INSUFFICIENT_STOCK_409: &str = r#"{"error":"insufficient stock available"}"#;
const ORDER_NOT_FOUND_404: &str = r#"{"error":"order not found"}"#;
const PAYMENT_MISMATCH_400: &str = r#"{"error":"payment amount or currency mismatch"}"#;
const INVALID_SIGNATURE_401: &str = r#"{"error":"invalid stripe signature"}"#;
const FPS_REFERENCE_REQUIRED_400: &str = r#"{"error":"fps_reference is required"}"#;
const NOT_RESERVED_409: &str = r#"{"error":"order is not reserved"}"#;
const NOT_SOLD_409: &str = r#"{"error":"order is not paid or fulfilled"}"#;

// ---------------------------------------------------------------------------
// Probes (seeded SKUs from 0003: slug -> (units, price_cents)).
// ---------------------------------------------------------------------------

/// 2 units @ 161500.
const RESERVE_SLUG: &str = "panthella-160-v3-beige";
/// 5 units @ 161500.
const WEBHOOK_SLUG: &str = "panthella-160-v3-white-opal";
/// 3 units @ 714000.
const EXPIRY_SLUG: &str = "ph-2-1-table-chrome";
/// 2 units @ 238000.
const QUARANTINE_SLUG: &str = "panthella-160-v3-opal-beige-chrome";
/// 1 unit @ 1037000.
const FPS_SLUG: &str = "ph-3-2-table-chrome";

const CUSTOMER_NAME: &str = "Ada Buyer";
const CUSTOMER_EMAIL: &str = "ada@example.com";
const FPS_REFERENCE: &str = "FPS-REF-0001-TEST";

/// Fixed test signing key (>= `config::MIN_STRIPE_WEBHOOK_SECRET_BYTES`).
const WEBHOOK_SECRET: &str = "d2c-orders-stripe-secret-0123456789abcdef";

// ---------------------------------------------------------------------------
// Boot / request helpers
// ---------------------------------------------------------------------------

/// The surface under test: the PUBLIC money path at the exact paths the
/// binary's `fn app()` mounts it, merged with the shared admin router (which
/// mounts the order admin surface behind `require_admin` + CSRF). The bin's
/// full wiring (CORS, trace layer) stays pinned by the `src/main.rs` tests.
fn orders_router(state: AppState) -> Router {
    Router::new()
        .route("/api/orders/reserve", post(orders::create_reservation))
        .route("/api/orders/{order_code}", get(orders::get_order_by_code))
        .route(
            "/api/orders/{order_code}/email-delivery",
            post(orders::record_email_delivery),
        )
        .route("/api/webhooks/stripe", post(orders::stripe_webhook))
        .with_state(state.clone())
        .merge(admin_router(state))
}

/// The webhook handler resolves `STRIPE_WEBHOOK_SECRET` from the process
/// environment — the real configuration path (`config::stripe_webhook_secret`)
/// and deliberately NOT a test seam. Mutating the environment from parallel
/// test threads would race, so the secret is armed EXACTLY ONCE per test
/// binary and always with the same value: the `Once` makes that single write
/// happen-before every webhook request, and no other test in this binary
/// reads that key.
fn arm_webhook_secret() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("STRIPE_WEBHOOK_SECRET", WEBHOOK_SECRET));
}

/// Boot + router + admin cookie for one suite test.
fn boot_orders(db: &ScratchDb) -> (Router, String) {
    let state = boot_admin(db);
    let cookie = minted_session_cookie(&state);
    (orders_router(state), cookie)
}

fn json_request(method: Method, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("test request must build")
}

/// One request through the router on the harness runtime: (status, body).
fn call(db: &ScratchDb, router: &Router, req: Request<Body>) -> (StatusCode, String) {
    let (status, _, body) = db.block_on(send(router, req));
    (status, body)
}

/// One cookie-authenticated admin request (the CSRF header rides along via
/// `common::admin_request`).
fn admin_call(
    db: &ScratchDb,
    router: &Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    call(db, router, admin_request(method, uri, Some(cookie), body))
}

/// `POST /api/orders/reserve` body: the full contract shape.
fn reserve_body(slug: &str, quantity: i32, payment_method: &str) -> String {
    json!({
        "slug": slug,
        "quantity": quantity,
        "payment_method": payment_method,
        "customer_name": CUSTOMER_NAME,
        "customer_email": CUSTOMER_EMAIL,
        "customer_phone": "+852 0000 0000",
        "delivery_address": "1 Test Road",
    })
    .to_string()
}

/// Reserves and asserts the 200, returning the parsed body.
fn reserve(
    db: &ScratchDb,
    router: &Router,
    slug: &str,
    quantity: i32,
    payment_method: &str,
) -> Value {
    let (status, body) = call(
        db,
        router,
        json_request(
            Method::POST,
            "/api/orders/reserve",
            &reserve_body(slug, quantity, payment_method),
        ),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "reservation of {quantity} x {slug} must be accepted: {body}"
    );
    serde_json::from_str(&body).expect("reservation response must be JSON")
}

/// `POST /api/webhooks/stripe` with a correctly signed payload (arms the test
/// secret first), returning (status, body).
fn stripe_event(
    db: &ScratchDb,
    router: &Router,
    event_id: &str,
    order_code: &str,
    amount_cents: i64,
    currency: &str,
) -> (StatusCode, String) {
    arm_webhook_secret();
    let payload = stripe_payload(event_id, order_code, amount_cents, currency);
    let signature = orders::stripe_signature_header(
        WEBHOOK_SECRET.as_bytes(),
        1_700_000_000,
        payload.as_bytes(),
    );
    call(db, router, webhook_request(&payload, &signature))
}

fn stripe_payload(event_id: &str, order_code: &str, amount_cents: i64, currency: &str) -> String {
    json!({
        "event_id": event_id,
        "event_type": "payment_intent.succeeded",
        "order_code": order_code,
        "amount_cents": amount_cents,
        "currency": currency,
    })
    .to_string()
}

fn webhook_request(payload: &str, signature: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/webhooks/stripe")
        .header("content-type", "application/json")
        .header("stripe-signature", signature)
        .body(Body::from(payload.to_string()))
        .expect("webhook request must build")
}

// ---------------------------------------------------------------------------
// DB witnesses (direct SQL: the row is the truth, never the handler's answer)
// ---------------------------------------------------------------------------

/// `(stock_total, stock_reserved, stock_quarantined)` for one slug.
fn stock(db: &ScratchDb, slug: &str) -> (i32, i32, i32) {
    db.block_on(async {
        sqlx::query_as::<_, (i32, i32, i32)>(
            "SELECT stock_total, stock_reserved, stock_quarantined \
             FROM products WHERE slug = $1",
        )
        .bind(slug)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|e| panic!("stock lookup failed for {slug}: {e}"))
    })
}

/// The saleable count — the number the whole module exists to protect.
fn available(db: &ScratchDb, slug: &str) -> i64 {
    let (total, reserved, quarantined) = stock(db, slug);
    i64::from(total) - i64::from(reserved) - i64::from(quarantined)
}

/// `(id, status, fps_reference)` for one order code.
fn order_row(db: &ScratchDb, order_code: &str) -> (i64, String, String) {
    db.block_on(async {
        sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, status, fps_reference FROM orders WHERE order_code = $1",
        )
        .bind(order_code)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("order lookup failed for {order_code}: {e}"))
        .unwrap_or_else(|| panic!("order {order_code} must exist"))
    })
}

fn order_status(db: &ScratchDb, order_code: &str) -> String {
    order_row(db, order_code).1
}

fn payment_event_count(db: &ScratchDb) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM payment_events")
            .fetch_one(db.pool())
            .await
            .expect("payment_events count failed")
    })
}

/// Moves one reservation's deadline into the past — the honest way to test an
/// expiry without sleeping or shortening the (clamped) TTL knobs.
fn force_expiry(db: &ScratchDb, order_code: &str) {
    db.block_on(async {
        sqlx::query(
            "UPDATE orders SET expires_at = now() - interval '1 minute' WHERE order_code = $1",
        )
        .bind(order_code)
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("forcing expiry failed for {order_code}: {e}"))
    });
}

/// The audit row for one action, panicking when absent.
fn audit_for(db: &ScratchDb, action: &str) -> common::AuditRow {
    audit_rows(db)
        .into_iter()
        .find(|row| row.1 == action)
        .unwrap_or_else(|| panic!("an audit row with action {action} must exist"))
}

/// Asserts an RFC3339 timestamp is in the future (reservation deadlines are
/// clock values — never comparable to a literal).
fn assert_in_future(raw: &str) {
    let parsed = DateTime::parse_from_rfc3339(raw).expect("timestamp must be RFC3339");
    assert!(
        parsed.with_timezone(&Utc) > Utc::now() - chrono::Duration::seconds(5),
        "{raw} must be in the future"
    );
}

/// Asserts the FULL webhook-response contract (findings G02): the PAY-01
/// verdict fields plus the recipient/email metadata the web tier needs to
/// address and dedupe the paid-confirmation email. Whole-value equality —
/// no field may go missing and none may appear from nowhere — with runtime
/// data (the order code) compared exactly.
fn assert_webhook_response(
    body: &str,
    idempotent: bool,
    status: &str,
    order_code: &str,
    locale: &str,
    paid_email_sent: bool,
) {
    let actual: Value = serde_json::from_str(body).expect("webhook response must be JSON");
    let expected = json!({
        "ok": true,
        "idempotent": idempotent,
        "status": status,
        "order_code": order_code,
        "customer_name": CUSTOMER_NAME,
        "customer_email": CUSTOMER_EMAIL,
        "locale": locale,
        "paid_email_sent": paid_email_sent,
    });
    assert_eq!(actual, expected, "webhook response contract: {body}");
}

/// One extra-field / extra-header reserve probe (idempotency and locale
/// inputs ride alongside the plain [`reserve_body`]).
fn reserve_raw(
    db: &ScratchDb,
    router: &Router,
    body: &str,
    header: Option<(&str, &str)>,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/orders/reserve")
        .header("content-type", "application/json");
    if let Some((name, value)) = header {
        builder = builder.header(name, value);
    }
    let request = builder
        .body(Body::from(body.to_string()))
        .expect("reserve request must build");
    call(db, router, request)
}

/// `(locale, idempotency_key, paid_email_sent_at, email_last_error)` for one
/// order — direct SQL: the row is the truth, never the handler's answer.
fn order_email_state(
    db: &ScratchDb,
    order_code: &str,
) -> (String, Option<String>, Option<DateTime<Utc>>, String) {
    db.block_on(async {
        sqlx::query_as::<_, (String, Option<String>, Option<DateTime<Utc>>, String)>(
            "SELECT locale, idempotency_key, paid_email_sent_at, email_last_error \
             FROM orders WHERE order_code = $1",
        )
        .bind(order_code)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("order lookup failed for {order_code}: {e}"))
        .unwrap_or_else(|| panic!("order {order_code} must exist"))
    })
}

/// `paid_email_claimed_at` for one order — the RECOVERABLE in-flight claim
/// lease ("a send is in flight", never delivery proof), read straight from
/// Postgres so the handler can never lie about the claim state.
fn order_paid_email_claimed_at(db: &ScratchDb, order_code: &str) -> Option<DateTime<Utc>> {
    let (claimed_at,) = db.block_on(async {
        sqlx::query_as::<_, (Option<DateTime<Utc>>,)>(
            "SELECT paid_email_claimed_at FROM orders WHERE order_code = $1",
        )
        .bind(order_code)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("order lookup failed for {order_code}: {e}"))
        .unwrap_or_else(|| panic!("order {order_code} must exist"))
    });
    claimed_at
}

/// Backdates `paid_email_claimed_at` (simulates a claim lease aging without
/// sleeping): the 2-minute RECOVERY window must expire in the database clock,
/// not in wall time the test would have to wait out.
fn backdate_paid_email_claim(db: &ScratchDb, order_code: &str, minutes: i64) {
    db.block_on(async {
        sqlx::query(
            "UPDATE orders SET paid_email_claimed_at = now() - ($2 || ' minutes')::interval \
             WHERE order_code = $1",
        )
        .bind(order_code)
        .bind(minutes.to_string())
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("claim backdate failed for {order_code}: {e}"));
    });
}

// ---------------------------------------------------------------------------
// 1. Atomic reservations (STOCK-01)
// ---------------------------------------------------------------------------

#[test]
fn reservation_reserves_stock_atomically_and_refuses_oversell() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_reserve_stock");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // Seed state (0003): 2 units, none reserved, none quarantined.
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 0, 0));

    // Reserve 1 — with a client-side price that must be IGNORED (money is
    // never taken from the client: the DB row just locked is the price list).
    let mut body: Value = serde_json::from_str(&reserve_body(RESERVE_SLUG, 1, "card")).unwrap();
    body["unit_price_cents"] = json!(1);
    body["total_cents"] = json!(1);
    let (status, response) = call(
        &db,
        &router,
        json_request(Method::POST, "/api/orders/reserve", &body.to_string()),
    );
    assert_eq!(status, StatusCode::OK, "{response}");
    let order: Value = serde_json::from_str(&response).expect("reservation response must be JSON");
    assert_eq!(order["unit_price_cents"], json!(161_500));
    assert_eq!(order["total_cents"], json!(161_500));
    assert_eq!(order["quantity"], json!(1));
    assert_eq!(order["product_slug"], json!(RESERVE_SLUG));
    assert_eq!(order["status"], json!("reserved"));
    assert_eq!(order["currency"], json!(config::default_currency()));
    assert_eq!(order["customer_name"], json!(CUSTOMER_NAME));
    assert_in_future(order["expires_at"].as_str().expect("expires_at"));
    let order_code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let order_id = order["id"].as_i64().expect("id");

    // The DB agrees: exactly one unit moved into the reserved pool.
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 1, 0));

    // Public status read: no-store (live stock state) and NO customer PII.
    let request = json_request(Method::GET, &format!("/api/orders/{order_code}"), "");
    let (status, headers, body) = db.block_on(send(&router, request));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-store")
    );
    let public: Value = serde_json::from_str(&body).expect("public order status must be JSON");
    assert_eq!(public["order_code"], json!(order_code));
    assert_eq!(public["status"], json!("reserved"));
    for pii in [
        "customer_name",
        "customer_email",
        "customer_phone",
        "delivery_address",
    ] {
        assert!(
            public.get(pii).is_none(),
            "public status must not carry {pii}"
        );
    }

    // Oversell boundary: available is 1, so 2 is refused, 1 lands, 1 more is
    // refused again at exactly 0 available.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            "/api/orders/reserve",
            &reserve_body(RESERVE_SLUG, 2, "card"),
        ),
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, INSUFFICIENT_STOCK_409);
    assert_eq!(
        stock(&db, RESERVE_SLUG),
        (2, 1, 0),
        "a refused order reserves nothing"
    );

    let second = reserve(&db, &router, RESERVE_SLUG, 1, "fps");
    assert_eq!(second["total_cents"], json!(161_500));
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 2, 0));

    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            "/api/orders/reserve",
            &reserve_body(RESERVE_SLUG, 1, "card"),
        ),
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, INSUFFICIENT_STOCK_409);
    assert_eq!(available(&db, RESERVE_SLUG), 0);

    // Unknown and hidden SKUs answer identically (404): no oracle.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            "/api/orders/reserve",
            &reserve_body("no-such-slug", 1, "card"),
        ),
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, PRODUCT_NOT_FOUND_404);

    // The `settings` `currency` override wins over the env/code default
    // (CONFIG-01: override -> env -> code default).
    db.block_on(async {
        sqlx::query("INSERT INTO settings (key, value) VALUES ('currency', 'USD')")
            .execute(db.pool())
            .await
            .expect("settings override insert failed")
    });
    let order = reserve(&db, &router, WEBHOOK_SLUG, 1, "card");
    assert_eq!(order["currency"], json!("USD"));
    assert_ne!(
        order["id"].as_i64(),
        Some(order_id),
        "every reservation is its own order"
    );
}

// ---------------------------------------------------------------------------
// 2. Stripe webhook authority + PAY-01 idempotency
// ---------------------------------------------------------------------------

#[test]
fn stripe_webhook_settles_a_reservation_once_and_replays_are_no_ops() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_webhook_pay");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    let order = reserve(&db, &router, WEBHOOK_SLUG, 2, "card");
    let order_code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let total_cents = order["total_cents"].as_i64().expect("total_cents");
    let currency = order["currency"].as_str().expect("currency").to_string();
    assert_eq!(total_cents, 323_000); // 161500 x 2, computed server-side
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 2, 0));

    // A body modified under a valid signature is forged: exact 401, and the
    // ledger stays empty (an unverified event never reaches the DB).
    let signed = stripe_payload("evt_forged", &order_code, total_cents, &currency);
    let signature = orders::stripe_signature_header(
        WEBHOOK_SECRET.as_bytes(),
        1_700_000_000,
        signed.as_bytes(),
    );
    arm_webhook_secret();
    let tampered = signed.replace("\"amount_cents\":323000", "\"amount_cents\":1");
    let (status, body) = call(&db, &router, webhook_request(&tampered, &signature));
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body, INVALID_SIGNATURE_401);
    assert_eq!(payment_event_count(&db), 0);

    // A verified event that does not match the order's (amount, currency) is
    // refused: the signature alone must not settle somebody else's total.
    let (status, body) = stripe_event(
        &db,
        &router,
        "evt_mismatch",
        &order_code,
        total_cents + 1,
        &currency,
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, PAYMENT_MISMATCH_400);
    assert_eq!(
        payment_event_count(&db),
        0,
        "a refused event is never recorded"
    );
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 2, 0));

    // The verified event: reserved -> paid, units consumed (PAY-01).
    let (status, body) = stripe_event(&db, &router, "evt_1", &order_code, total_cents, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &order_code, "en", false);
    assert_eq!(order_status(&db, &order_code), "paid");
    assert_eq!(stock(&db, WEBHOOK_SLUG), (3, 0, 0)); // 5 - 2 sold, 0 held
    assert_eq!(payment_event_count(&db), 1);

    // PAY-01 replay: same event_id -> `idempotent: true` and NOTHING moves.
    let (status, body) = stripe_event(&db, &router, "evt_1", &order_code, total_cents, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, true, "paid", &order_code, "en", false);
    assert_eq!(
        stock(&db, WEBHOOK_SLUG),
        (3, 0, 0),
        "a replay deducts nothing"
    );
    assert_eq!(payment_event_count(&db), 1);

    // A NEW event on the already-paid order: recorded, but stock never moves
    // twice (the state rules only settle a live reservation).
    let (status, body) = stripe_event(&db, &router, "evt_2", &order_code, total_cents, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &order_code, "en", false);
    assert_eq!(
        stock(&db, WEBHOOK_SLUG),
        (3, 0, 0),
        "settled orders deduct once"
    );
    assert_eq!(payment_event_count(&db), 2);

    // An unknown order code is the exact 404.
    let (status, body) = stripe_event(&db, &router, "evt_3", "ORD-nope-1", total_cents, &currency);
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, ORDER_NOT_FOUND_404);
}

// ---------------------------------------------------------------------------
// 3. Expiry release + late payment (STOCK-01 / R2-5)
// ---------------------------------------------------------------------------

#[test]
fn expired_reservation_is_released_and_late_payment_is_flagged() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_expiry_late");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // An FPS reservation (24h window), then force its deadline into the past.
    let first = reserve(&db, &router, EXPIRY_SLUG, 1, "fps");
    let first_code = first["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_eq!(stock(&db, EXPIRY_SLUG), (3, 1, 0));
    force_expiry(&db, &first_code);

    // The NEXT reservation sweeps it: the lapsed order is `expired` and its
    // unit is back in the saleable pool — a release is NOT a restock, the
    // unit never left the building.
    let second = reserve(&db, &router, EXPIRY_SLUG, 1, "card");
    let second_code = second["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_eq!(order_status(&db, &first_code), "expired");
    assert_eq!(stock(&db, EXPIRY_SLUG), (3, 1, 0), "released + re-reserved");

    // The public read sweeps too (the read must answer against the clock).
    force_expiry(&db, &second_code);
    let request = json_request(Method::GET, &format!("/api/orders/{second_code}"), "");
    let (status, _, body) = db.block_on(send(&router, request));
    assert_eq!(status, StatusCode::OK, "{body}");
    let public: Value = serde_json::from_str(&body).expect("public order status must be JSON");
    assert_eq!(public["status"], json!("expired"));
    assert_eq!(
        stock(&db, EXPIRY_SLUG),
        (3, 0, 0),
        "the read released the hold"
    );

    // R2-5: a verified payment against the EXPIRED first order is flagged for
    // review/refund — it never silently fulfils against released stock.
    let first_total = first["total_cents"].as_i64().expect("total_cents");
    let first_currency = first["currency"].as_str().expect("currency").to_string();
    let (status, body) = stripe_event(
        &db,
        &router,
        "evt_late",
        &first_code,
        first_total,
        &first_currency,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(
        &body,
        false,
        "late_payment_review",
        &first_code,
        "en",
        false,
    );
    assert_eq!(order_status(&db, &first_code), "late_payment_review");
    assert_eq!(
        stock(&db, EXPIRY_SLUG),
        (3, 0, 0),
        "a late payment consumes no stock"
    );
    assert_eq!(payment_event_count(&db), 1, "the late payment IS recorded");
}

// ---------------------------------------------------------------------------
// 4. Returned units quarantine (STOCK-01 / R2-11)
// ---------------------------------------------------------------------------

#[test]
fn quarantine_return_quarantines_units_without_increasing_availability() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_quarantine");
    db.apply_migrations();
    let (router, cookie) = boot_orders(&db);

    // Sell one unit end to end (reserve -> verified payment).
    let order = reserve(&db, &router, QUARANTINE_SLUG, 1, "card");
    let order_id = order["id"].as_i64().expect("id");
    let order_code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let total_cents = order["total_cents"].as_i64().expect("total_cents");
    let currency = order["currency"].as_str().expect("currency").to_string();
    assert_eq!(available(&db, QUARANTINE_SLUG), 1, "2 seeded - 1 reserved");
    let (status, body) = stripe_event(
        &db,
        &router,
        "evt_sold",
        &order_code,
        total_cents,
        &currency,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(stock(&db, QUARANTINE_SLUG), (1, 0, 0));
    assert_eq!(available(&db, QUARANTINE_SLUG), 1);

    // R2-11: the returned unit comes back into `stock_total` but lands in
    // `stock_quarantined` — the SALEABLE count is exactly unchanged.
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order_id}/quarantine-return"),
        "",
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let returned: Value = serde_json::from_str(&body).expect("admin order must be JSON");
    assert_eq!(returned["status"], json!("returned_quarantined"));
    assert_eq!(stock(&db, QUARANTINE_SLUG), (2, 0, 1));
    assert_eq!(
        available(&db, QUARANTINE_SLUG),
        1,
        "a quarantined unit is back in the building, never back on sale"
    );

    let (actor, action, entity, entity_id, detail) = audit_for(&db, "quarantine_return");
    assert_eq!(actor, common::ADMIN_USERNAME);
    assert_eq!(action, "quarantine_return");
    assert_eq!(entity, "orders");
    assert_eq!(entity_id, Some(order_id.to_string()));
    assert_eq!(detail["order_code"], json!(order_code));
    assert_eq!(detail["quantity"], json!(1));

    // A second return of the same order is refused and changes nothing.
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order_id}/quarantine-return"),
        "",
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, NOT_SOLD_409);
    assert_eq!(stock(&db, QUARANTINE_SLUG), (2, 0, 1));

    // The admin list serves the newest order first, no-store.
    let (status, headers, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/orders", Some(cookie.as_str()), ""),
    ));
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        headers.get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-store")
    );
    let orders: Vec<Value> = serde_json::from_str(&body).expect("orders list must be JSON");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0]["id"], json!(order_id));
    assert_eq!(orders[0]["status"], json!("returned_quarantined"));
}

// ---------------------------------------------------------------------------
// 5. FPS confirmation (R5a admin leg)
// ---------------------------------------------------------------------------

#[test]
fn fps_confirmation_settles_live_orders_and_flags_late_transfers() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_fps_confirm");
    db.apply_migrations();
    let (router, cookie) = boot_orders(&db);

    // Live FPS reservation -> confirmed -> paid, units consumed, audited.
    let order = reserve(&db, &router, FPS_SLUG, 1, "fps");
    let order_id = order["id"].as_i64().expect("id");
    let order_code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_eq!(stock(&db, FPS_SLUG), (1, 1, 0));

    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order_id}/confirm-fps"),
        &json!({ "fps_reference": FPS_REFERENCE }).to_string(),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let confirmed: Value = serde_json::from_str(&body).expect("admin order must be JSON");
    assert_eq!(confirmed["status"], json!("paid"));
    assert_eq!(confirmed["fps_reference"], json!(FPS_REFERENCE));
    assert_eq!(order_status(&db, &order_code), "paid");
    assert_eq!(stock(&db, FPS_SLUG), (0, 0, 0), "the reserved unit is sold");

    let (actor, action, entity, entity_id, detail) = audit_for(&db, "confirm_fps");
    assert_eq!(actor, common::ADMIN_USERNAME);
    assert_eq!(action, "confirm_fps");
    assert_eq!(entity, "orders");
    assert_eq!(entity_id, Some(order_id.to_string()));
    assert_eq!(detail["fps_reference"], json!(FPS_REFERENCE));
    assert_eq!(detail["order_code"], json!(order_code));

    // R2-5 again, the manual-transfer leg: a confirmation against an EXPIRED
    // reservation is flagged for review and consumes no stock.
    let late = reserve(&db, &router, QUARANTINE_SLUG, 1, "fps");
    let late_id = late["id"].as_i64().expect("id");
    let late_code = late["order_code"].as_str().expect("order_code").to_string();
    force_expiry(&db, &late_code);
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{late_id}/confirm-fps"),
        &json!({ "fps_reference": "FPS-LATE-0002" }).to_string(),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let flagged: Value = serde_json::from_str(&body).expect("admin order must be JSON");
    assert_eq!(flagged["status"], json!("late_payment_review"));
    assert_eq!(flagged["fps_reference"], json!("FPS-LATE-0002"));
    assert_eq!(
        stock(&db, QUARANTINE_SLUG),
        (2, 0, 0),
        "a late FPS confirmation consumes no stock"
    );
    let (_, action, _, _, _) = audit_for(&db, "late_fps_review");
    assert_eq!(action, "late_fps_review");

    // Empty reference: 400 before any DB work (and no audit row for it).
    let before = audit_rows(&db).len();
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{late_id}/confirm-fps"),
        &json!({ "fps_reference": "   " }).to_string(),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, FPS_REFERENCE_REQUIRED_400);
    assert_eq!(
        audit_rows(&db).len(),
        before,
        "a rejected write audits nothing"
    );

    // Missing order: the exact 404.
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        "/api/admin/orders/999999/confirm-fps",
        &json!({ "fps_reference": FPS_REFERENCE }).to_string(),
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, ORDER_NOT_FOUND_404);
}

// ---------------------------------------------------------------------------
// 6. Release (STOCK-01: a release is not a restock)
// ---------------------------------------------------------------------------

#[test]
fn release_cancels_a_reservation_and_frees_its_units() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_release");
    db.apply_migrations();
    let (router, cookie) = boot_orders(&db);

    let order = reserve(&db, &router, WEBHOOK_SLUG, 2, "card");
    let order_id = order["id"].as_i64().expect("id");
    let order_code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 2, 0));

    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order_id}/release"),
        "",
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let released: Value = serde_json::from_str(&body).expect("admin order must be JSON");
    assert_eq!(released["status"], json!("cancelled"));
    assert_eq!(order_status(&db, &order_code), "cancelled");
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 0, 0), "the hold is released");

    let (actor, action, entity, entity_id, detail) = audit_for(&db, "release");
    assert_eq!(actor, common::ADMIN_USERNAME);
    assert_eq!(action, "release");
    assert_eq!(entity, "orders");
    assert_eq!(entity_id, Some(order_id.to_string()));
    assert_eq!(detail["order_code"], json!(order_code));
    assert_eq!(detail["quantity"], json!(2));

    // A cancelled order cannot be released again: exact 409, no second audit,
    // and the released units stay released (never double-counted).
    let audits = audit_rows(&db).len();
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order_id}/release"),
        "",
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, NOT_RESERVED_409);
    assert_eq!(audit_rows(&db).len(), audits);
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 0, 0));

    // Releasing an unknown order is the exact 404.
    let (status, body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        "/api/admin/orders/999999/release",
        "",
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, ORDER_NOT_FOUND_404);

    // And a cancelled order's late payment is still flagged, never fulfilled
    // (R2-5 covers `cancelled` alongside `expired`).
    let total_cents = order["total_cents"].as_i64().expect("total_cents");
    let currency = order["currency"].as_str().expect("currency").to_string();
    let (status, body) = stripe_event(
        &db,
        &router,
        "evt_cancelled",
        &order_code,
        total_cents,
        &currency,
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(
        &body,
        false,
        "late_payment_review",
        &order_code,
        "en",
        false,
    );
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 0, 0), "no stock is consumed");
}

// ---------------------------------------------------------------------------
// 7. Multi-SKU Shopping Cart atomic reservation, settlement & release (CART-01)
// ---------------------------------------------------------------------------

#[test]
fn multi_sku_shopping_cart_reserves_settles_and_releases_all_items_atomically() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_multi_sku_cart");
    db.apply_migrations();
    let (router, cookie) = boot_orders(&db);

    // Reserve ALL 8 SKUs in one Shopping Cart transaction (CART-01).
    let all_skus = [
        ("panthella-160-v3-beige", 1, 161_500_i64),
        ("panthella-160-v3-white-opal", 2, 161_500_i64),
        ("panthella-160-v3-opal-beige-chrome", 1, 238_000_i64),
        ("panthella-160-v3-opal-pale-blue", 1, 161_500_i64),
        ("panthella-160-v3-opal-pale-rose", 1, 161_500_i64),
        ("ph-2-1-table-chrome", 2, 714_000_i64),
        ("ph-3-2-table-chrome", 1, 1_037_000_i64),
        ("ph-1-1-piano-table-limited", 1, 1_079_500_i64),
    ];
    let expected_total_cents: i64 = all_skus
        .iter()
        .map(|(_, qty, price)| i64::from(*qty) * *price)
        .sum();
    let expected_total_qty: i32 = all_skus.iter().map(|(_, qty, _)| *qty).sum();

    let items_json: Vec<Value> = all_skus
        .iter()
        .map(|(slug, qty, _)| json!({ "slug": slug, "quantity": qty }))
        .collect();

    let body = json!({
        "items": items_json,
        "payment_method": "card",
        "customer_name": CUSTOMER_NAME,
        "customer_email": CUSTOMER_EMAIL,
        "customer_phone": "+852 9000 0000",
        "delivery_address": "10 Queen's Road Central",
    })
    .to_string();

    let (status, resp_body) = call(
        &db,
        &router,
        json_request(Method::POST, "/api/orders/reserve", &body),
    );
    assert_eq!(status, StatusCode::OK, "{resp_body}");
    let order: Value = serde_json::from_str(&resp_body).expect("must be JSON");
    assert_eq!(order["total_cents"], json!(expected_total_cents));
    assert_eq!(order["quantity"], json!(expected_total_qty));
    assert_eq!(order["items"].as_array().unwrap().len(), 8);

    // Every one of the 8 SKUs now has its reserved count bumped in Postgres.
    for (slug, qty, _) in &all_skus {
        let (_, reserved, _) = stock(&db, slug);
        assert_eq!(reserved, *qty, "reserved count for {slug}");
    }

    // Settle the 8-SKU cart via verified Stripe webhook: all 8 SKUs consume stock.
    let order_code = order["order_code"].as_str().unwrap().to_string();
    let currency = order["currency"].as_str().unwrap().to_string();
    let (status, wh_body) = stripe_event(
        &db,
        &router,
        "evt_multi_8_skus",
        &order_code,
        expected_total_cents,
        &currency,
    );
    assert_eq!(status, StatusCode::OK, "{wh_body}");
    for (slug, _, _) in &all_skus {
        let (_, reserved, _) = stock(&db, slug);
        assert_eq!(reserved, 0, "reserved consumed for {slug}");
    }

    // Also verify releasing a 2-SKU cart frees both SKUs atomically.
    let two_sku_body = json!({
        "items": [
            { "slug": "panthella-160-v3-beige", "quantity": 1 },
            { "slug": "ph-2-1-table-chrome", "quantity": 1 }
        ],
        "payment_method": "fps",
        "customer_name": CUSTOMER_NAME,
        "customer_email": CUSTOMER_EMAIL,
    })
    .to_string();
    let (status, resp2) = call(
        &db,
        &router,
        json_request(Method::POST, "/api/orders/reserve", &two_sku_body),
    );
    assert_eq!(status, StatusCode::OK, "{resp2}");
    let order2: Value = serde_json::from_str(&resp2).unwrap();
    let order2_id = order2["id"].as_i64().unwrap();
    assert_eq!(stock(&db, "panthella-160-v3-beige"), (1, 1, 0));
    assert_eq!(stock(&db, "ph-2-1-table-chrome"), (1, 1, 0));

    let (status, rel_body) = admin_call(
        &db,
        &router,
        &cookie,
        Method::POST,
        &format!("/api/admin/orders/{order2_id}/release"),
        "",
    );
    assert_eq!(status, StatusCode::OK, "{rel_body}");
    assert_eq!(stock(&db, "panthella-160-v3-beige"), (1, 0, 0));
    assert_eq!(stock(&db, "ph-2-1-table-chrome"), (1, 0, 0));
}

// ---------------------------------------------------------------------------
// 8. Reservation idempotency — one order per pay tap (findings G01)
// ---------------------------------------------------------------------------

#[test]
fn reservation_idempotency_key_creates_one_order_per_pay_attempt() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_idempotency");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // One pay tap = one order = one held unit (RESERVE_SLUG seeds 2 units).
    let mut body: Value = serde_json::from_str(&reserve_body(RESERVE_SLUG, 1, "card")).unwrap();
    body["idempotency_key"] = json!("pay-attempt-1");
    let (status, first) = reserve_raw(&db, &router, &body.to_string(), None);
    assert_eq!(status, StatusCode::OK, "{first}");
    let order_a: Value = serde_json::from_str(&first).expect("reservation response must be JSON");
    let code_a = order_a["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let id_a = order_a["id"].as_i64().expect("id");
    assert_eq!(
        order_a["idempotent_replay"],
        json!(false),
        "a fresh insert is never a replay"
    );
    assert_eq!(
        stock(&db, RESERVE_SLUG),
        (2, 1, 0),
        "one tap holds one unit"
    );

    // The response is lost and the client retries the SAME pay attempt: the
    // very same order comes back and NOTHING is held a second time — the
    // ghost-order scenario from findings G01 is closed.
    let (status, retry) = reserve_raw(&db, &router, &body.to_string(), None);
    assert_eq!(status, StatusCode::OK, "{retry}");
    let order_b: Value = serde_json::from_str(&retry).expect("reservation response must be JSON");
    assert_eq!(order_b["order_code"], json!(code_a));
    assert_eq!(order_b["id"], json!(id_a));
    assert_eq!(order_b["items"], order_a["items"]);
    assert_eq!(order_b["total_cents"], order_a["total_cents"]);
    assert_eq!(
        order_b["idempotent_replay"],
        json!(true),
        "a replayed attempt is flagged so the web tier sends no second email"
    );
    assert_eq!(
        stock(&db, RESERVE_SLUG),
        (2, 1, 0),
        "a retry never reserves twice"
    );

    // Key normalisation: a padded spelling is the SAME attempt.
    body["idempotency_key"] = json!("  pay-attempt-1  ");
    let (status, padded) = reserve_raw(&db, &router, &body.to_string(), None);
    assert_eq!(status, StatusCode::OK, "{padded}");
    let padded_order: Value =
        serde_json::from_str(&padded).expect("reservation response must be JSON");
    assert_eq!(padded_order["order_code"], json!(code_a));
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 1, 0));

    // The body key WINS over a conflicting `Idempotency-Key` header (the
    // header is only a fallback), so this is still pay attempt 1.
    body["idempotency_key"] = json!("pay-attempt-1");
    let (status, via_body) = reserve_raw(
        &db,
        &router,
        &body.to_string(),
        Some(("idempotency-key", "pay-attempt-2")),
    );
    assert_eq!(status, StatusCode::OK, "{via_body}");
    let via_body_order: Value =
        serde_json::from_str(&via_body).expect("reservation response must be JSON");
    assert_eq!(via_body_order["order_code"], json!(code_a));
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 1, 0));

    // Header fallback (G01): no body key at all -> the standard
    // `Idempotency-Key` header names the attempt. A DIFFERENT key IS a
    // different attempt: a new order holds the second unit — and retrying
    // THAT key replays that order.
    let hdr_body = reserve_body(RESERVE_SLUG, 1, "fps");
    let (status, second) = reserve_raw(
        &db,
        &router,
        &hdr_body,
        Some(("idempotency-key", "pay-attempt-2")),
    );
    assert_eq!(status, StatusCode::OK, "{second}");
    let order_c: Value = serde_json::from_str(&second).expect("reservation response must be JSON");
    let code_c = order_c["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_ne!(code_c, code_a, "a different key is a different attempt");
    assert_eq!(stock(&db, RESERVE_SLUG), (2, 2, 0));

    let (status, retry_c) = reserve_raw(
        &db,
        &router,
        &hdr_body,
        Some(("idempotency-key", "pay-attempt-2")),
    );
    assert_eq!(status, StatusCode::OK, "{retry_c}");
    let retry_c_order: Value =
        serde_json::from_str(&retry_c).expect("reservation response must be JSON");
    assert_eq!(retry_c_order["order_code"], json!(code_c));
    assert_eq!(retry_c_order["id"], order_c["id"]);
    assert_eq!(
        stock(&db, RESERVE_SLUG),
        (2, 2, 0),
        "the header-key retry holds nothing extra"
    );

    // Postgres agrees: exactly two orders, keyed 1:1 with the two attempts.
    let rows = db.block_on(async {
        sqlx::query_as::<_, (i64, String, Option<String>)>(
            "SELECT id, order_code, idempotency_key FROM orders ORDER BY id",
        )
        .fetch_all(db.pool())
        .await
        .expect("orders idempotency scan failed")
    });
    assert_eq!(rows.len(), 2, "one order per pay attempt: {rows:?}");
    assert_eq!(
        rows[0],
        (id_a, code_a.clone(), Some("pay-attempt-1".to_string()))
    );
    assert_eq!(rows[1].1, code_c);
    assert_eq!(rows[1].2.as_deref(), Some("pay-attempt-2"));
}

// ---------------------------------------------------------------------------
// 9. Order locale + paid-email delivery ledger (findings G02)
// ---------------------------------------------------------------------------

#[test]
fn order_locale_is_persisted_and_paid_email_metadata_round_trips() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_locale_email");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // A zh-HK checkout REMEMBERS its language (findings G02): on the
    // response and in Postgres, so the admin FPS email is not forced English.
    let mut zh_body: Value = serde_json::from_str(&reserve_body(RESERVE_SLUG, 1, "card")).unwrap();
    zh_body["locale"] = json!("zh-HK");
    let (status, zh) = reserve_raw(&db, &router, &zh_body.to_string(), None);
    assert_eq!(status, StatusCode::OK, "{zh}");
    let order_zh: Value = serde_json::from_str(&zh).expect("reservation response must be JSON");
    let code_zh = order_zh["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let total_zh = order_zh["total_cents"].as_i64().expect("total_cents");
    let currency_zh = order_zh["currency"].as_str().expect("currency").to_string();
    assert_eq!(order_zh["locale"], json!("zh-HK"));
    assert_eq!(order_email_state(&db, &code_zh).0, "zh-HK");

    // An omitted locale defaults to "en" — the pre-0008 contract unchanged.
    let order_en = reserve(&db, &router, WEBHOOK_SLUG, 1, "card");
    let code_en = order_en["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    assert_eq!(order_en["locale"], json!("en"));
    assert_eq!(order_email_state(&db, &code_en).0, "en");

    // The webhook response carries WHO to tell, in WHICH language, and
    // whether they have been told already — before any email: false.
    let (status, body) = stripe_event(&db, &router, "evt_zh_1", &code_zh, total_zh, &currency_zh);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &code_zh, "zh-HK", false);

    // The web tier reports the delivered email: one atomic statement stamps
    // the delivery ledger.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code_zh}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#
    );
    let (_, _, sent_at_zh, last_error) = order_email_state(&db, &code_zh);
    let sent_at_zh = sent_at_zh.expect("paid_email_sent_at must be set after a delivered email");
    assert_eq!(last_error, "", "a delivery carries no error");

    // A duplicate report (the classic double-send race) never moves the
    // original delivery moment — the dedupe flag is stable.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code_zh}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#
    );
    assert_eq!(
        order_email_state(&db, &code_zh).2,
        Some(sent_at_zh),
        "the delivery moment never moves"
    );

    // A later event on the settled order now says the customer WAS told.
    let (status, body) = stripe_event(&db, &router, "evt_zh_2", &code_zh, total_zh, &currency_zh);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &code_zh, "zh-HK", true);

    // Failure path: `sent:false` + error records the reason and leaves the
    // ledger open — no failure disappears silently (G02).
    let total_en = order_en["total_cents"].as_i64().expect("total_cents");
    let currency_en = order_en["currency"].as_str().expect("currency").to_string();
    let (status, body) = stripe_event(&db, &router, "evt_en_1", &code_en, total_en, &currency_en);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &code_en, "en", false);
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code_en}/email-delivery"),
            r#"{"sent":false,"error":"smtp down"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );
    let (_, _, sent_en, last_error) = order_email_state(&db, &code_en);
    assert_eq!(sent_en, None, "a failed send never claims delivery");
    assert_eq!(last_error, "smtp down");

    // The successful re-send clears the error and stamps the delivery.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code_en}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#
    );
    let (_, _, sent_en, last_error) = order_email_state(&db, &code_en);
    assert!(sent_en.is_some(), "the re-send stamps the delivery");
    assert_eq!(
        last_error, "",
        "a successful re-send clears the recorded error"
    );

    // Unknown order: the exact 404 — no ledger row is ever invented.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            "/api/orders/ORD-nope-1/email-delivery",
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, ORDER_NOT_FOUND_404);

    // A NUL in the error note is the exact 400 before any DB work.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code_zh}/email-delivery"),
            r#"{"sent":false,"error":"a\u0000b"}"#,
        ),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, r#"{"error":"error contains a NUL byte"}"#);
}

// ---------------------------------------------------------------------------
// 10. One pay tap racing for the LAST unit in stock (findings G01 recovery)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_same_key_retries_on_the_last_unit_share_one_order() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_idempotent_race");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // 0003 seeds FPS_SLUG with exactly ONE unit — both attempts want it.
    assert_eq!(stock(&db, FPS_SLUG), (1, 0, 0));

    let mut body: Value = serde_json::from_str(&reserve_body(FPS_SLUG, 1, "card")).unwrap();
    body["idempotency_key"] = json!("pay-attempt-last-unit");
    let raw = body.to_string();

    // ONE pay tap fired twice at the same moment (a lost response plus a
    // refresh / second-tab retry) racing for the last unit in stock. Whichever
    // way the race resolves — the UNIQUE-key loser or the insufficient-stock
    // re-check — BOTH callers must get the SAME order and the unit must be
    // held exactly once: a 409 for one retry would be a lost customer.
    let (first, second) = db.block_on(async {
        tokio::join!(
            send(
                &router,
                json_request(Method::POST, "/api/orders/reserve", &raw)
            ),
            send(
                &router,
                json_request(Method::POST, "/api/orders/reserve", &raw)
            ),
        )
    });
    let (status_a, _, body_a) = first;
    let (status_b, _, body_b) = second;
    assert_eq!(status_a, StatusCode::OK, "{body_a}");
    assert_eq!(status_b, StatusCode::OK, "{body_b}");
    let order_a: Value = serde_json::from_str(&body_a).expect("reservation response must be JSON");
    let order_b: Value = serde_json::from_str(&body_b).expect("reservation response must be JSON");
    assert_eq!(order_a["order_code"], order_b["order_code"]);
    assert_eq!(order_a["id"], order_b["id"]);
    assert_eq!(
        stock(&db, FPS_SLUG),
        (1, 1, 0),
        "one pay tap holds the last unit exactly once"
    );
}

// ---------------------------------------------------------------------------
// 11. Atomic paid-email claim & failure release (findings G02 exactly-once)
// ---------------------------------------------------------------------------

#[test]
fn paid_email_claim_is_atomic_and_a_failed_send_releases_it() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_email_claim");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    let mut body: Value = serde_json::from_str(&reserve_body(RESERVE_SLUG, 1, "card")).unwrap();
    body["idempotency_key"] = json!("pay-attempt-claim");
    let (status, first) = reserve_raw(&db, &router, &body.to_string(), None);
    assert_eq!(status, StatusCode::OK, "{first}");
    let order: Value = serde_json::from_str(&first).expect("reservation response must be JSON");
    let code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let total = order["total_cents"].as_i64().expect("total_cents");
    let currency = order["currency"].as_str().expect("currency").to_string();

    // Nothing to claim before settlement: the PAID email is meaningless on a
    // `reserved` order — no claim won, no stamp written.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );

    // Settle the order (the webhook-only path).
    let (status, body) = stripe_event(&db, &router, "evt_claim_1", &code, total, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_webhook_response(&body, false, "paid", &code, "en", false);

    // Exactly ONE claimer wins — the loser must not send a second receipt.
    // A won claim reports `paid_email_sent: false`: the claim is a RECOVERABLE
    // lease on the send, never delivery proof (`paid_email_sent_at` is only
    // stamped when the actual dispatch succeeds).
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body, r#"{"ok":true,"paid_email_sent":false,"claimed":true}"#,
        "the claim winner has NOT been recorded as delivered yet"
    );
    assert_eq!(
        order_email_state(&db, &code).2,
        None,
        "claiming alone must never stamp paid_email_sent_at"
    );
    assert!(
        order_paid_email_claimed_at(&db, &code).is_some(),
        "the winning claim is recorded as an in-flight lease"
    );
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body, r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#,
        "a second claimer must never send"
    );

    // A failed dispatch after claiming RELEASES the claim and records the
    // reason, so a retry or the admin re-send surface can still succeed.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":false,"error":"smtp down"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );
    let (_, _, sent_at, last_error) = order_email_state(&db, &code);
    assert_eq!(sent_at, None, "a failed send must never fake a delivery");
    assert_eq!(
        order_paid_email_claimed_at(&db, &code),
        None,
        "a failed send must release the claim"
    );
    assert_eq!(last_error, "smtp down");

    // The retry claims again — and delivers.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body, r#"{"ok":true,"paid_email_sent":false,"claimed":true}"#,
        "the released claim is claimable again"
    );
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#
    );
    let (_, _, sent_at, last_error) = order_email_state(&db, &code);
    assert!(
        sent_at.is_some(),
        "the successful retry stamps the delivery"
    );
    assert_eq!(
        order_paid_email_claimed_at(&db, &code),
        None,
        "a delivered send releases the claim"
    );
    assert_eq!(last_error, "", "a success clears the recorded error");

    // Re-sending a RESERVATION (hold) email on a `reserved` order must never
    // mark the PAID email as sent.
    let hold = reserve(&db, &router, RESERVE_SLUG, 1, "fps");
    let hold_code = hold["order_code"].as_str().expect("order_code").to_string();
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{hold_code}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );
    let (_, _, hold_sent, _) = order_email_state(&db, &hold_code);
    assert_eq!(
        hold_sent, None,
        "a hold email never stamps the paid-email ledger"
    );
}

// ---------------------------------------------------------------------------
// 12. Claim lease recovery, `kind` routing & delivery durability (round-2)
// ---------------------------------------------------------------------------

#[test]
fn paid_email_claim_lease_expires_and_reservation_kind_never_stamps() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "orders_email_claim_lease");
    db.apply_migrations();
    let (router, _cookie) = boot_orders(&db);

    // Settled order for the claim-lease + delivery-durability half.
    let order = reserve(&db, &router, RESERVE_SLUG, 1, "card");
    let code = order["order_code"]
        .as_str()
        .expect("order_code")
        .to_string();
    let total = order["total_cents"].as_i64().expect("total_cents");
    let currency = order["currency"].as_str().expect("currency").to_string();
    let (status, body) = stripe_event(&db, &router, "evt_lease_1", &code, total, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");

    // The claim is won as a lease — delivery is still unproven.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":true}"#
    );

    // While the lease is live, nobody else may take the send.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );

    // A concurrent admin re-send of the RESERVATION (hold) email must neither
    // stamp the PAID-email ledger nor steal/release the in-flight paid claim.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":true,"kind":"reservation"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );
    assert_eq!(
        order_email_state(&db, &code).2,
        None,
        "a reservation outcome must never stamp the paid-email ledger"
    );
    assert!(
        order_paid_email_claimed_at(&db, &code).is_some(),
        "a reservation outcome must not release the in-flight paid claim"
    );

    // Even a FAILED reservation re-send must leave the active paid claim intact.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":false,"kind":"reservation","error":"Reservation SMTP timeout"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        order_paid_email_claimed_at(&db, &code).is_some(),
        "a failed reservation outcome must not release the in-flight paid claim"
    );
    assert_eq!(
        order_email_state(&db, &code).3,
        "Reservation SMTP timeout",
        "failed reservation outcome still records email_last_error"
    );

    // A web tier that died mid-dispatch must not block the receipt forever:
    // once the 2-minute lease expires, the send is claimable again.
    backdate_paid_email_claim(&db, &code, 3);
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"claim":true,"sent":false}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body, r#"{"ok":true,"paid_email_sent":false,"claimed":true}"#,
        "an expired claim lease is recoverable"
    );

    // The recovered send delivers and stamps the confirmed moment.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":true}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    let confirmed_at = order_email_state(&db, &code)
        .2
        .expect("paid_email_sent_at must be set after a delivered email");

    // A FAILED later manual re-send records its reason but must NEVER wipe
    // the already-confirmed delivery moment.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":false,"error":"manual resend failed"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body, r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#,
        "a failed re-send never un-tells a customer who was already told"
    );
    let (_, _, sent_at, last_error) = order_email_state(&db, &code);
    assert_eq!(
        sent_at,
        Some(confirmed_at),
        "the confirmed delivery moment survives a failed re-send"
    );
    assert_eq!(last_error, "manual resend failed");

    // A later RESERVATION re-send clears the error note ONLY — the confirmed
    // paid-email moment stays exactly where it was.
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{code}/email-delivery"),
            r#"{"sent":true,"kind":"reservation"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":true,"claimed":false}"#
    );
    let (_, _, sent_at, last_error) = order_email_state(&db, &code);
    assert_eq!(
        sent_at,
        Some(confirmed_at),
        "a reservation outcome never moves the paid-email moment"
    );
    assert_eq!(
        last_error, "",
        "a successful reservation re-send clears the recorded error"
    );

    // On a RESERVED order (admin "Re-send Receipt" of the hold confirmation):
    // the failure is recorded, and the reservation success clears ONLY the
    // error — the PAID-email ledger is never touched.
    let hold = reserve(&db, &router, RESERVE_SLUG, 1, "fps");
    let hold_code = hold["order_code"].as_str().expect("order_code").to_string();
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{hold_code}/email-delivery"),
            r#"{"sent":false,"error":"smtp down"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(order_email_state(&db, &hold_code).3, "smtp down");
    let (status, body) = call(
        &db,
        &router,
        json_request(
            Method::POST,
            &format!("/api/orders/{hold_code}/email-delivery"),
            r#"{"sent":true,"kind":"reservation"}"#,
        ),
    );
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        r#"{"ok":true,"paid_email_sent":false,"claimed":false}"#
    );
    let (_, _, hold_sent, hold_error) = order_email_state(&db, &hold_code);
    assert_eq!(
        hold_sent, None,
        "a hold email never stamps the paid-email ledger"
    );
    assert_eq!(
        hold_error, "",
        "a delivered hold email clears the recorded error"
    );
    assert_eq!(
        order_paid_email_claimed_at(&db, &hold_code),
        None,
        "a reserved order never holds a paid-email claim"
    );
}
