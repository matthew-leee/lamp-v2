//! The M1 money path (R5a): `POST /api/orders/reserve`,
//! `GET /api/orders/{order_code}`, `POST /api/webhooks/stripe` and the
//! `/api/admin/orders` operations (FPS confirmation, release, quarantine).
//!
//! Contracts this module exists to keep (Vision: PAY-01, STOCK-01, R2-5,
//! R2-11), in the order the code enforces them:
//!
//! 1. **Validate FIRST, before any DB work** (same contract as
//!    `admin_crud`): a bad request is a 400 with a specific
//!    `{"error": "..."}` body and must never open a connection. All field
//!    rules live in one pure function ([`validate_reservation`]).
//! 2. **Money is never taken from the client.** `unit_price_cents` comes from
//!    the `products` row the transaction just LOCKED; `total_cents` is
//!    computed server-side from it. A client-sent price is ignored.
//! 3. **Reservations are atomic** (STOCK-01). One transaction: sweep lapsed
//!    reservations, `SELECT ... FOR UPDATE` the product row, re-check
//!    `available = stock_total - stock_reserved - stock_quarantined`, bump
//!    `stock_reserved`, insert the order. The row lock is what makes
//!    concurrent checkouts on the same SKU serial instead of racing into an
//!    oversell.
//! 4. **A lapsed reservation is released, never restocked** (STOCK-01).
//!    [`expire_stale_reservations`] runs at the start of EVERY order touch
//!    (reserve, read, webhook, admin op) so "expired" is observed
//!    deterministically and the units return to the saleable pool exactly
//!    once. Only a returned unit enters `stock_quarantined` (R2-11).
//! 5. **The gateway is the payment authority** (PAY-01). A webhook counts
//!    only when its `Stripe-Signature` verifies against `STRIPE_WEBHOOK_SECRET`
//!    and its (amount, currency) match the locked order. The `payment_events`
//!    row is inserted `ON CONFLICT (event_id) DO NOTHING` INSIDE the same
//!    transaction: a replayed event changes nothing (idempotent answer), and
//!    a crash can never deduct stock twice.
//! 6. **Late payment is flagged, never fulfilled** (R2-5). A verified payment
//!    against an `expired`/`cancelled` order moves it to
//!    `late_payment_review` for a human refund decision; stock is untouched
//!    (the reservation was already released).
//!
//! Order state transitions are pure functions ([`payment_transition`],
//! [`fps_confirmation_transition`], [`release_transition`],
//! [`quarantine_transition`]) so the rules are unit-testable without a
//! database; the DB suites then pin the SQL side effects.
//!
//! JSON field names are snake_case, exactly like the catalogue/admin DTOs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Extension, Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::{FromRow, PgPool};
use tracing::{info, warn};

use crate::admin::CurrentUser;
use crate::admin_crud::insert_audit;
use crate::config;
use crate::state::{self, ApiError, AppState, ErrorBody};

type HmacSha256 = Hmac<Sha256>;

/// Bounded retries for the (astronomically rare) `order_code` UNIQUE
/// collision: each retry rolls the whole reservation transaction back and
/// regenerates the code — a Postgres transaction is aborted after a failed
/// statement, so retrying a bare INSERT inside it could not work.
const MAX_ORDER_CODE_ATTEMPTS: u32 = 3;

/// Maximum units per order (matches the reservation contract: the whole drop
/// is 16 units, so an order may never quietly become a bulk order).
pub const MAX_ORDER_QUANTITY: i32 = 16;

// ---------------------------------------------------------------------------
// Request / response shapes
// ---------------------------------------------------------------------------

/// One requested line item in a multi-SKU reservation (`CART-01`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReservationItemInput {
    pub slug: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
}

/// One resolved line item on an order (`CART-01`).
///
/// The trailing metadata fields are a live `LEFT JOIN products` enrichment,
/// populated regardless of `products.active`: a SKU deactivated after a
/// customer checked out must still render its title, photo and stage config
/// on the checkout / Collector's Pass pages. Older payloads without these
/// keys deserialize to the DB defaults via `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, PartialEq, Eq)]
pub struct OrderItem {
    pub product_id: i64,
    pub product_slug: String,
    pub quantity: i32,
    pub unit_price_cents: i64,
    pub line_total_cents: i64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub image_day_url: String,
    #[serde(default)]
    pub image_lit_url: String,
    #[serde(default = "default_order_item_art_variant")]
    pub art_variant: String,
    #[serde(default = "default_order_item_art_hue")]
    pub art_hue: i32,
    #[serde(default)]
    pub dimensions: String,
}

/// Fallback stage config when the enrichment join finds no product row
/// (matches the `products` column defaults).
fn default_order_item_art_variant() -> String {
    "dome".to_string()
}

fn default_order_item_art_hue() -> i32 {
    35
}

/// `POST /api/orders/reserve` body. Supports both single-SKU (`slug` +
/// `quantity`) and multi-SKU Shopping Cart (`items: [{ slug, quantity }]`)
/// requests (`CART-01`). `quantity`, `customer_phone` and `delivery_address`
/// may be omitted (defaults below). No price field exists here on purpose —
/// see the module docs: money comes from the DB.
///
/// `locale` remembers the customer's storefront language ("en" / "zh-HK",
/// LANG-01) on the order row so every later email speaks it; `idempotency_key`
/// is the client-side identity of ONE pay attempt ("one order per pay tap",
/// findings G01): replaying the same key returns the first order instead of a
/// second stock-holding ghost. The `Idempotency-Key` HTTP header is the
/// fallback when the body carries none.
#[derive(Debug, Deserialize)]
pub struct CreateReservationRequest {
    #[serde(default)]
    pub slug: String,
    #[serde(default = "default_quantity")]
    pub quantity: i32,
    #[serde(default)]
    pub items: Vec<ReservationItemInput>,
    pub payment_method: String,
    pub customer_name: String,
    pub customer_email: String,
    #[serde(default)]
    pub customer_phone: String,
    #[serde(default)]
    pub delivery_address: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Cart price-change detection (`CART-01`): the total the customer last
    /// SAW (cart subtotal). When present and it no longer equals the
    /// authoritative total computed from the locked product rows, the
    /// reservation is refused with a 409 instead of silently charging a
    /// different amount than the cart showed.
    #[serde(default)]
    pub expected_total_cents: Option<i64>,
}

fn default_quantity() -> i32 {
    1
}

fn default_locale() -> String {
    "en".to_string()
}

/// `POST /api/orders/reserve` response — the created reservation, including
/// all line items (`items`). `idempotent_replay` is `false` on a fresh insert
/// and `true` when this answer is a REPLAY of an earlier attempt loaded via
/// `load_order_response_by_idempotency_key` — the web tier must not dispatch
/// a second reservation email for a replay.
#[derive(Debug, Serialize)]
pub struct OrderResponse {
    pub id: i64,
    pub order_code: String,
    pub product_id: i64,
    pub product_slug: String,
    pub quantity: i32,
    pub unit_price_cents: i64,
    pub total_cents: i64,
    pub currency: String,
    pub payment_method: String,
    pub locale: String,
    pub status: String,
    pub customer_name: String,
    pub customer_email: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub items: Vec<OrderItem>,
    pub idempotent_replay: bool,
}

/// `GET /api/orders/{order_code}` response — the PUBLIC order status: no
/// customer PII, just what a buyer needs to see, including all line items.
#[derive(Debug, Serialize)]
pub struct PublicOrderStatus {
    pub order_code: String,
    pub product_slug: String,
    pub quantity: i32,
    pub unit_price_cents: i64,
    pub total_cents: i64,
    pub currency: String,
    pub payment_method: String,
    pub locale: String,
    pub status: String,
    pub fps_reference: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub items: Vec<OrderItem>,
}

#[derive(Debug, FromRow)]
struct PublicOrderStatusRow {
    id: i64,
    order_code: String,
    product_id: i64,
    product_slug: String,
    quantity: i32,
    unit_price_cents: i64,
    total_cents: i64,
    currency: String,
    payment_method: String,
    locale: String,
    status: String,
    fps_reference: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// One `orders` row as the admin console sees it (full row, incl. the
/// contact/delivery fields and `items` — admin-only data behind `require_admin`).
#[derive(Debug, Serialize)]
pub struct AdminOrder {
    pub id: i64,
    pub order_code: String,
    pub product_id: i64,
    pub product_slug: String,
    pub quantity: i32,
    pub unit_price_cents: i64,
    pub total_cents: i64,
    pub currency: String,
    pub payment_method: String,
    pub locale: String,
    pub status: String,
    pub customer_name: String,
    pub customer_email: String,
    pub customer_phone: String,
    pub delivery_address: String,
    pub fps_reference: String,
    pub paid_email_sent_at: Option<DateTime<Utc>>,
    pub email_last_error: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub items: Vec<OrderItem>,
}

#[derive(Debug, FromRow)]
struct AdminOrderRow {
    id: i64,
    order_code: String,
    product_id: i64,
    product_slug: String,
    quantity: i32,
    unit_price_cents: i64,
    total_cents: i64,
    currency: String,
    payment_method: String,
    locale: String,
    status: String,
    customer_name: String,
    customer_email: String,
    customer_phone: String,
    delivery_address: String,
    fps_reference: String,
    paid_email_sent_at: Option<DateTime<Utc>>,
    email_last_error: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// `POST /api/webhooks/stripe` normalized payload, extracted from either
/// Stripe's standard Event envelope (`{ id, type, data: { object: { ... } } }`)
/// or the direct flat representation (`{ event_id, event_type, order_code,
/// amount_cents, currency }`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StripeWebhookPayload {
    pub event_id: String,
    pub event_type: String,
    pub order_code: String,
    pub amount_cents: i64,
    pub currency: String,
}

/// Only confirmed payment-success event types may settle an order (PAY-01).
pub fn is_settled_stripe_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "payment_intent.succeeded"
            | "checkout.session.completed"
            | "checkout.session.async_payment_succeeded"
            | "charge.succeeded"
    )
}

/// Parse either a flat [`StripeWebhookPayload`] or a real Stripe Event
/// envelope (`id`, `type`, `data.object`).
fn decode_stripe_payload(raw_body: &[u8]) -> Option<StripeWebhookPayload> {
    if let Ok(flat) = serde_json::from_slice::<StripeWebhookPayload>(raw_body) {
        return Some(flat);
    }
    let v: serde_json::Value = serde_json::from_slice(raw_body).ok()?;
    let event_id = v.get("id")?.as_str()?.to_string();
    let mut event_type = v.get("type")?.as_str()?.to_string();
    let obj = v.get("data")?.get("object")?;
    if event_type == "checkout.session.completed" {
        if let Some(payment_status) = obj.get("payment_status").and_then(|s| s.as_str()) {
            if !payment_status.eq_ignore_ascii_case("paid") {
                event_type = "checkout.session.unpaid".to_string();
            }
        }
    }
    let order_code = obj
        .get("metadata")
        .and_then(|m| m.get("order_code"))
        .and_then(|c| c.as_str())
        .or_else(|| obj.get("client_reference_id").and_then(|c| c.as_str()))
        .or_else(|| obj.get("order_code").and_then(|c| c.as_str()))?
        .to_string();
    let amount_cents = obj
        .get("amount_total")
        .and_then(|a| a.as_i64())
        .or_else(|| obj.get("amount_received").and_then(|a| a.as_i64()))
        .or_else(|| obj.get("amount").and_then(|a| a.as_i64()))?;
    let currency = obj.get("currency")?.as_str()?.to_string();
    Some(StripeWebhookPayload {
        event_id,
        event_type,
        order_code,
        amount_cents,
        currency,
    })
}

