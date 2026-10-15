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
    assert_eq!(body, r#"{"ok":true,"idempotent":false,"status":"paid"}"#);
    assert_eq!(order_status(&db, &order_code), "paid");
    assert_eq!(stock(&db, WEBHOOK_SLUG), (3, 0, 0)); // 5 - 2 sold, 0 held
    assert_eq!(payment_event_count(&db), 1);

    // PAY-01 replay: same event_id -> `idempotent: true` and NOTHING moves.
    let (status, body) = stripe_event(&db, &router, "evt_1", &order_code, total_cents, &currency);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, r#"{"ok":true,"idempotent":true,"status":"paid"}"#);
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
    assert_eq!(body, r#"{"ok":true,"idempotent":false,"status":"paid"}"#);
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
    assert_eq!(
        body,
        r#"{"ok":true,"idempotent":false,"status":"late_payment_review"}"#
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
    assert_eq!(
        body,
        r#"{"ok":true,"idempotent":false,"status":"late_payment_review"}"#
    );
    assert_eq!(stock(&db, WEBHOOK_SLUG), (5, 0, 0), "no stock is consumed");
}