/// `POST /api/webhooks/stripe` response. `idempotent: true` means the event
/// was already processed (PAY-01 replay) and NOTHING changed this time.
/// The order metadata (`order_code`, `customer_*`, `locale`,
/// `paid_email_sent`) tells the web tier whether the paid-confirmation email
/// still needs to go out — `paid_email_sent` is `paid_email_sent_at IS NOT
/// NULL` on the order row (the durable "customer was told" ledger).
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub ok: bool,
    pub idempotent: bool,
    pub status: String,
    pub order_code: String,
    pub customer_name: String,
    pub customer_email: String,
    pub locale: String,
    pub paid_email_sent: bool,
}

/// `POST /api/orders/{order_code}/email-delivery` body: the web tier reports
/// the OUTCOME of the paid-confirmation email attempt (`sent: true` = the
/// mail gateway accepted it; `sent: false` + `error` = the failure reason),
/// so duplicate sends can be avoided and failures stay visible to the admin
/// "re-send receipt" surface (findings G02).
///
/// `claim: true` turns the call into an ATOMIC paid-email claim instead: it
/// only reports success when this caller won the right to send the paid
/// email (`claimed: true`), so concurrent settlement paths can never
/// double-send it. The claim is a RECOVERABLE lease on the send
/// (`paid_email_claimed_at`), never proof of delivery — `paid_email_sent_at`
/// is only stamped when the actual dispatch succeeds.
///
/// `kind` names WHICH email this outcome belongs to: `"reservation"` (the
/// hold confirmation) reports must never touch the PAID-email ledger beyond
/// clearing `email_last_error`, while anything else (default: `"paid"`) is
/// the paid receipt.
#[derive(Debug, Deserialize)]
pub struct EmailDeliveryRequest {
    pub sent: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub claim: bool,
    #[serde(default)]
    pub kind: Option<String>,
}

/// `POST /api/orders/{order_code}/email-delivery` response: `paid_email_sent`
/// is the order's post-update `paid_email_sent_at IS NOT NULL` state (the
/// durable "the customer WAS told" ledger — a claim alone never sets it);
/// `claimed: true` means THIS call atomically claimed the one paid-email send
/// (`claim: true` request against a paid/fulfilled order whose paid email was
/// not claimed/sent yet).
#[derive(Debug, Serialize)]
pub struct EmailDeliveryResponse {
    pub ok: bool,
    pub paid_email_sent: bool,
    pub claimed: bool,
}

/// `POST /api/admin/orders/{id}/confirm-fps` body.
#[derive(Debug, Deserialize)]
pub struct ConfirmFpsRequest {
    pub fps_reference: String,
}

// ---------------------------------------------------------------------------
// Error bodies (the exact shapes the rest of the API uses)
// ---------------------------------------------------------------------------

fn error_body(status: StatusCode, error: &str) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: error.to_string(),
        }),
    )
}

fn bad_request(msg: &str) -> ApiError {
    error_body(StatusCode::BAD_REQUEST, msg)
}

fn not_found(msg: &str) -> ApiError {
    error_body(StatusCode::NOT_FOUND, msg)
}

fn conflict(msg: &str) -> ApiError {
    error_body(StatusCode::CONFLICT, msg)
}

/// The acting admin, read from request extensions (the `require_admin`
/// middleware always inserts it): a missing user is a 401, never a value
/// taken from the request body.
fn actor(user: Option<Extension<CurrentUser>>) -> Result<String, ApiError> {
    user.map(|Extension(user)| user.username)
        .ok_or_else(|| error_body(StatusCode::UNAUTHORIZED, "not authenticated"))
}

/// Unwraps the `Json` extractor rejection (malformed JSON, bad field types)
/// into the standard 400 body.
fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|_| bad_request("invalid JSON body"))
}

// ---------------------------------------------------------------------------
// Validation (pure: no clock, no env, no DB)
// ---------------------------------------------------------------------------

/// Normalizes [`CreateReservationRequest`] into a non-empty list of
/// `(slug, quantity)` line items, merging any duplicate slugs while
/// preserving first-seen order (`CART-01`).
pub fn normalized_reservation_items(
    req: &CreateReservationRequest,
) -> Result<Vec<ReservationItemInput>, String> {
    let raw_items: Vec<ReservationItemInput> = if !req.items.is_empty() {
        req.items.clone()
    } else {
        vec![ReservationItemInput {
            slug: req.slug.clone(),
            quantity: req.quantity,
        }]
    };

    let mut merged: Vec<ReservationItemInput> = Vec::with_capacity(raw_items.len());
    for item in raw_items {
        let trimmed = item.slug.trim();
        if trimmed.is_empty() {
            return Err("slug is required".to_string());
        }
        reject_nul("slug", &item.slug)?;
        if !(1..=MAX_ORDER_QUANTITY).contains(&item.quantity) {
            return Err(format!(
                "quantity must be between 1 and {MAX_ORDER_QUANTITY}"
            ));
        }
        if let Some(existing) = merged.iter_mut().find(|i| i.slug == trimmed) {
            let sum = existing
                .quantity
                .checked_add(item.quantity)
                .ok_or_else(|| format!("quantity must be between 1 and {MAX_ORDER_QUANTITY}"))?;
            if !(1..=MAX_ORDER_QUANTITY).contains(&sum) {
                return Err(format!(
                    "quantity must be between 1 and {MAX_ORDER_QUANTITY}"
                ));
            }
            existing.quantity = sum;
        } else {
            merged.push(ReservationItemInput {
                slug: trimmed.to_string(),
                quantity: item.quantity,
            });
        }
    }
    if merged.is_empty() {
        return Err("slug is required".to_string());
    }
    Ok(merged)
}

/// Field rules for [`CreateReservationRequest`]. Every text field is NUL-free
/// (Postgres TEXT cannot hold a NUL; a 503 from the DB would be the wrong
/// answer to a client mistake) and the required ones are non-empty.
pub fn validate_reservation(req: &CreateReservationRequest) -> Result<(), String> {
    normalized_reservation_items(req)?;
    if !matches!(req.payment_method.as_str(), "card" | "fps") {
        return Err("payment_method must be \"card\" or \"fps\"".to_string());
    }
    if !matches!(req.locale.as_str(), "en" | "zh-HK") {
        return Err("locale must be \"en\" or \"zh-HK\"".to_string());
    }
    if req.customer_name.trim().is_empty() {
        return Err("customer_name is required".to_string());
    }
    reject_nul("customer_name", &req.customer_name)?;
    // Shape check only (an "@" somewhere): deliverability is the mail
    // gateway's question, not this endpoint's.
    if !req.customer_email.contains('@') {
        return Err("customer_email must contain @".to_string());
    }
    reject_nul("customer_email", &req.customer_email)?;
    reject_nul("customer_phone", &req.customer_phone)?;
    reject_nul("delivery_address", &req.delivery_address)?;
    // An idempotency key is OPTIONAL (older clients send none), but when
    // present and non-blank it must be a usable ledger key: NUL-free (it is
    // stored in TEXT) and bounded so one client cannot fill the unique index
    // with megabyte keys. Blank/whitespace keys are treated as ABSENT by the
    // normalizer — validation mirrors that (findings G01).
    if let Some(key) = req
        .idempotency_key
        .as_deref()
        .filter(|k| !k.trim().is_empty())
    {
        reject_nul("idempotency_key", key)?;
        if key.trim().len() > 128 {
            return Err("idempotency_key must be at most 128 characters".to_string());
        }
    }
    Ok(())
}

fn reject_nul(field: &str, value: &str) -> Result<(), String> {
    if value.contains('\0') {
        Err(format!("{field} contains a NUL byte"))
    } else {
        Ok(())
    }
}

/// `total_cents = unit_price_cents * quantity` with the overflow held out:
/// quantity is capped at 16 and prices are BIGINT cents, so this can only
/// fire on corrupt data — and a wrapped total would silently sell at the
/// wrong price, so it errors instead.
pub fn order_total(unit_price_cents: i64, quantity: i32) -> Option<i64> {
    unit_price_cents.checked_mul(i64::from(quantity))
}

// ---------------------------------------------------------------------------
// Order codes
// ---------------------------------------------------------------------------

/// Process-wide order-code sequence, seeded ONCE from the wall clock in
/// microseconds and then strictly increasing: two processes that share a
/// millisecond still draw disjoint suffixes, and the `orders.order_code`
/// UNIQUE constraint plus [`MAX_ORDER_CODE_ATTEMPTS`] is the hard backstop.
static ORDER_CODE_SEQ: OnceLock<AtomicU64> = OnceLock::new();

fn next_order_seq() -> u64 {
    let seq = ORDER_CODE_SEQ.get_or_init(|| {
        let seed = Utc::now().timestamp_micros().max(0) as u64 | 1;
        AtomicU64::new(seed)
    });
    seq.fetch_add(1, Ordering::Relaxed)
}

/// One `ORD-<millis>-<seq>` code: sortable-ish, unique per call within the
/// process, and never derived from client input.
pub fn next_order_code() -> String {
    format!("ORD-{}-{}", Utc::now().timestamp_millis(), next_order_seq())
}

/// `orders_order_code_key` is the constraint Postgres generates for the
/// `order_code TEXT UNIQUE` column (0006). Anything else is a real failure.
fn is_duplicate_order_code(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.constraint() == Some("orders_order_code_key"))
}

/// `orders_idempotency_key_key` is the constraint Postgres generates for the
/// `idempotency_key TEXT UNIQUE` column (0008): two concurrent attempts of
/// ONE pay tap both passed the pre-check, and the loser of the INSERT race
/// must return the winner's order instead of failing (G01).
fn is_duplicate_idempotency_key(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.constraint() == Some("orders_idempotency_key_key"))
}

// ---------------------------------------------------------------------------
// State-transition rules (pure: the SQL side effects stay in the handlers)
// ---------------------------------------------------------------------------

/// What a VERIFIED payment event does to an order (PAY-01 + R2-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentTransition {
    /// `reserved -> paid`: the reserved units are consumed from stock.
    MarkPaid,
    /// `expired`/`cancelled -> late_payment_review`: flag for a human refund
    /// decision — the units were already released, so nothing is fulfilled.
    LatePaymentReview,
    /// Already settled (`paid`, `fulfilled`, `late_payment_review`,
    /// `returned_quarantined`): record the event, change nothing.
    NoChange,
}

pub fn payment_transition(status: &str) -> PaymentTransition {
    match status {
        "reserved" => PaymentTransition::MarkPaid,
        "expired" | "cancelled" => PaymentTransition::LatePaymentReview,
        _ => PaymentTransition::NoChange,
    }
}

/// What an admin FPS confirmation does to an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpsTransition {
    /// `reserved -> paid`, consuming the reserved units.
    MarkPaid,
    /// `expired -> late_payment_review` (R2-5: a manual transfer that landed
    /// after the reservation lapsed is reviewed/refunded, never fulfilled).
    LatePaymentReview,
    /// Every other state: nothing to confirm, no state change.
    Reject,
}

pub fn fps_confirmation_transition(status: &str) -> FpsTransition {
    match status {
        "reserved" => FpsTransition::MarkPaid,
        "expired" => FpsTransition::LatePaymentReview,
        _ => FpsTransition::Reject,
    }
}

/// What an admin "release" does to an order: only an unpaid `reserved` order
/// can be cancelled and have its units returned to the saleable pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseTransition {
    Cancel,
    Reject,
}

pub fn release_transition(status: &str) -> ReleaseTransition {
    match status {
        "reserved" => ReleaseTransition::Cancel,
        _ => ReleaseTransition::Reject,
    }
}

/// What a returned unit does to stock (STOCK-01 / R2-11): only a sold order
/// (`paid`/`fulfilled`) can be returned, and the units go into
/// `stock_quarantined` — they are back in the building, never back on sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnTransition {
    Quarantine,
    Reject,
}

pub fn quarantine_transition(status: &str) -> ReturnTransition {
    match status {
        "paid" | "fulfilled" => ReturnTransition::Quarantine,
        _ => ReturnTransition::Reject,
    }
}

// ---------------------------------------------------------------------------
// Stripe signature (pure)
// ---------------------------------------------------------------------------

/// Pure hex decode: even length, hex digits only (either case).
fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        out.push((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?);
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Verifies a `Stripe-Signature` header: `t=<unix-seconds>,v1=<hex>` where
/// each `v1` is `HMAC-SHA256(secret, "{t}.{raw_body}")` in hex. The body is
/// the EXACT received bytes (that is why the handler takes `Bytes`, not a
/// parsed value). The timestamp is covered by the signature, so tampering
/// with it invalidates the header; the comparison itself is constant-time
/// (`hmac::Mac::verify_slice`). Any failure returns a reason (logged by the
/// handler) — the HTTP layer maps every reason to the same 401.
pub fn verify_stripe_signature(
    secret: &[u8],
    signature_header: &str,
    raw_body: &[u8],
) -> Result<(), &'static str> {
    let mut timestamp: Option<&str> = None;
    let mut candidates: Vec<&str> = Vec::new();
    for part in signature_header.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            return Err("signature header is not t=...,v1=...");
        };
        match key.trim() {
            "t" if timestamp.is_none() && !value.is_empty() => timestamp = Some(value),
            "v1" if !value.is_empty() => candidates.push(value),
            // Unknown scheme (v0, ...) or a duplicate/empty t: ignored, like
            // Stripe's own verifier — only a matching v1 can decide.
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or("signature header carries no t= timestamp")?;
    if candidates.is_empty() {
        return Err("signature header carries no v1= signature");
    }

    let mut signed = Vec::with_capacity(timestamp.len() + 1 + raw_body.len());
    signed.extend_from_slice(timestamp.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(raw_body);

    for candidate in candidates {
        let Some(expected) = decode_hex(candidate) else {
            continue;
        };
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC-SHA256 accepts keys of any length");
        mac.update(&signed);
        if mac.verify_slice(&expected).is_ok() {
            return Ok(());
        }
    }
    Err("signature does not match the signed payload")
}

/// Builds a `Stripe-Signature` header for `raw_body`. Used by the test
/// suites (and any local webhook simulator) so they sign exactly the way the
/// verifier expects — a hand-rolled test signature would only test itself.
pub fn stripe_signature_header(secret: &[u8], timestamp: i64, raw_body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    format!(
        "t={timestamp},v1={}",
        encode_hex(&mac.finalize().into_bytes())
    )
}

/// Pure hex encode (lowercase), the inverse of [`decode_hex`].
fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The webhook's pre-DB half: configured secret -> signature -> payload.
/// Pure over its arguments (no env, no clock, no pool) so every rejection
/// path is unit-testable without a database or a process environment.
fn parse_signed_webhook(
    secret: Option<&[u8]>,
    headers: &HeaderMap,
    raw_body: &[u8],
) -> Result<StripeWebhookPayload, ApiError> {
    let Some(secret) = secret else {
        return Err(error_body(
            StatusCode::SERVICE_UNAVAILABLE,
            "stripe webhook not configured",
        ));
    };
    let Some(signature) = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(error_body(
            StatusCode::UNAUTHORIZED,
            "invalid stripe signature",
        ));
    };
    if let Err(reason) = verify_stripe_signature(secret, signature, raw_body) {
        warn!(reason, "stripe webhook signature rejected");
        return Err(error_body(
            StatusCode::UNAUTHORIZED,
            "invalid stripe signature",
        ));
    }
    let payload =
        decode_stripe_payload(raw_body).ok_or_else(|| bad_request("invalid stripe payload"))?;
    if payload.event_id.trim().is_empty() || payload.order_code.trim().is_empty() {
        return Err(bad_request("invalid stripe payload"));
    }
    if !is_settled_stripe_event_type(payload.event_type.trim()) {
        return Err(bad_request("unsupported or non-success stripe event type"));
    }
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Shared DB helpers (all inside the caller's transaction)
// ---------------------------------------------------------------------------

/// Loads the line items for one order (`order_items` table, falling back to
/// the single-item columns on `orders` if `order_items` is empty).
///
/// Every item is enriched with the LIVE product metadata through a
/// `LEFT JOIN products` that deliberately does NOT filter on `active`: an
/// admin deactivating a SKU must not blank the title/photo/badge a customer
/// is looking at on the checkout or Collector's Pass page. Missing rows
/// (the `product_id = 0` fallback) degrade to the column defaults.
async fn load_order_items_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_id: i64,
    fallback_product_id: i64,
    fallback_slug: &str,
    fallback_qty: i32,
    fallback_unit_cents: i64,
    fallback_total_cents: i64,
) -> Result<Vec<OrderItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, OrderItem>(
        "SELECT oi.product_id, oi.product_slug, oi.quantity, oi.unit_price_cents, \
             oi.line_total_cents, \
             COALESCE(p.model, '') AS model, \
             COALESCE(p.brand, '') AS brand, \
             COALESCE(p.image_day_url, '') AS image_day_url, \
             COALESCE(p.image_lit_url, '') AS image_lit_url, \
             COALESCE(p.art_variant, 'dome') AS art_variant, \
             COALESCE(p.art_hue, 35) AS art_hue, \
             COALESCE(p.dimensions, '') AS dimensions \
         FROM order_items oi \
         LEFT JOIN products p ON p.id = oi.product_id \
         WHERE oi.order_id = $1 ORDER BY oi.product_slug ASC, oi.id ASC",
    )
    .bind(order_id)
    .fetch_all(&mut **tx)
    .await?;

    if !rows.is_empty() {
        return Ok(rows);
    }

    // Pre-0007 single-item orders have no `order_items` row: rebuild the item
    // from the `orders` columns and enrich it from products the same way.
    let enrichment = sqlx::query_as::<_, (String, String, String, String, String, i32, String)>(
        "SELECT COALESCE(model, ''), COALESCE(brand, ''), COALESCE(image_day_url, ''), \
             COALESCE(image_lit_url, ''), COALESCE(art_variant, 'dome'), \
             COALESCE(art_hue, 35), COALESCE(dimensions, '') \
         FROM products WHERE id = $1",
    )
    .bind(fallback_product_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (model, brand, image_day_url, image_lit_url, art_variant, art_hue, dimensions) = enrichment
        .unwrap_or_else(|| {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                default_order_item_art_variant(),
                default_order_item_art_hue(),
                String::new(),
            )
        });

    Ok(vec![OrderItem {
        product_id: fallback_product_id,
        product_slug: fallback_slug.to_string(),
        quantity: fallback_qty,
        unit_price_cents: fallback_unit_cents,
        line_total_cents: fallback_total_cents,
        model,
        brand,
        image_day_url,
        image_lit_url,
        art_variant,
        art_hue,
        dimensions,
    }])
}

/// Sweeps lapsed reservations (STOCK-01): every `reserved` order past its
/// `expires_at` is locked, moved to `expired`, and its units are RETURNED to
/// the saleable pool (`stock_reserved` drops — a release is not a restock;
/// only [`quarantine_return_order`] puts units back in the building).
///
/// Runs at the top of every order-touching transaction so "is this
/// reservation still live?" is answered against the clock, not against
/// whatever a stale row claims. `GREATEST(0, ...)` keeps the 0001 CHECK
/// satisfiable even if an admin edit ever left `stock_reserved` below the
/// reserved quantity.
pub async fn expire_stale_reservations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<usize, sqlx::Error> {
    let expired: Vec<(i64, i64, String, i32, i64, i64)> = sqlx::query_as(
        "SELECT id, product_id, product_slug, quantity, unit_price_cents, total_cents \
         FROM orders WHERE status = 'reserved' AND expires_at <= now() FOR UPDATE",
    )
    .fetch_all(&mut **tx)
    .await?;
    for (order_id, product_id, product_slug, quantity, unit_price_cents, total_cents) in &expired {
        let items = load_order_items_tx(
            tx,
            *order_id,
            *product_id,
            product_slug,
            *quantity,
            *unit_price_cents,
            *total_cents,
        )
        .await?;
        for item in &items {
            sqlx::query(
                "UPDATE products SET stock_reserved = GREATEST(0, stock_reserved - $1) WHERE id = $2",
            )
            .bind(item.quantity)
            .bind(item.product_id)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query("UPDATE orders SET status = 'expired', updated_at = now() WHERE id = $1")
            .bind(*order_id)
            .execute(&mut **tx)
            .await?;
    }
    if !expired.is_empty() {
        info!(count = expired.len(), "expired stale reservations");
    }
    Ok(expired.len())
}

/// Runs [`expire_stale_reservations`] in its own committed transaction so
/// live reads (`GET /api/products`, `GET /api/admin/orders`) and mutations
/// that later return a business 409 (`release_order`, `create_reservation`)
/// still persist the expiration of any lapsed holds.
pub async fn sweep_stale_reservations(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let count = expire_stale_reservations(&mut tx).await?;
    tx.commit().await?;
    Ok(count)
}

/// The columns every order-state decision needs, read with `FOR UPDATE` so
/// concurrent state transitions on one order serialise on the row lock.
#[derive(Debug, FromRow)]
struct LockedOrder {
    id: i64,
    order_code: String,
    product_id: i64,
    product_slug: String,
    quantity: i32,
    unit_price_cents: i64,
    payment_method: String,
    status: String,
    total_cents: i64,
    currency: String,
    customer_name: String,
    customer_email: String,
    locale: String,
    paid_email_sent_at: Option<DateTime<Utc>>,
}

async fn lock_order_by_code(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_code: &str,
) -> Result<Option<LockedOrder>, sqlx::Error> {
    sqlx::query_as::<_, LockedOrder>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, \
             payment_method, status, total_cents, currency, customer_name, \
             customer_email, locale, paid_email_sent_at \
         FROM orders WHERE order_code = $1 FOR UPDATE",
    )
    .bind(order_code)
    .fetch_optional(&mut **tx)
    .await
}

async fn lock_order_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
) -> Result<Option<LockedOrder>, sqlx::Error> {
    sqlx::query_as::<_, LockedOrder>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, \
             payment_method, status, total_cents, currency, customer_name, \
             customer_email, locale, paid_email_sent_at \
         FROM orders WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
}

async fn load_public_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_code: &str,
) -> Result<Option<PublicOrderStatus>, sqlx::Error> {
    let row = sqlx::query_as::<_, PublicOrderStatusRow>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, total_cents, currency, \
             payment_method, locale, status, fps_reference, expires_at, created_at \
         FROM orders WHERE order_code = $1",
    )
    .bind(order_code)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let items = load_order_items_tx(
        tx,
        row.id,
        row.product_id,
        &row.product_slug,
        row.quantity,
        row.unit_price_cents,
        row.total_cents,
    )
    .await?;

    Ok(Some(PublicOrderStatus {
        order_code: row.order_code,
        product_slug: row.product_slug,
        quantity: row.quantity,
        unit_price_cents: row.unit_price_cents,
        total_cents: row.total_cents,
        currency: row.currency,
        payment_method: row.payment_method,
        locale: row.locale,
        status: row.status,
        fps_reference: row.fps_reference,
        expires_at: row.expires_at,
        created_at: row.created_at,
        items,
    }))
}

/// Re-reads the full admin row after a mutation (inside the same transaction:
/// the response must reflect exactly what commits).
async fn load_admin_order(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
) -> Result<Option<AdminOrder>, sqlx::Error> {
    let row = sqlx::query_as::<_, AdminOrderRow>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, \
             total_cents, currency, payment_method, locale, status, customer_name, \
             customer_email, customer_phone, delivery_address, fps_reference, \
             paid_email_sent_at, email_last_error, expires_at, created_at, updated_at \
         FROM orders WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let items = load_order_items_tx(
        tx,
        row.id,
        row.product_id,
        &row.product_slug,
        row.quantity,
        row.unit_price_cents,
        row.total_cents,
    )
    .await?;

    Ok(Some(AdminOrder {
        id: row.id,
        order_code: row.order_code,
        product_id: row.product_id,
        product_slug: row.product_slug,
        quantity: row.quantity,
        unit_price_cents: row.unit_price_cents,
        total_cents: row.total_cents,
        currency: row.currency,
        payment_method: row.payment_method,
        locale: row.locale,
        status: row.status,
        customer_name: row.customer_name,
        customer_email: row.customer_email,
        customer_phone: row.customer_phone,
        delivery_address: row.delivery_address,
        fps_reference: row.fps_reference,
        paid_email_sent_at: row.paid_email_sent_at,
        email_last_error: row.email_last_error,
        expires_at: row.expires_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        items,
    }))
}

/// The `orders` columns one reservation replay needs (findings G01: same pay
/// attempt -> same order).
#[derive(Debug, FromRow)]
struct ReservationReplayRow {
    id: i64,
    order_code: String,
    product_id: i64,
    product_slug: String,
    quantity: i32,
    unit_price_cents: i64,
    total_cents: i64,
    currency: String,
    payment_method: String,
    locale: String,
    status: String,
    customer_name: String,
    customer_email: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// Loads the order a PREVIOUS reservation with this idempotency key created,
/// with its line items, so a retried pay attempt answers with the original
/// order and never reserves stock twice. Read-only: it runs OUTSIDE the
/// reservation transaction on purpose — the caller may be a loser of the
/// `orders_idempotency_key_key` race whose own transaction is aborted.
async fn load_order_response_by_idempotency_key(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<Option<OrderResponse>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, ReservationReplayRow>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, \
             total_cents, currency, payment_method, locale, status, customer_name, \
             customer_email, expires_at, created_at \
         FROM orders WHERE idempotency_key = $1",
    )
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let items = load_order_items_tx(
        &mut tx,
        row.id,
        row.product_id,
        &row.product_slug,
        row.quantity,
        row.unit_price_cents,
        row.total_cents,
    )
    .await?;
    Ok(Some(OrderResponse {
        id: row.id,
        order_code: row.order_code,
        product_id: row.product_id,
        product_slug: row.product_slug,
        quantity: row.quantity,
        unit_price_cents: row.unit_price_cents,
        total_cents: row.total_cents,
        currency: row.currency,
        payment_method: row.payment_method,
        locale: row.locale,
        status: row.status,
        customer_name: row.customer_name,
        customer_email: row.customer_email,
        expires_at: row.expires_at,
        created_at: row.created_at,
        items,
        // A replayed pay attempt — the web tier must not send a second
        // reservation email for an order the customer already knows about.
        idempotent_replay: true,
    }))
}

/// The currency new orders are quoted in: the `settings` override when
/// present and well-formed (CONFIG-01: override -> env -> code default),
/// otherwise [`config::default_currency`].
async fn order_currency(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<String, sqlx::Error> {
    let override_value: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'currency'")
            .fetch_optional(&mut **tx)
            .await?;
    Ok(match override_value {
        Some(value) => match config::parse_currency(&value) {
            Some(code) => code,
            None => {
                warn!(value = %value, "settings currency is not a 3-letter ISO 4217 code; using the default");
                config::default_currency()
            }
        },
        None => config::default_currency(),
    })
}

// ---------------------------------------------------------------------------
// Handlers: public money path
// ---------------------------------------------------------------------------

/// `POST /api/orders/reserve` — atomically hold `quantity` units of one SKU
/// (STOCK-01). Returns the created reservation with the AUTHORITATIVE price
/// taken from the locked product row.
pub async fn create_reservation(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<CreateReservationRequest>, JsonRejection>,
) -> Result<Json<OrderResponse>, ApiError> {
    let mut req = json_body(body)?;
    // `Idempotency-Key` header fallback (findings G01): when the body carries
    // no key (or only a blank one), the standard header names the pay attempt
    // instead. The body field wins when both are present; blank keys on both
    // sides mean "no idempotency" exactly like before.
    if req
        .idempotency_key
        .as_deref()
        .is_none_or(|key| key.trim().is_empty())
    {
        if let Some(key) = headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .filter(|key| !key.trim().is_empty())
        {
            req.idempotency_key = Some(key.to_string());
        }
    }
    // Validation before ANY database work (see the module docs).
    if let Err(msg) = validate_reservation(&req) {
        return Err(bad_request(&msg));
    }
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    for _ in 0..MAX_ORDER_CODE_ATTEMPTS {
        match reserve_once(pool, &req).await {
            Ok(Ok(order)) => return Ok(Json(order)),
            Ok(Err(ReserveRejection::ProductNotFound)) => {
                return Err(not_found("product not found"))
            }
            Ok(Err(ReserveRejection::InsufficientStock)) => {
                return Err(conflict("insufficient stock available"));
            }
            Ok(Err(ReserveRejection::PriceChanged {
                current_total_cents,
            })) => {
                warn!(
                    current_total_cents,
                    "cart total changed since it was added; refusing the reservation"
                );
                return Err(conflict(
                    "price updated since items were added to cart — please review updated total",
                ));
            }
            Err(ReserveFailure::DuplicateOrderCode) => continue,
            Err(ReserveFailure::MoneyOverflow) => {
                return Err(error_body(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "order total overflows the money range",
                ));
            }
            Err(ReserveFailure::Db(e)) => {
                return Err(state::database_unavailable("failed to create order", &e));
            }
        }
    }
    warn!("could not allocate a unique order code; refusing the reservation");
    Err(error_body(
        StatusCode::SERVICE_UNAVAILABLE,
        "could not allocate a unique order code",
    ))
}

/// Business-level refusal of one reservation attempt (mapped to 404 / 409 by
/// the handler); infrastructure failures stay in [`ReserveFailure`].
enum ReserveRejection {
    ProductNotFound,
    InsufficientStock,
    /// The cart's authoritative total no longer matches `expected_total_cents`
    /// (an admin price edit landed while the customer was in checkout): the
    /// handler refuses with a 409 and the current total.
    PriceChanged {
        current_total_cents: i64,
    },
}

enum ReserveFailure {
    DuplicateOrderCode,
    MoneyOverflow,
    Db(sqlx::Error),
}

/// One full reservation transaction. Split out from [`create_reservation`] so
/// the `order_code` UNIQUE retry can roll it back and start clean.
async fn reserve_once(
    pool: &PgPool,
    req: &CreateReservationRequest,
) -> Result<Result<OrderResponse, ReserveRejection>, ReserveFailure> {
    let requested_items =
        normalized_reservation_items(req).map_err(|_| ReserveFailure::MoneyOverflow)?;
    // One order per pay tap (findings G01): the key is normalised to its
    // trimmed non-empty form (" key " and "key" are the same attempt), and a
    // replay of a KNOWN key returns the first order before anything is
    // locked — a retry must never reserve stock a second time.
    let idempotency_key = req
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if let Some(key) = idempotency_key {
        if let Some(existing) = load_order_response_by_idempotency_key(pool, key)
            .await
            .map_err(ReserveFailure::Db)?
        {
            return Ok(Ok(existing));
        }
    }
    let mut tx = pool.begin().await.map_err(ReserveFailure::Db)?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(ReserveFailure::Db)?;

    // Lock product rows in ascending slug order to prevent deadlocks when two
    // multi-SKU carts check out concurrently.
    let mut slugs_sorted: Vec<String> = requested_items.iter().map(|i| i.slug.clone()).collect();
    slugs_sorted.sort();
    slugs_sorted.dedup();

    let mut locked_map = std::collections::HashMap::<String, LockedProduct>::new();
    for slug in &slugs_sorted {
        let product: Option<LockedProduct> = sqlx::query_as(
            "SELECT id, slug, price_cents, active, stock_total, stock_reserved, stock_quarantined, \
                 model, brand, image_day_url, image_lit_url, art_variant, art_hue, dimensions \
             FROM products WHERE slug = $1 FOR UPDATE",
        )
        .bind(slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ReserveFailure::Db)?;

        let Some(product) = product else {
            return Ok(Err(ReserveRejection::ProductNotFound));
        };
        if !product.active {
            return Ok(Err(ReserveRejection::ProductNotFound));
        }
        locked_map.insert(slug.clone(), product);
    }

    let mut resolved_items: Vec<OrderItem> = Vec::with_capacity(requested_items.len());
    let mut total_cents: i64 = 0;
    let mut total_quantity: i32 = 0;

    for req_item in &requested_items {
        let Some(product) = locked_map.get(&req_item.slug) else {
            return Ok(Err(ReserveRejection::ProductNotFound));
        };
        let available = i64::from(product.stock_total)
            - i64::from(product.stock_reserved)
            - i64::from(product.stock_quarantined);
        if available < i64::from(req_item.quantity) {
            // Concurrent same-key retry on the LAST unit in stock: the other
            // attempt of this very pay tap may have won the race between our
            // early replay check and this availability check — look for the
            // order this key created before refusing. The replay read runs
            // OUTSIDE the transaction (it is about to return without commit).
            if let Some(key) = idempotency_key {
                drop(tx);
                if let Some(existing) = load_order_response_by_idempotency_key(pool, key)
                    .await
                    .map_err(ReserveFailure::Db)?
                {
                    return Ok(Ok(existing));
                }
            }
            return Ok(Err(ReserveRejection::InsufficientStock));
        }
        let line_total_cents = order_total(product.price_cents, req_item.quantity)
            .ok_or(ReserveFailure::MoneyOverflow)?;
        total_cents = total_cents
            .checked_add(line_total_cents)
            .ok_or(ReserveFailure::MoneyOverflow)?;
        total_quantity = total_quantity
            .checked_add(req_item.quantity)
            .ok_or(ReserveFailure::MoneyOverflow)?;
        resolved_items.push(OrderItem {
            product_id: product.id,
            product_slug: product.slug.clone(),
            quantity: req_item.quantity,
            unit_price_cents: product.price_cents,
            line_total_cents,
            model: product.model.clone(),
            brand: product.brand.clone(),
            image_day_url: product.image_day_url.clone(),
            image_lit_url: product.image_lit_url.clone(),
            art_variant: product.art_variant.clone(),
            art_hue: product.art_hue,
            dimensions: product.dimensions.clone(),
        });
    }

    if let Some(expected) = req.expected_total_cents {
        if expected != total_cents {
            return Ok(Err(ReserveRejection::PriceChanged {
                current_total_cents: total_cents,
            }));
        }
    }

    let primary = &resolved_items[0];
    let currency = order_currency(&mut tx).await.map_err(ReserveFailure::Db)?;

    let expires_at = match req.payment_method.as_str() {
        "card" => Utc::now() + Duration::minutes(config::card_reservation_ttl_mins()),
        _ => Utc::now() + Duration::hours(config::fps_reservation_ttl_hours()),
    };

    for item in &resolved_items {
        sqlx::query("UPDATE products SET stock_reserved = stock_reserved + $1 WHERE id = $2")
            .bind(item.quantity)
            .bind(item.product_id)
            .execute(&mut *tx)
            .await
            .map_err(ReserveFailure::Db)?;
    }

    let order_code = next_order_code();
    let inserted: Result<(i64, DateTime<Utc>), sqlx::Error> = sqlx::query_as(
        "INSERT INTO orders (order_code, product_id, product_slug, quantity, unit_price_cents, \
             total_cents, currency, payment_method, status, customer_name, customer_email, \
             customer_phone, delivery_address, expires_at, locale, idempotency_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'reserved', $9, $10, $11, $12, $13, $14, $15) \
         RETURNING id, created_at",
    )
    .bind(&order_code)
    .bind(primary.product_id)
    .bind(&primary.product_slug)
    .bind(total_quantity)
    .bind(primary.unit_price_cents)
    .bind(total_cents)
    .bind(&currency)
    .bind(&req.payment_method)
    .bind(&req.customer_name)
    .bind(&req.customer_email)
    .bind(&req.customer_phone)
    .bind(&req.delivery_address)
    .bind(expires_at)
    .bind(&req.locale)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await;
    let (id, created_at) = match inserted {
        Ok(row) => row,
        Err(e) if is_duplicate_order_code(&e) => return Err(ReserveFailure::DuplicateOrderCode),
        // Two concurrent attempts of ONE pay tap: the `idempotency_key`
        // UNIQUE index picks the winner; the loser's transaction is aborted
        // (its reserved units roll back with it) and it returns the WINNER's
        // order — exactly one order and one stock hold per key (G01).
        Err(e) if is_duplicate_idempotency_key(&e) => {
            drop(tx);
            let existing = match idempotency_key {
                Some(key) => load_order_response_by_idempotency_key(pool, key)
                    .await
                    .map_err(ReserveFailure::Db)?,
                None => None,
            };
            return match existing {
                Some(order) => Ok(Ok(order)),
                // The winning order vanished between the conflict and this
                // read: surface the original failure, never invent a result.
                None => Err(ReserveFailure::Db(e)),
            };
        }
        Err(e) => return Err(ReserveFailure::Db(e)),
    };

    for item in &resolved_items {
        sqlx::query(
            "INSERT INTO order_items (order_id, product_id, product_slug, quantity, \
                 unit_price_cents, line_total_cents, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(item.product_id)
        .bind(&item.product_slug)
        .bind(item.quantity)
        .bind(item.unit_price_cents)
        .bind(item.line_total_cents)
        .bind(created_at)
        .execute(&mut *tx)
        .await
        .map_err(ReserveFailure::Db)?;
    }

    tx.commit().await.map_err(ReserveFailure::Db)?;
    Ok(Ok(OrderResponse {
        id,
        order_code,
        product_id: primary.product_id,
        product_slug: primary.product_slug.clone(),
        quantity: total_quantity,
        unit_price_cents: primary.unit_price_cents,
        total_cents,
        currency,
        payment_method: req.payment_method.clone(),
        locale: req.locale.clone(),
        status: "reserved".to_string(),
        customer_name: req.customer_name.clone(),
        customer_email: req.customer_email.clone(),
        expires_at,
        created_at,
        items: resolved_items,
        idempotent_replay: false,
    }))
}

#[derive(Debug, FromRow)]
struct LockedProduct {
    id: i64,
    slug: String,
    price_cents: i64,
    active: bool,
    stock_total: i32,
    stock_reserved: i32,
    stock_quarantined: i32,
    model: String,
    brand: String,
    image_day_url: String,
    image_lit_url: String,
    art_variant: String,
    art_hue: i32,
    dimensions: String,
}

/// `GET /api/orders/{order_code}` — public order status, never cacheable.
/// Also the read that lets a buyer see their reservation lapse: the expiry
/// sweep runs first, so the answer is against the clock.
pub async fn get_order_by_code(
    State(app): State<AppState>,
    Path(order_code): Path<String>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<PublicOrderStatus>,
    ),
    ApiError,
> {
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open order lookup", &e))?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let order = load_public_order_tx(&mut tx, &order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?;
    // Commit (not roll back): the sweep above is a real state change.
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit order lookup", &e))?;

    let Some(order) = order else {
        return Err(not_found("order not found"));
    };
    // Order state is live data (reserved/paid/expired changes answers):
    // nothing may cache it.
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(order)))
}

// ---------------------------------------------------------------------------
// Handlers: Stripe webhook (PAY-01)
// ---------------------------------------------------------------------------

/// `POST /api/webhooks/stripe` — the payment authority. A verified,
/// first-time event settles a `reserved` order and consumes its units; a
/// replay is answered `idempotent: true` with no second stock deduction
/// (PAY-01); a payment against a lapsed reservation is flagged
/// `late_payment_review` (R2-5).
pub async fn stripe_webhook(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookResponse>, ApiError> {
    let payload =
        parse_signed_webhook(config::stripe_webhook_secret().as_deref(), &headers, &body)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open webhook transaction", &e))?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let Some(order) = lock_order_by_code(&mut tx, &payload.order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };

    // The gateway event must match the order it claims to pay — a verified
    // signature alone must not settle somebody else's total.
    if payload.amount_cents != order.total_cents
        || !payload.currency.eq_ignore_ascii_case(&order.currency)
    {
        return Err(bad_request("payment amount or currency mismatch"));
    }

    // PAY-01 idempotency ledger: the event_id PRIMARY KEY + DO NOTHING is
    // what makes a replay (or a crash-then-redelivery) a no-op. Inserted
    // BEFORE the state change so both live or both roll back.
    let recorded = sqlx::query(
        "INSERT INTO payment_events (event_id, order_id, event_type, amount_cents, currency) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&payload.event_id)
    .bind(order.id)
    .bind(&payload.event_type)
    .bind(payload.amount_cents)
    .bind(&payload.currency)
    .execute(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to record payment event", &e))?;
    if recorded.rows_affected() == 0 {
        tx.commit()
            .await
            .map_err(|e| state::database_unavailable("failed to commit webhook replay", &e))?;
        return Ok(Json(WebhookResponse {
            ok: true,
            idempotent: true,
            status: order.status,
            order_code: order.order_code.clone(),
            customer_name: order.customer_name.clone(),
            customer_email: order.customer_email.clone(),
            locale: order.locale.clone(),
            paid_email_sent: order.paid_email_sent_at.is_some(),
        }));
    }

    let new_status = match payment_transition(&order.status) {
        PaymentTransition::MarkPaid => {
            let items = load_order_items_tx(
                &mut tx,
                order.id,
                order.product_id,
                &order.product_slug,
                order.quantity,
                order.unit_price_cents,
                order.total_cents,
            )
            .await
            .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
            for item in &items {
                sqlx::query(
                    "UPDATE products SET stock_total = stock_total - $1, \
                         stock_reserved = stock_reserved - $1 WHERE id = $2",
                )
                .bind(item.quantity)
                .bind(item.product_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| state::database_unavailable("failed to consume reserved stock", &e))?;
            }
            sqlx::query("UPDATE orders SET status = 'paid', updated_at = now() WHERE id = $1")
                .bind(order.id)
                .execute(&mut *tx)
                .await
                .map_err(|e| state::database_unavailable("failed to mark order paid", &e))?;
            "paid"
        }
        PaymentTransition::LatePaymentReview => {
            sqlx::query(
                "UPDATE orders SET status = 'late_payment_review', updated_at = now() \
                 WHERE id = $1",
            )
            .bind(order.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to flag late payment", &e))?;
            "late_payment_review"
        }
        PaymentTransition::NoChange => order.status.as_str(),
    };
    if new_status != order.status {
        info!(
            order_code = %order.order_code,
            status = new_status,
            event_id = %payload.event_id,
            "stripe webhook applied"
        );
    }

    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit webhook", &e))?;
    Ok(Json(WebhookResponse {
        ok: true,
        idempotent: false,
        status: new_status.to_string(),
        order_code: order.order_code.clone(),
        customer_name: order.customer_name.clone(),
        customer_email: order.customer_email.clone(),
        locale: order.locale.clone(),
        paid_email_sent: order.paid_email_sent_at.is_some(),
    }))
}

// ---------------------------------------------------------------------------
// Handlers: admin order operations
// ---------------------------------------------------------------------------

/// `GET /api/admin/orders` — the last 200 orders, newest first.
pub async fn list_orders(
    State(app): State<AppState>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Vec<AdminOrder>>,
    ),
    ApiError,
> {
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open orders list", &e))?;
    let rows = sqlx::query_as::<_, AdminOrderRow>(
        "SELECT id, order_code, product_id, product_slug, quantity, unit_price_cents, \
             total_cents, currency, payment_method, locale, status, customer_name, \
             customer_email, customer_phone, delivery_address, fps_reference, \
             paid_email_sent_at, email_last_error, expires_at, created_at, updated_at \
         FROM orders ORDER BY created_at DESC, id DESC LIMIT 200",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to load orders", &e))?;

    let mut orders = Vec::with_capacity(rows.len());
    for row in rows {
        let items = load_order_items_tx(
            &mut tx,
            row.id,
            row.product_id,
            &row.product_slug,
            row.quantity,
            row.unit_price_cents,
            row.total_cents,
        )
        .await
        .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
        orders.push(AdminOrder {
            id: row.id,
            order_code: row.order_code,
            product_id: row.product_id,
            product_slug: row.product_slug,
            quantity: row.quantity,
            unit_price_cents: row.unit_price_cents,
            total_cents: row.total_cents,
            currency: row.currency,
            payment_method: row.payment_method,
            locale: row.locale,
            status: row.status,
            customer_name: row.customer_name,
            customer_email: row.customer_email,
            customer_phone: row.customer_phone,
            delivery_address: row.delivery_address,
            fps_reference: row.fps_reference,
            paid_email_sent_at: row.paid_email_sent_at,
            email_last_error: row.email_last_error,
            expires_at: row.expires_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            items,
        });
    }
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit orders list", &e))?;
    // Order state is live data: never cacheable.
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(orders)))
}

/// `POST /api/admin/orders/{id}/confirm-fps` — an FPS transfer arrived:
/// settle the reservation (`reserved -> paid`, units consumed) or, when the
/// reservation already lapsed, flag `late_payment_review` (R2-5: reviewed
/// and refunded, never silently fulfilled against released stock).
pub async fn confirm_fps_order(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
    body: Result<Json<ConfirmFpsRequest>, JsonRejection>,
) -> Result<Json<AdminOrder>, ApiError> {
    let actor = actor(user)?;
    let body = json_body(body)?;
    if body.fps_reference.trim().is_empty() {
        return Err(bad_request("fps_reference is required"));
    }
    if body.fps_reference.contains('\0') {
        return Err(bad_request("fps_reference contains a NUL byte"));
    }
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let mut tx = begin_order_tx(pool, "FPS confirmation").await?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let Some(order) = lock_order_by_id(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if order.payment_method != "fps" {
        return Err(conflict("order is not awaiting FPS confirmation"));
    }

    let (status_after, audit_action) = match fps_confirmation_transition(&order.status) {
        FpsTransition::MarkPaid => {
            let items = load_order_items_tx(
                &mut tx,
                order.id,
                order.product_id,
                &order.product_slug,
                order.quantity,
                order.unit_price_cents,
                order.total_cents,
            )
            .await
            .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
            for item in &items {
                sqlx::query(
                    "UPDATE products SET stock_total = stock_total - $1, \
                         stock_reserved = stock_reserved - $1 WHERE id = $2",
                )
                .bind(item.quantity)
                .bind(item.product_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| state::database_unavailable("failed to consume reserved stock", &e))?;
            }
            ("paid", "confirm_fps")
        }
        FpsTransition::LatePaymentReview => ("late_payment_review", "late_fps_review"),
        FpsTransition::Reject => {
            return Err(conflict("order is not awaiting FPS confirmation"));
        }
    };
    sqlx::query(
        "UPDATE orders SET status = $2, fps_reference = $3, updated_at = now() WHERE id = $1",
    )
    .bind(order.id)
    .bind(status_after)
    .bind(&body.fps_reference)
    .execute(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to update order", &e))?;

    insert_audit(
        &mut tx,
        &actor,
        audit_action,
        "orders",
        Some(&order.id.to_string()),
        serde_json::json!({
            "order_code": order.order_code,
            "quantity": order.quantity,
            "total_cents": order.total_cents,
            "currency": order.currency,
            "fps_reference": body.fps_reference,
        }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to write audit row", &e))?;

    finish_order_mutation(tx, order.id).await
}

/// `POST /api/admin/orders/{id}/release` — cancel an unpaid `reserved` order
/// and return its units to the saleable pool (STOCK-01: a release is not a
/// restock, the units never left the building).
pub async fn release_order(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> Result<Json<AdminOrder>, ApiError> {
    let actor = actor(user)?;
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let mut tx = begin_order_tx(pool, "order release").await?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let Some(order) = lock_order_by_id(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if release_transition(&order.status) == ReleaseTransition::Reject {
        return Err(conflict("order is not reserved"));
    }

    let items = load_order_items_tx(
        &mut tx,
        order.id,
        order.product_id,
        &order.product_slug,
        order.quantity,
        order.unit_price_cents,
        order.total_cents,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
    for item in &items {
        sqlx::query("UPDATE products SET stock_reserved = stock_reserved - $1 WHERE id = $2")
            .bind(item.quantity)
            .bind(item.product_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to release reserved stock", &e))?;
    }
    sqlx::query("UPDATE orders SET status = 'cancelled', updated_at = now() WHERE id = $1")
        .bind(order.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to cancel order", &e))?;

    insert_audit(
        &mut tx,
        &actor,
        "release",
        "orders",
        Some(&order.id.to_string()),
        serde_json::json!({ "order_code": order.order_code, "quantity": order.quantity }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to write audit row", &e))?;

    finish_order_mutation(tx, order.id).await
}

/// `POST /api/admin/orders/{id}/quarantine-return` — a sold unit came back
/// (STOCK-01 / R2-11): it re-enters `stock_total` but lands in
/// `stock_quarantined`, so the SALEABLE count is unchanged — a returned lamp
/// is inspected, never silently resold.
pub async fn quarantine_return_order(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> Result<Json<AdminOrder>, ApiError> {
    let actor = actor(user)?;
    let pool = state::require_pool(&app)?;

    let mut tx = begin_order_tx(pool, "quarantine return").await?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let Some(order) = lock_order_by_id(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if quarantine_transition(&order.status) == ReturnTransition::Reject {
        return Err(conflict("order is not paid or fulfilled"));
    }

    // Back in the building (stock_total) but NOT on sale (stock_quarantined):
    // `available = stock_total - stock_reserved - stock_quarantined` is
    // deliberately unchanged by this write.
    let items = load_order_items_tx(
        &mut tx,
        order.id,
        order.product_id,
        &order.product_slug,
        order.quantity,
        order.unit_price_cents,
        order.total_cents,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
    for item in &items {
        sqlx::query(
            "UPDATE products SET stock_total = stock_total + $1, \
                 stock_quarantined = stock_quarantined + $1 WHERE id = $2",
        )
        .bind(item.quantity)
        .bind(item.product_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to quarantine returned stock", &e))?;
    }
    sqlx::query(
        "UPDATE orders SET status = 'returned_quarantined', updated_at = now() WHERE id = $1",
    )
    .bind(order.id)
    .execute(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to mark order returned", &e))?;

    insert_audit(
        &mut tx,
        &actor,
        "quarantine_return",
        "orders",
        Some(&order.id.to_string()),
        serde_json::json!({ "order_code": order.order_code, "quantity": order.quantity }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to write audit row", &e))?;

    finish_order_mutation(tx, order.id).await
}

/// `POST /api/orders/{order_code}/submit-fps` — customer submits their HK FPS
/// transfer reference directly from the checkout screen.
pub async fn submit_customer_fps(
    State(app): State<AppState>,
    Path(order_code): Path<String>,
    body: Result<Json<ConfirmFpsRequest>, JsonRejection>,
) -> Result<Json<PublicOrderStatus>, ApiError> {
    let body = json_body(body)?;
    let fps_ref = body.fps_reference.trim();
    if fps_ref.is_empty() {
        return Err(bad_request("fps_reference is required"));
    }
    if fps_ref.contains('\0') {
        return Err(bad_request("fps_reference contains a NUL byte"));
    }
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let mut tx = begin_order_tx(pool, "customer FPS reference").await?;
    let Some(order) = lock_order_by_code(&mut tx, &order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if order.payment_method != "fps" {
        return Err(conflict("order does not use FPS payment"));
    }
    let next_status = match order.status.as_str() {
        "reserved" => "reserved",
        "expired" => "late_payment_review",
        _ => return Err(conflict("order is not awaiting FPS reference")),
    };

    sqlx::query(
        "UPDATE orders SET fps_reference = $2, status = $3, updated_at = now() WHERE id = $1",
    )
    .bind(order.id)
    .bind(fps_ref)
    .bind(next_status)
    .execute(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to save FPS reference", &e))?;

    let updated = load_public_order_tx(&mut tx, &order.order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to reload order", &e))?
        .ok_or_else(|| not_found("order not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit FPS reference", &e))?;
    Ok(Json(updated))
}

/// `POST /api/orders/{order_code}/cancel` — customer self-cancels an unpaid
/// reservation hold (e.g. to switch payment method or free the piece).
pub async fn cancel_customer_order(
    State(app): State<AppState>,
    Path(order_code): Path<String>,
) -> Result<Json<PublicOrderStatus>, ApiError> {
    let pool = state::require_pool(&app)?;
    sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let mut tx = begin_order_tx(pool, "customer cancel order").await?;
    let Some(order) = lock_order_by_code(&mut tx, &order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if release_transition(&order.status) == ReleaseTransition::Reject {
        return Err(conflict("order is not reserved"));
    }

    let items = load_order_items_tx(
        &mut tx,
        order.id,
        order.product_id,
        &order.product_slug,
        order.quantity,
        order.unit_price_cents,
        order.total_cents,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to load order items", &e))?;
    for item in &items {
        sqlx::query(
            "UPDATE products SET stock_reserved = GREATEST(0, stock_reserved - $1) WHERE id = $2",
        )
        .bind(item.quantity)
        .bind(item.product_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to release reserved stock", &e))?;
    }
    sqlx::query("UPDATE orders SET status = 'cancelled', updated_at = now() WHERE id = $1")
        .bind(order.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to cancel order", &e))?;

    let updated = load_public_order_tx(&mut tx, &order.order_code)
        .await
        .map_err(|e| state::database_unavailable("failed to reload order", &e))?
        .ok_or_else(|| not_found("order not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit order cancellation", &e))?;
    Ok(Json(updated))
}

/// `POST /api/orders/{order_code}/email-delivery` — the web tier's durable
/// record of the paid-confirmation email outcome (findings G02). The ledger
/// separates RECOVERABLE claim state (`paid_email_claimed_at`: "a send is in
/// flight") from CONFIRMED delivery (`paid_email_sent_at`: "the customer was
/// told"), so a crash mid-dispatch can never fake — or block — a delivery.
/// Every send path (webhook settlement, session-verify return, direct card,
/// admin FPS confirmation, admin re-send) claims here before sending and
/// reports the outcome afterwards, so no customer gets two receipts and no
/// failure disappears silently.
///
/// Four call shapes share this one endpoint:
/// * `claim: true` — ATOMICALLY win the one paid-email send: only a
///   `paid`/`fulfilled` order whose `paid_email_sent_at IS NULL` and whose
///   claim is free (never taken, or the 2-minute lease expired) can be
///   claimed, and only one caller ever gets `claimed: true`. The claim is
///   taken BEFORE the dispatch so concurrent settlement paths (webhook +
///   return + admin) can never double-send. A won claim reports
///   `paid_email_sent: false` — the claim is a lease, NOT delivery proof;
///   only a successful dispatch stamps `paid_email_sent_at`.
/// * `sent: true` + `kind: "reservation"` — the hold-confirmation email was
///   delivered: clear `email_last_error` ONLY. It must never touch
///   `paid_email_sent_at` (the PAID email ledger) — not even on an order
///   that later settled.
/// * `sent: true` (any other kind) — the paid receipt was delivered:
///   stamp `paid_email_sent_at` ONLY when the order is `paid`/`fulfilled`
///   (COALESCE'd so a repeat can never move the first moment) and release
///   the claim.
/// * `sent: false` — an attempt failed after claiming (or a manual re-send
///   failed later): RELEASE the claim (`paid_email_claimed_at = NULL`) and
///   record `email_last_error`, so a retry or the admin re-send can still
///   succeed. An already-confirmed `paid_email_sent_at` is NEVER wiped by a
///   later failed re-send.
///
/// Deliberately NO reservation sweep and no state transition: this is
/// delivery bookkeeping on whatever state the order is in — flipping
/// `expired -> paid` or releasing holds here would mix concerns.
pub async fn record_email_delivery(
    State(app): State<AppState>,
    Path(order_code): Path<String>,
    body: Result<Json<EmailDeliveryRequest>, JsonRejection>,
) -> Result<Json<EmailDeliveryResponse>, ApiError> {
    let body = json_body(body)?;
    // The failure note is stored TEXT: NUL-free (or the DB would answer 503
    // to a client mistake) and bounded (one stuck retry loop must not write
    // megabytes per failure).
    if let Some(error_note) = body.error.as_deref() {
        if let Err(msg) = reject_nul("error", error_note) {
            return Err(bad_request(&msg));
        }
        if error_note.len() > 2000 {
            return Err(bad_request("error must be at most 2000 characters"));
        }
    }
    let pool = state::require_pool(&app)?;
    let error_note = body.error.unwrap_or_default();

    // CLAIM: atomically win the right to send THE paid email exactly once.
    // `paid_email_claimed_at` is a 2-minute RECOVERABLE lease on the send,
    // not delivery proof: a web tier that dies mid-dispatch frees the send
    // after the lease instead of blocking the receipt forever, while a live
    // lease keeps concurrent settlement paths silent.
    if body.claim {
        let claimed: Option<(String,)> = sqlx::query_as(
            "UPDATE orders SET paid_email_claimed_at = now(), updated_at = now() \
             WHERE order_code = $1 \
               AND status IN ('paid', 'fulfilled') \
               AND paid_email_sent_at IS NULL \
               AND (paid_email_claimed_at IS NULL \
                    OR paid_email_claimed_at <= now() - interval '2 minutes') \
             RETURNING order_code",
        )
        .bind(&order_code)
        .fetch_optional(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to claim paid email delivery", &e))?;
        if claimed.is_some() {
            // The claim is a lease, never proof of delivery: the paid email
            // has NOT gone out yet — `paid_email_sent_at` stays NULL until
            // the claimed dispatch actually succeeds.
            return Ok(Json(EmailDeliveryResponse {
                ok: true,
                paid_email_sent: false,
                claimed: true,
            }));
        }
        // Nothing claimed — already claimed/sent, or the order is not paid:
        // report the ledger state unchanged (404 only for an unknown order).
        let current: Option<(bool,)> = sqlx::query_as(
            "SELECT (paid_email_sent_at IS NOT NULL) FROM orders WHERE order_code = $1",
        )
        .bind(&order_code)
        .fetch_optional(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to read email delivery ledger", &e))?;
        let Some((paid_email_sent,)) = current else {
            return Err(not_found("order not found"));
        };
        return Ok(Json(EmailDeliveryResponse {
            ok: true,
            paid_email_sent,
            claimed: false,
        }));
    }

    // `(statement, uses_$2)`: reservation-kind outcomes never touch the
    // paid-email claim or sent timestamp, whether `sent` is true or false.
    let (update_sql, binds_error_note) = if body.kind.as_deref() == Some("reservation") {
        let sql = if body.sent {
            "UPDATE orders SET email_last_error = '', updated_at = now() \
                 WHERE order_code = $1 \
                 RETURNING (paid_email_sent_at IS NOT NULL)"
        } else {
            "UPDATE orders SET email_last_error = $2, updated_at = now() \
                 WHERE order_code = $1 \
                 RETURNING (paid_email_sent_at IS NOT NULL)"
        };
        (sql, !body.sent)
    } else if body.sent {
        // The PAID receipt was delivered: stamp the delivery moment ONLY for
        // a settled order (re-sending a hold email on a `reserved` order must
        // never record the PAID email as sent) and release the claim.
        (
            "UPDATE orders SET \
                     paid_email_sent_at = CASE WHEN status IN ('paid', 'fulfilled') \
                         THEN COALESCE(paid_email_sent_at, now()) ELSE paid_email_sent_at END, \
                     paid_email_claimed_at = NULL, \
                     email_last_error = $2, updated_at = now() \
                 WHERE order_code = $1 \
                 RETURNING (paid_email_sent_at IS NOT NULL)",
            true,
        )
    } else {
        // An attempt failed (claimed automatic send, or a later manual
        // re-send): RELEASE the claim and keep the failure reason, so a
        // retry or the admin re-send surface can still reach the customer.
        // An already-confirmed `paid_email_sent_at` is deliberately NOT
        // wiped — one failed re-send must never un-tell a customer who was
        // already told.
        (
            "UPDATE orders SET paid_email_claimed_at = NULL, email_last_error = $2, \
                     updated_at = now() \
                 WHERE order_code = $1 \
                 RETURNING (paid_email_sent_at IS NOT NULL)",
            true,
        )
    };
    let mut update = sqlx::query_as::<_, (bool,)>(update_sql).bind(&order_code);
    if binds_error_note {
        update = update.bind(&error_note);
    }
    let updated: Option<(bool,)> = update
        .fetch_optional(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to record email delivery", &e))?;
    let Some((paid_email_sent,)) = updated else {
        return Err(not_found("order not found"));
    };
    Ok(Json(EmailDeliveryResponse {
        ok: true,
        paid_email_sent,
        claimed: false,
    }))
}

/// `POST /api/admin/orders/{id}/fulfil` — marks a `paid` order as `fulfilled`
/// once dispatched or handed over to the collector.
pub async fn fulfil_order(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<i64>,
) -> Result<Json<AdminOrder>, ApiError> {
    let actor = actor(user)?;
    let pool = state::require_pool(&app)?;

    let mut tx = begin_order_tx(pool, "order fulfilment").await?;
    expire_stale_reservations(&mut tx)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;
    let Some(order) = lock_order_by_id(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load order", &e))?
    else {
        return Err(not_found("order not found"));
    };
    if order.status != "paid" {
        return Err(conflict("order is not paid"));
    }

    sqlx::query("UPDATE orders SET status = 'fulfilled', updated_at = now() WHERE id = $1")
        .bind(order.id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to mark order fulfilled", &e))?;

    insert_audit(
        &mut tx,
        &actor,
        "fulfil",
        "orders",
        Some(&order.id.to_string()),
        serde_json::json!({ "order_code": order.order_code, "quantity": order.quantity }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to write audit row", &e))?;

    finish_order_mutation(tx, order.id).await
}

async fn begin_order_tx(
    pool: &PgPool,
    context: &'static str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, ApiError> {
    pool.begin()
        .await
        .map_err(|e| state::database_unavailable(context, &e))
}

/// Commits an admin order mutation and answers with the committed row
/// (read-back inside the transaction: the response must be exactly what
/// lands).
async fn finish_order_mutation(
    mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
    id: i64,
) -> Result<Json<AdminOrder>, ApiError> {
    let updated = load_admin_order(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to reload order", &e))?
        .ok_or_else(|| not_found("order not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit order update", &e))?;
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn request() -> CreateReservationRequest {
        CreateReservationRequest {
            slug: "ph-2-1-table-chrome".to_string(),
            quantity: 2,
            items: Vec::new(),
            payment_method: "card".to_string(),
            customer_name: "Ada Buyer".to_string(),
            customer_email: "ada@example.com".to_string(),
            customer_phone: "+852 0000 0000".to_string(),
            delivery_address: "1 Test Road".to_string(),
            locale: "en".to_string(),
            idempotency_key: None,
            expected_total_cents: None,
        }
    }

    /// Asserts the exact (status, body) an [`ApiError`] carries — the error
    /// bodies are a client contract, so they are compared byte-for-byte.
    fn assert_error(err: ApiError, status: StatusCode, body: &str) {
        assert_eq!(err.0, status, "unexpected status for {body}");
        let Json(error) = err.1;
        assert_eq!(serde_json::to_string(&error).unwrap(), body);
    }

    // -- validation ---------------------------------------------------------

    #[test]
    fn validate_reservation_accepts_both_payment_methods() {
        let mut req = request();
        assert_eq!(validate_reservation(&req), Ok(()));
        req.payment_method = "fps".to_string();
        assert_eq!(validate_reservation(&req), Ok(()));
        // Optional fields may stay empty.
        req.customer_phone = String::new();
        req.delivery_address = String::new();
        assert_eq!(validate_reservation(&req), Ok(()));
    }

    #[test]
    fn reservation_json_defaults_the_optional_fields() {
        let parsed: CreateReservationRequest = serde_json::from_str(
            r#"{"slug":"s","payment_method":"fps","customer_name":"A","customer_email":"a@b.c"}"#,
        )
        .expect("body must parse");
        assert_eq!(parsed.quantity, 1);
        assert_eq!(parsed.customer_phone, "");
        assert_eq!(parsed.delivery_address, "");
        assert_eq!(parsed.locale, "en");
        assert_eq!(parsed.idempotency_key, None);
        assert_eq!(validate_reservation(&parsed), Ok(()));
    }

    #[test]
    fn validate_reservation_checks_locale_and_idempotency_key() {
        // Locale is the LANG-01 pair only: a typo must never silently send
        // the wrong-language receipt (0008 CHECK mirrors this rule).
        let mut req = request();
        req.locale = "zh-HK".to_string();
        assert_eq!(validate_reservation(&req), Ok(()));
        for bad_locale in ["fr", "EN", "zh-hk", "zh-HK ", ""] {
            req.locale = bad_locale.to_string();
            assert_eq!(
                validate_reservation(&req),
                Err("locale must be \"en\" or \"zh-HK\"".to_string()),
                "{bad_locale:?}"
            );
        }

        // Idempotency key is OPTIONAL and blank means absent (the same rule
        // the normalizer applies).
        req = request();
        for absent in [None, Some(""), Some("   ")] {
            req.idempotency_key = absent.map(str::to_string);
            assert_eq!(validate_reservation(&req), Ok(()), "{absent:?}");
        }
        // A present key is NUL-free and bounded after trimming.
        req.idempotency_key = Some("  pay-attempt-1  ".to_string());
        assert_eq!(validate_reservation(&req), Ok(()));
        req.idempotency_key = Some("x".repeat(128));
        assert_eq!(validate_reservation(&req), Ok(()));
        req.idempotency_key = Some(" ".repeat(4) + &"x".repeat(125));
        assert_eq!(validate_reservation(&req), Ok(()));
        req.idempotency_key = Some("x".repeat(129));
        assert_eq!(
            validate_reservation(&req),
            Err("idempotency_key must be at most 128 characters".to_string())
        );
        req.idempotency_key = Some(format!("{}{}", " ".repeat(4), "x".repeat(129)));
        assert_eq!(
            validate_reservation(&req),
            Err("idempotency_key must be at most 128 characters".to_string())
        );
        req.idempotency_key = Some("a\u{0}b".to_string());
        assert_eq!(
            validate_reservation(&req),
            Err("idempotency_key contains a NUL byte".to_string())
        );
    }

    #[test]
    fn email_delivery_request_defaults_error_to_none_and_claim_to_false() {
        let parsed: EmailDeliveryRequest =
            serde_json::from_str(r#"{"sent":true}"#).expect("body must parse");
        assert!(parsed.sent);
        assert_eq!(parsed.error, None);
        assert!(!parsed.claim, "claim must default to false");
        assert_eq!(parsed.kind, None, "kind must default to none (paid)");

        let claim: EmailDeliveryRequest =
            serde_json::from_str(r#"{"claim":true,"sent":false}"#).expect("claim body must parse");
        assert!(claim.claim);
        assert!(!claim.sent);
        assert_eq!(claim.error, None);
        assert_eq!(claim.kind, None);

        let reservation: EmailDeliveryRequest =
            serde_json::from_str(r#"{"sent":true,"kind":"reservation"}"#)
                .expect("reservation outcome body must parse");
        assert!(reservation.sent);
        assert_eq!(reservation.kind.as_deref(), Some("reservation"));
    }

    #[test]
    fn validate_reservation_rejects_every_bad_field() {
        /// One bad-field mutation applied to an otherwise valid request.
        type Mutation = fn(&mut CreateReservationRequest);
        let cases: &[(Mutation, &str)] = &[
            (|r| r.slug = String::new(), "slug is required"),
            (|r| r.slug = "   ".to_string(), "slug is required"),
            (
                |r| r.slug = "a\u{0}b".to_string(),
                "slug contains a NUL byte",
            ),
            (|r| r.quantity = 0, "quantity must be between 1 and 16"),
            (|r| r.quantity = 17, "quantity must be between 1 and 16"),
            (|r| r.quantity = -1, "quantity must be between 1 and 16"),
            (
                |r| r.payment_method = "paypal".to_string(),
                "payment_method must be \"card\" or \"fps\"",
            ),
            (
                |r| r.payment_method = "Card".to_string(),
                "payment_method must be \"card\" or \"fps\"",
            ),
            (
                |r| r.payment_method = String::new(),
                "payment_method must be \"card\" or \"fps\"",
            ),
            (
                |r| r.customer_name = "  ".to_string(),
                "customer_name is required",
            ),
            (
                |r| r.customer_name = "A\u{0}".to_string(),
                "customer_name contains a NUL byte",
            ),
            (
                |r| r.customer_email = "no-at-sign".to_string(),
                "customer_email must contain @",
            ),
            (
                |r| r.customer_email = String::new(),
                "customer_email must contain @",
            ),
            (
                |r| r.customer_email = "a@b\u{0}.c".to_string(),
                "customer_email contains a NUL byte",
            ),
            (
                |r| r.customer_phone = "+852\u{0}".to_string(),
                "customer_phone contains a NUL byte",
            ),
            (
                |r| r.delivery_address = "x\u{0}y".to_string(),
                "delivery_address contains a NUL byte",
            ),
        ];
        for (mutate, expected) in cases {
            let mut req = request();
            mutate(&mut req);
            assert_eq!(
                validate_reservation(&req),
                Err(expected.to_string()),
                "{expected}"
            );
        }
    }

    // -- money ---------------------------------------------------------------

    #[test]
    fn order_total_is_computed_from_the_db_price() {
        // 161500 cents x 2 — the client never contributes a factor.
        assert_eq!(order_total(161_500, 2), Some(323_000));
        assert_eq!(order_total(161_500, 1), Some(161_500));
        assert_eq!(order_total(0, 16), Some(0));
    }

    #[test]
    fn order_total_refuses_to_wrap() {
        // A wrapped total would silently sell at the wrong price.
        assert_eq!(order_total(i64::MAX, 2), None);
        assert_eq!(order_total(i64::MAX / 2 + 1, 2), None);
        assert!(order_total(i64::MAX / 16, 16).is_some());
    }

    // -- order codes ----------------------------------------------------------

    #[test]
    fn order_codes_are_unique_per_call() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let code = next_order_code();
            assert!(code.starts_with("ORD-"), "{code}");
            assert!(seen.insert(code), "order code must never repeat");
        }
    }

    // -- Stripe signature ------------------------------------------------------

    #[test]
    fn stripe_signature_matches_the_known_answer() {
        // Known-answer vectors computed OUTSIDE this crate (python
        // hmac/sha256 over `"{t}.{body}"`): they pin the exact payload
        // composition and hex encoding — a self-signed round trip alone
        // could not catch a consistently wrong construction.
        let secret = b"whsec_test_0123456789abcdef";
        let body = br#"{"event_id":"evt_1"}"#;
        let header = stripe_signature_header(secret, 1_712_345_678, body);
        assert_eq!(
            header,
            "t=1712345678,v1=499d7e645aaa7e326cd1d1988112326acb589e2b132d32421855aed4df4aa236"
        );
        assert_eq!(verify_stripe_signature(secret, &header, body), Ok(()));

        let secret = b"0123456789abcdef";
        let body = br#"{"hello":"world"}"#;
        let header = stripe_signature_header(secret, 1_699_999_999, body);
        assert_eq!(
            header,
            "t=1699999999,v1=f0d05a28453d3eac6c2987aef76a9532117e0ae7106da3a06da1cb0fa7af82fe"
        );
        assert_eq!(verify_stripe_signature(secret, &header, body), Ok(()));
    }

    #[test]
    fn stripe_signature_rejects_tampering_and_malformed_headers() {
        let secret = b"0123456789abcdef";
        let body = br#"{"amount_cents":100}"#;
        let header = stripe_signature_header(secret, 1_699_999_999, body);

        // Body changed after signing: the signature no longer covers it.
        assert!(verify_stripe_signature(secret, &header, br#"{"amount_cents":1}"#).is_err());
        // Timestamp changed after signing: t is INSIDE the signed payload.
        let tampered_t = header.replacen("t=1699999999", "t=1699999998", 1);
        assert!(verify_stripe_signature(secret, &tampered_t, body).is_err());
        // Wrong key.
        assert!(verify_stripe_signature(b"other-secret-012345", &header, body).is_err());

        for bad_header in [
            "",                    // empty
            "t=1699999999",        // no v1
            "v1=00",               // no t
            "nonsense",            // no '=' parts
            "t=,v1=00",            // empty t
            "t=1699999999,v1=",    // empty v1
            "t=1699999999,v1=zz",  // not hex
            "t=1699999999,v1=abc", // odd-length hex
            "t=1699999999,v1=0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert!(
                verify_stripe_signature(secret, bad_header, body).is_err(),
                "{bad_header:?} must not verify"
            );
        }
    }

    #[test]
    fn stripe_signature_accepts_alternate_spellings() {
        let secret = b"0123456789abcdef";
        let body = br#"{"hello":"world"}"#;
        let header = stripe_signature_header(secret, 1_699_999_999, body);
        let signature = header.split("v1=").nth(1).unwrap();

        // Uppercase hex is the same signature.
        let upper = format!("t=1699999999,v1={}", signature.to_ascii_uppercase());
        assert_eq!(verify_stripe_signature(secret, &upper, body), Ok(()));
        // Unknown schemes (v0, ...) ride along and are ignored.
        let with_v0 = format!("t=1699999999,v0=deadbeef,v1={signature}");
        assert_eq!(verify_stripe_signature(secret, &with_v0, body), Ok(()));
        // Whitespace around the parts is tolerated.
        let spaced = format!(" t=1699999999 , v1={signature} ");
        assert_eq!(verify_stripe_signature(secret, &spaced, body), Ok(()));
        // Multiple v1 values (key rotation): any match verifies.
        let rotated = format!("t=1699999999,v1={},v1={signature}", "00".repeat(32));
        assert_eq!(verify_stripe_signature(secret, &rotated, body), Ok(()));
    }

    // -- webhook front half ----------------------------------------------------

    const WEBHOOK_BODY: &[u8] = br#"{"event_id":"evt_1","event_type":"payment_intent.succeeded","order_code":"ORD-1-2","amount_cents":161500,"currency":"HKD"}"#;
    const WEBHOOK_SECRET: &[u8] = b"0123456789abcdef";

    fn headers_with(signature: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(signature) = signature {
            headers.insert(
                "stripe-signature",
                HeaderValue::from_str(signature).expect("signature is a header value"),
            );
        }
        headers
    }

    #[test]
    fn webhook_without_a_configured_secret_is_503() {
        let headers = headers_with(Some(&stripe_signature_header(
            WEBHOOK_SECRET,
            1,
            WEBHOOK_BODY,
        )));
        assert_error(
            parse_signed_webhook(None, &headers, WEBHOOK_BODY).unwrap_err(),
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"stripe webhook not configured"}"#,
        );
    }

    #[test]
    fn webhook_with_a_missing_or_bad_signature_is_401() {
        // Missing header.
        assert_error(
            parse_signed_webhook(Some(WEBHOOK_SECRET), &headers_with(None), WEBHOOK_BODY)
                .unwrap_err(),
            StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid stripe signature"}"#,
        );
        // Header present but not a signature.
        assert_error(
            parse_signed_webhook(
                Some(WEBHOOK_SECRET),
                &headers_with(Some("nope")),
                WEBHOOK_BODY,
            )
            .unwrap_err(),
            StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid stripe signature"}"#,
        );
        // Body signed under a DIFFERENT secret.
        let forged = stripe_signature_header(b"another-secret-01", 1, WEBHOOK_BODY);
        assert_error(
            parse_signed_webhook(
                Some(WEBHOOK_SECRET),
                &headers_with(Some(&forged)),
                WEBHOOK_BODY,
            )
            .unwrap_err(),
            StatusCode::UNAUTHORIZED,
            r#"{"error":"invalid stripe signature"}"#,
        );
    }

    #[test]
    fn webhook_with_malformed_payload_is_400_after_verification() {
        for body in [
            b"not json".as_slice(),
            b"{}".as_slice(),
            br#"{"event_id":"","event_type":"t","order_code":"ORD-1-2","amount_cents":1,"currency":"HKD"}"#.as_slice(),
            br#"{"event_id":"evt_1","event_type":"t","order_code":" ","amount_cents":1,"currency":"HKD"}"#.as_slice(),
        ] {
            let headers = headers_with(Some(&stripe_signature_header(
                WEBHOOK_SECRET, 1, body,
            )));
            assert_error(
                parse_signed_webhook(Some(WEBHOOK_SECRET), &headers, body).unwrap_err(),
                StatusCode::BAD_REQUEST,
                r#"{"error":"invalid stripe payload"}"#,
            );
        }
    }

    #[test]
    fn webhook_with_a_verified_signature_parses_the_payload() {
        let headers = headers_with(Some(&stripe_signature_header(
            WEBHOOK_SECRET,
            1_699_999_999,
            WEBHOOK_BODY,
        )));
        let payload = parse_signed_webhook(Some(WEBHOOK_SECRET), &headers, WEBHOOK_BODY)
            .expect("a correctly signed payload must parse");
        assert_eq!(payload.event_id, "evt_1");
        assert_eq!(payload.event_type, "payment_intent.succeeded");
        assert_eq!(payload.order_code, "ORD-1-2");
        assert_eq!(payload.amount_cents, 161_500);
        assert_eq!(payload.currency, "HKD");

        // Real Stripe Event envelope (nested data.object.metadata.order_code + amount_total).
        let envelope = br#"{"id":"evt_live_99","type":"checkout.session.completed","data":{"object":{"metadata":{"order_code":"ORD-99-1"},"amount_total":714000,"currency":"hkd"}}}"#;
        let env_headers = headers_with(Some(&stripe_signature_header(
            WEBHOOK_SECRET,
            1_699_999_999,
            envelope,
        )));
        let parsed_env = parse_signed_webhook(Some(WEBHOOK_SECRET), &env_headers, envelope)
            .expect("real Stripe Event envelope must parse");
        assert_eq!(parsed_env.event_id, "evt_live_99");
        assert_eq!(parsed_env.event_type, "checkout.session.completed");
        assert_eq!(parsed_env.order_code, "ORD-99-1");
        assert_eq!(parsed_env.amount_cents, 714_000);
        assert_eq!(parsed_env.currency, "hkd");

        // Unpaid checkout.session.completed (e.g. delayed async payment method) is rejected 400 (PAY-01 / STRIPE-01).
        let unpaid_session = br#"{"id":"evt_unpaid_1","type":"checkout.session.completed","data":{"object":{"payment_status":"unpaid","metadata":{"order_code":"ORD-99-1"},"amount_total":714000,"currency":"hkd"}}}"#;
        let unpaid_headers = headers_with(Some(&stripe_signature_header(
            WEBHOOK_SECRET,
            1_699_999_999,
            unpaid_session,
        )));
        assert_error(
            parse_signed_webhook(Some(WEBHOOK_SECRET), &unpaid_headers, unpaid_session)
                .unwrap_err(),
            StatusCode::BAD_REQUEST,
            r#"{"error":"unsupported or non-success stripe event type"}"#,
        );

        // Non-success event types (e.g. payment_intent.payment_failed) are rejected 400 (PAY-01).
        let failed_event = br#"{"id":"evt_fail_1","type":"payment_intent.payment_failed","data":{"object":{"metadata":{"order_code":"ORD-99-1"},"amount":714000,"currency":"hkd"}}}"#;
        let fail_headers = headers_with(Some(&stripe_signature_header(
            WEBHOOK_SECRET,
            1_699_999_999,
            failed_event,
        )));
        assert_error(
            parse_signed_webhook(Some(WEBHOOK_SECRET), &fail_headers, failed_event).unwrap_err(),
            StatusCode::BAD_REQUEST,
            r#"{"error":"unsupported or non-success stripe event type"}"#,
        );
    }

    // -- state rules (PAY-01 / R2-5 / STOCK-01 / R2-11) -------------------------

    #[test]
    fn payment_transition_follows_pay01_and_r25() {
        // A live reservation is settled and consumes its units.
        assert_eq!(payment_transition("reserved"), PaymentTransition::MarkPaid);
        // A lapsed/cancelled reservation is flagged, never fulfilled.
        assert_eq!(
            payment_transition("expired"),
            PaymentTransition::LatePaymentReview
        );
        assert_eq!(
            payment_transition("cancelled"),
            PaymentTransition::LatePaymentReview
        );
        // Already settled states: record the event, change nothing.
        for status in [
            "paid",
            "fulfilled",
            "late_payment_review",
            "returned_quarantined",
        ] {
            assert_eq!(
                payment_transition(status),
                PaymentTransition::NoChange,
                "{status}"
            );
        }
    }

    #[test]
    fn fps_confirmation_transition_only_settles_a_live_reservation() {
        assert_eq!(
            fps_confirmation_transition("reserved"),
            FpsTransition::MarkPaid
        );
        assert_eq!(
            fps_confirmation_transition("expired"),
            FpsTransition::LatePaymentReview
        );
        for status in [
            "paid",
            "fulfilled",
            "cancelled",
            "late_payment_review",
            "returned_quarantined",
        ] {
            assert_eq!(
                fps_confirmation_transition(status),
                FpsTransition::Reject,
                "{status}"
            );
        }
    }

    #[test]
    fn release_transition_only_cancels_a_reservation() {
        assert_eq!(release_transition("reserved"), ReleaseTransition::Cancel);
        for status in [
            "paid",
            "fulfilled",
            "expired",
            "cancelled",
            "late_payment_review",
            "returned_quarantined",
        ] {
            assert_eq!(
                release_transition(status),
                ReleaseTransition::Reject,
                "{status}"
            );
        }
    }

    #[test]
    fn quarantine_transition_only_returns_sold_orders() {
        for status in ["paid", "fulfilled"] {
            assert_eq!(
                quarantine_transition(status),
                ReturnTransition::Quarantine,
                "{status}"
            );
        }
        for status in ["reserved", "expired", "cancelled", "late_payment_review"] {
            assert_eq!(
                quarantine_transition(status),
                ReturnTransition::Reject,
                "{status}"
            );
        }
    }

    #[test]
    fn webhook_response_serialises_the_idempotency_contract() {
        // PAY-01: the replay answer is exactly this shape — including the
        // email-metadata fields the web tier needs to avoid duplicate sends
        // and to know whom to tell (findings G02).
        let replay = WebhookResponse {
            ok: true,
            idempotent: true,
            status: "paid".to_string(),
            order_code: "ORD-1-2".to_string(),
            customer_name: "Ada Buyer".to_string(),
            customer_email: "ada@example.com".to_string(),
            locale: "zh-HK".to_string(),
            paid_email_sent: false,
        };
        assert_eq!(
            serde_json::to_string(&replay).unwrap(),
            r#"{"ok":true,"idempotent":true,"status":"paid","order_code":"ORD-1-2","customer_name":"Ada Buyer","customer_email":"ada@example.com","locale":"zh-HK","paid_email_sent":false}"#
        );
        let applied = WebhookResponse {
            ok: true,
            idempotent: false,
            status: "late_payment_review".to_string(),
            order_code: "ORD-3-4".to_string(),
            customer_name: "Chan Tai Man".to_string(),
            customer_email: "chan@example.com".to_string(),
            locale: "en".to_string(),
            paid_email_sent: true,
        };
        assert_eq!(
            serde_json::to_string(&applied).unwrap(),
            r#"{"ok":true,"idempotent":false,"status":"late_payment_review","order_code":"ORD-3-4","customer_name":"Chan Tai Man","customer_email":"chan@example.com","locale":"en","paid_email_sent":true}"#
        );
    }
}
