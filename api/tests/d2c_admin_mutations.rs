//! D2c step 4 — admin settings + editor DB-backed mutation suites against the
//! scratch Postgres harness (`common::ScratchDb`).
//!
//! What these tests pin down (the D2b unit suites deliberately deferred: the
//! presence-wins settings toggle round-trip at the DB/API layer, and the
//! editor CRUD paths at the DB layer — every contract asserted against real
//! Postgres with direct-SQL witnesses and EXACT response bodies):
//!
//! * Settings presence-wins round-trip (CONFIG-01 / SCOPE-01, founder toggle
//!   spec): `settings_round_trip_presence_wins_empty_override_and_delete_revert`
//!   proves snapshot-before -> override -> EMPTY override (the explicit OFF
//!   state: the row persists EMPTY and the endpoint serves the empty
//!   override) -> re-upsert on the SAME single row -> delete-revert-equals-
//!   snapshot. `settings_currency_round_trip_valid_shape_invalid_shape_and_no_off`
//!   does the same for the value-shape-validated `currency` key and pins the
//!   valid/invalid forms plus the fact that `currency` has NO OFF state.
//!   `settings_secret_looking_key_is_never_editable` /
//!   `settings_key_outside_the_whitelist_is_never_editable` /
//!   `settings_delete_without_override_is_404_without_audit` pin the write
//!   rejections (exact bodies, no row, no audit) and the delete-not-found
//!   contract.
//! * Product patch incl. empty family: create with `family=""` lands the row
//!   (the D2a write contract allows it), a partial update writes ONLY the
//!   provided fields (audit detail carries exactly those), the `active`
//!   toggle round-trips with audits, and an invalid field value is a 400 that
//!   leaves the row untouched.
//! * Product-content locale gating: ('en','colourway') + ('zh-HK','model_alt')
//!   upserts land rows + audits; a locale outside the configured pair ('fr'),
//!   a field outside the 0002 CHECK set, and an empty value are all 400s that
//!   land no rows and write no audits.
//! * Reference prices: a valid upsert lands the row + audit, `min > max` is
//!   rejected with the exact mapped body and no row, delete removes the row +
//!   audit, and delete-without-row is the handler's exact 404.
//! * Pieces: reassign moves the row + audit, reassign to a missing product is
//!   a 404 with no mutation, delete removes the row + audit (the duplicate
//!   piece_no conflict is already pinned by the step-3 suite).
//! * Site content: upsert/update/delete lifecycle (single row per (key,
//!   locale), no duplicates) + audits, delete-without-row is the handler's
//!   exact 404, and the ACTUAL contract for
//!   transport-unsafe keys ('.' / '..'): the API ACCEPTS free-form keys —
//!   `validate_site_content_key` only rejects empty/NUL keys — and web-side
//!   `isTransportSafeKey` owns transport safety (the D2b notes say the WEB
//!   tier rejects transport-unsafe keys; this test pins the API half honestly
//!   and does NOT invent an API rejection).
//!
//! Determinism rules (every assertion is ambient-env-independent):
//! * No absolute default values: the settings tests snapshot the served map
//!   BEFORE any write and assert every later state relative to that snapshot
//!   (snapshot -> override -> empty -> delete-revert-equals-snapshot).
//! * The locale-gating assertions depend on the configured locale pair being
//!   the code DEFAULT (en, zh-HK): `boot_mutations` refuses a set ambient
//!   `LOCALES` (name-only, never the value) before every test and pins the
//!   resolved pair — see `common::assert_no_locales_env`.
//! * The revalidation webhook knobs are refused the same way by
//!   `common::boot_admin` (the handlers fire the webhook after every COMMITTED
//!   mutation; a set knob would open real network connections).
//!
//! Argon2 budget: no test performs a live login (those are pinned by the
//! step-3 suite); every test mints its cookie through the same signer via
//! `common::minted_session_cookie`. Each test's boot still hashes one argon2
//! password (the bootstrap upsert) — that is the cost of the real boot path.
//!
//! Tests are plain `#[test]` fns driving the harness runtime through
//! `ScratchDb::block_on` — see `common` for why (`Drop` cannot block a
//! `#[tokio::test]` executor). The shared boot / router / cookie / request
//! machinery lives in `common` (extracted from `d2c_admin_auth.rs` in this
//! same step; that suite's semantics are unchanged).

mod common;

use axum::http::{Method, StatusCode};
use axum::Router;
use chrono::{DateTime, Utc};
use lamp_api::config;
use lamp_api::state::AppState;
use serde_json::{json, Value};

use common::{
    admin_request, admin_router, assert_no_locales_env, audit_rows, boot_admin, first_product_id,
    minted_session_cookie, pieces_snapshot, product_created_at, product_row, send, ProductRow,
    ScratchDb, ADMIN_USERNAME, MISSING_PRODUCT_ID, PRODUCT_NOT_FOUND_404,
};

// ---------------------------------------------------------------------------
// Exact bodies the handlers guarantee (asserted byte-for-byte: an exact string
// is checkable, so a fuzzy contains would only hide drift).
// ---------------------------------------------------------------------------

const KEY_NOT_EDITABLE_400: &str = r#"{"error":"settings key not admin-editable"}"#;
const CURRENCY_SHAPE_400: &str =
    r#"{"error":"currency must be three uppercase letters (ISO 4217)"}"#;
const SETTINGS_OVERRIDE_NOT_FOUND_404: &str = r#"{"error":"settings override not found"}"#;
const MODEL_EMPTY_400: &str = r#"{"error":"model cannot be empty"}"#;
const LOCALE_NOT_ALLOWED_400: &str = r#"{"error":"locale not in the configured locale set"}"#;
const FIELD_NOT_ALLOWED_400: &str =
    r#"{"error":"field must be one of model_alt, colourway, heritage, electrical"}"#;
const VALUE_EMPTY_400: &str = r#"{"error":"value cannot be empty"}"#;
const MIN_GT_MAX_400: &str = r#"{"error":"min_price_cents must be <= max_price_cents"}"#;
const REFERENCE_PRICE_NOT_FOUND_404: &str = r#"{"error":"reference price not found"}"#;
const SITE_CONTENT_NOT_FOUND_404: &str = r#"{"error":"site content row not found"}"#;
const OK_BODY: &str = r#"{"ok":true}"#;

// ---------------------------------------------------------------------------
// Probes (fixed literals so rows and audit details compare to EXACT values).
// ---------------------------------------------------------------------------

/// The optional whitelist key of the presence-wins round-trip: one of
/// `content::PUBLIC_SETTING_KEYS` that `validate_setting` allows to be written
/// EMPTY (the explicit OFF state — `brand.name`/`site.url` are required and
/// `currency` is shape-checked, so those cannot carry the OFF state).
const SETTINGS_OPTIONAL_KEY: &str = "contact.whatsapp";

/// The probe override value: never a real number.
const SETTINGS_PROBE_VALUE: &str = "+852 0000 0000";

/// `currency` probes: the valid ISO shape the handler accepts and the exact
/// invalid form it rejects.
const CURRENCY_VALID: &str = "USD";
const CURRENCY_INVALID: &str = "usd";

/// The probe product of the patch suite. Its `family` is EMPTY on purpose —
/// the literal `""` written inline by `probe_product_json` /
/// `probe_product_row` (no named family constant carries it): the D2a write
/// contract allows an empty family and the storefront catalogue parser
/// accepts it — the row must land with `family = ''`.
const PROBE_SLUG: &str = "d2c-mut-probe-pendant";
const PROBE_BRAND: &str = "Probe Brand";
const PROBE_MODEL: &str = "Probe Model";
const PROBE_PRICE_CENTS: i64 = 45_600;
const PROBE_ART_VARIANT: &str = "shades";
const PROBE_ART_HUE: i32 = 12;
const PROBE_ACTIVE: bool = true;

/// The reference-price probes (money stays integer cents).
const REF_MIN_CENTS: i64 = 600_000;
const REF_MAX_CENTS: i64 = 800_000;
const REF_SOURCE_NAME: &str = "Nordic Nest";
const REF_SOURCE_URL: &str = "https://www.nordicnest.com/probe";
const REF_CHECKED_ON: &str = "2026-02-28";

/// The site-content probe key: a free-form dotted key no seed row uses.
const SITE_KEY: &str = "d2c.probe.notice";

// ---------------------------------------------------------------------------
// Boot and request helpers
// ---------------------------------------------------------------------------

/// Boots the shared admin stack for one mutation test
/// (`common::boot_admin`), plus this suite's two determinism guards:
/// a set ambient `LOCALES` is refused (name-only) because the locale-gating
/// assertions depend on the default pair, and the resolved pair is pinned to
/// that default so a config change can never silently soften the 'fr'
/// rejection into a membership question.
fn boot_mutations(db: &ScratchDb) -> AppState {
    // Live wiring of the pure guard: READ-ONLY env access (mutating the
    // process env from parallel test threads would race), unset-or-empty both
    // counting as clean — mirroring `config::locales()`'s own fallback.
    assert_no_locales_env(|name| std::env::var(name).ok().filter(|v| !v.is_empty()));
    let state = boot_admin(db);
    assert_eq!(
        config::locales(),
        vec!["en".to_string(), "zh-HK".to_string()],
        "the locale-gating tests pin the DEFAULT configured pair; if this \
         fails the code default moved — update the tests deliberately, never \
         by relaxing the 'fr' rejection"
    );
    state
}

/// One cookie-authenticated request through the router on the harness
/// runtime, returning (status, body). The CSRF custom header rides along via
/// `common::admin_request`. Every mutation-suite request is authenticated:
/// authorization itself is pinned by the step-3 suite.
fn call(
    db: &ScratchDb,
    router: &Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let (status, _, body) =
        db.block_on(send(router, admin_request(method, uri, Some(cookie), body)));
    (status, body)
}

/// A one-entry settings map, exactly as `GET /api/settings` must serve an
/// override (a FLAT `{ key: value }` object).
fn settings_entry(key: &str, value: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(key.to_string(), Value::String(value.to_string()));
    Value::Object(map)
}

/// The `PUT /api/admin/settings/{key}` body.
fn setting_json(value: &str) -> String {
    json!({ "value": value }).to_string()
}

/// `GET /api/settings` parsed to the served map. The endpoint is public and
/// serves the whitelisted `settings` rows — the serving half of the
/// presence-wins contract these tests pin.
fn served_settings(db: &ScratchDb, router: &Router, cookie: &str) -> Value {
    let (status, body) = call(db, router, cookie, Method::GET, "/api/settings", "");
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/settings must serve the settings map"
    );
    serde_json::from_str(&body).expect("GET /api/settings must answer JSON")
}

/// The probe product body: every field fixed, `family` EMPTY.
fn probe_product_json() -> String {
    json!({
        "slug": PROBE_SLUG,
        "brand": PROBE_BRAND,
        "model": PROBE_MODEL,
        "price_cents": PROBE_PRICE_CENTS,
        "family": "",
        "art_variant": PROBE_ART_VARIANT,
        "art_hue": PROBE_ART_HUE,
        "active": PROBE_ACTIVE,
    })
    .to_string()
}

/// The exact `products` row the probe create must land (family EMPTY).
fn probe_product_row(id: i64) -> ProductRow {
    (
        id,
        PROBE_SLUG.to_string(),
        PROBE_BRAND.to_string(),
        PROBE_MODEL.to_string(),
        PROBE_PRICE_CENTS,
        String::new(),
        PROBE_ART_VARIANT.to_string(),
        PROBE_ART_HUE,
        PROBE_ACTIVE,
        0,
        0,
    )
}

/// The `audit_log.detail` the probe create must write: exactly the posted
/// fields, nothing else.
fn probe_create_audit_detail() -> Value {
    json!({
        "slug": PROBE_SLUG,
        "brand": PROBE_BRAND,
        "model": PROBE_MODEL,
        "price_cents": PROBE_PRICE_CENTS,
        "family": "",
        "art_variant": PROBE_ART_VARIANT,
        "art_hue": PROBE_ART_HUE,
        "active": PROBE_ACTIVE,
    })
}

/// Creates the probe product and returns the new id (the create itself is
/// pinned by `product_create_with_empty_family_lands_the_row`).
fn create_probe_product(db: &ScratchDb, router: &Router, cookie: &str) -> i64 {
    let (status, body) = call(
        db,
        router,
        cookie,
        Method::POST,
        "/api/admin/products",
        &probe_product_json(),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "the probe product create must be accepted"
    );
    let created: Value = serde_json::from_str(&body).expect("create response must be JSON");
    created["id"]
        .as_i64()
        .expect("create response must carry the numeric id")
}

/// Asserts the COMPLETE `PUT /api/admin/products/{id}` success body: exactly
/// the `AdminProduct` shape the handler returns from its `RETURNING` row —
/// every field at its exact expected value (patched fields at their new
/// value, the rest unchanged), nothing else. `created_at` is a clock value
/// (never comparable to a literal) and is clock-witnessed the way
/// `mutation_round_trip_writes_product_row_and_audit_row` does: the response
/// string must parse as RFC3339, agree with the row's Postgres timestamp
/// within 1s, and be recent — a defaulted or mangled timestamp passes none
/// of those checks.
fn assert_product_update_response(db: &ScratchDb, body: &str, expected: &ProductRow) {
    let (
        id,
        slug,
        brand,
        model,
        price_cents,
        family,
        art_variant,
        art_hue,
        active,
        stock_total,
        stock_reserved,
    ) = expected;
    let row: Value = serde_json::from_str(body).expect("the update response must be JSON");
    let created_at_raw = row["created_at"].as_str().unwrap_or_else(|| {
        panic!(
            "the update response must carry created_at as a string, got {}",
            row["created_at"]
        )
    });
    let created_at = DateTime::parse_from_rfc3339(created_at_raw)
        .unwrap_or_else(|e| {
            panic!(
                "the update response created_at must be RFC3339: {e}; raw value \
                 kept in this message on purpose: {created_at_raw:?}"
            )
        })
        .with_timezone(&Utc);
    assert_eq!(
        row,
        json!({
            "id": id,
            "slug": slug,
            "brand": brand,
            "model": model,
            "price_cents": price_cents,
            "family": family,
            "art_variant": art_variant,
            "art_hue": art_hue,
            "active": active,
            "stock_total": stock_total,
            "stock_reserved": stock_reserved,
            "created_at": row["created_at"],
        }),
        "the update response must be EXACTLY the full product row: every \
         patched field at its new value, every other field unchanged, no \
         extra fields"
    );
    let db_created_at = product_created_at(db, slug);
    let skew_ms = (created_at - db_created_at).num_milliseconds().abs();
    assert!(
        skew_ms <= 1_000,
        "the update response created_at and the Postgres row timestamp must \
         agree within 1s (sub-second precision differences allowed): response \
         {created_at_raw:?} ({created_at:?}) vs db {db_created_at:?} — |delta| = \
         {skew_ms}ms"
    );
    let now = Utc::now();
    for (label, timestamp) in [("response", created_at), ("db", db_created_at)] {
        let age_s = (now - timestamp).num_seconds().abs();
        assert!(
            age_s <= 300,
            "{label} created_at must be within the last few minutes of now \
             (sanity): {timestamp:?} is {age_s}s away from {now:?}"
        );
    }
}

/// Asserts the COMPLETE `PUT /api/admin/settings/{key}` success body: exactly
/// the `AdminSetting` shape — `{key, value, updated_at}` with the key and the
/// written value at their exact values, nothing else. `updated_at` is a clock
/// value (never comparable to a literal) and is clock-witnessed against the
/// `settings` row's Postgres timestamp: the response string must parse as
/// RFC3339 and agree with the row within 1s.
fn assert_setting_upsert_response(db: &ScratchDb, body: &str, key: &str, value: &str) {
    let row: Value = serde_json::from_str(body).expect("the upsert response must be JSON");
    let updated_at_raw = row["updated_at"].as_str().unwrap_or_else(|| {
        panic!(
            "the upsert response must carry updated_at as a string, got {}",
            row["updated_at"]
        )
    });
    let updated_at = DateTime::parse_from_rfc3339(updated_at_raw)
        .unwrap_or_else(|e| {
            panic!(
                "the upsert response updated_at must be RFC3339: {e}; raw value \
                 kept in this message on purpose: {updated_at_raw:?}"
            )
        })
        .with_timezone(&Utc);
    assert_eq!(
        row,
        json!({ "key": key, "value": value, "updated_at": row["updated_at"] }),
        "the upsert response must be EXACTLY {{key, value, updated_at}}: the \
         key and the written value, nothing else"
    );
    let db_updated_at = settings_updated_at(db, key);
    let skew_ms = (updated_at - db_updated_at).num_milliseconds().abs();
    assert!(
        skew_ms <= 1_000,
        "the upsert response updated_at and the Postgres row timestamp must \
         agree within 1s (sub-second precision differences allowed): response \
         {updated_at_raw:?} ({updated_at:?}) vs db {db_updated_at:?} — |delta| = \
         {skew_ms}ms"
    );
}

// ---------------------------------------------------------------------------
// Direct-SQL witnesses (the DB is what a mutation wrote, never the response)
// ---------------------------------------------------------------------------

/// One `settings` row by key: (key, value).
fn settings_row(db: &ScratchDb, key: &str) -> Option<(String, String)> {
    db.block_on(async {
        sqlx::query_as::<_, (String, String)>("SELECT key, value FROM settings WHERE key = $1")
            .bind(key)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("settings lookup failed for key {key}: {e}"))
    })
}

/// `SELECT COUNT(*)` over `settings` for one key (the no-duplicate witness).
fn settings_key_count(db: &ScratchDb, key: &str) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM settings WHERE key = $1")
            .bind(key)
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("settings count failed for key {key}: {e}"))
    })
}

/// The `settings.updated_at` clock value for one key: the clock witness the
/// upsert response's `updated_at` is cross-checked against (a clock value
/// never compares to a literal, but it must mirror the row's timestamp).
fn settings_updated_at(db: &ScratchDb, key: &str) -> DateTime<Utc> {
    db.block_on(async {
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT updated_at FROM settings WHERE key = $1")
            .bind(key)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("settings updated_at lookup failed for key {key}: {e}"))
            .expect("the settings row must exist to witness its updated_at")
    })
}

/// One `product_content` value by (product, locale, field).
fn product_content_value(
    db: &ScratchDb,
    product_id: i64,
    locale: &str,
    field: &str,
) -> Option<String> {
    db.block_on(async {
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM product_content \
             WHERE product_id = $1 AND locale = $2 AND field = $3",
        )
        .bind(product_id)
        .bind(locale)
        .bind(field)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| {
            panic!("product_content lookup failed for ({product_id},{locale},{field}): {e}")
        })
    })
}

/// `SELECT COUNT(*)` over `product_content` (the "no rows from rejections"
/// witness).
fn product_content_count(db: &ScratchDb) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM product_content")
            .fetch_one(db.pool())
            .await
            .expect("product_content count failed")
    })
}

/// One `reference_prices` row: (product_id, min, max, source_name,
/// source_url, checked_on) with `checked_on` as `YYYY-MM-DD` text — a literal
/// shape, unlike a clock value.
type ReferenceRow = (i64, i64, i64, String, String, String);

fn reference_price_row(db: &ScratchDb, product_id: i64) -> Option<ReferenceRow> {
    db.block_on(async {
        sqlx::query_as::<_, ReferenceRow>(
            "SELECT product_id, min_price_cents, max_price_cents, source_name, \
                 source_url, checked_on::text \
             FROM reference_prices WHERE product_id = $1",
        )
        .bind(product_id)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("reference_prices lookup failed for {product_id}: {e}"))
    })
}

/// A seeded product id WITHOUT a reference-price row (the seed gives the
/// PH 1/1 Piano Table none on purpose — TRUST-02 "verification pending").
fn product_id_without_reference_price(db: &ScratchDb) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM products \
             WHERE NOT EXISTS \
                 (SELECT 1 FROM reference_prices r WHERE r.product_id = products.id) \
             ORDER BY id LIMIT 1",
        )
        .fetch_one(db.pool())
        .await
        .expect("the seed must leave one product without a reference price")
    })
}

/// A seeded product id WITH a reference-price row (the seed labels 7 of 8).
fn product_id_with_reference_price(db: &ScratchDb) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>(
            "SELECT product_id FROM reference_prices ORDER BY product_id LIMIT 1",
        )
        .fetch_one(db.pool())
        .await
        .expect("the seed must carry reference prices")
    })
}

/// One piece's owning product id.
fn piece_product_id(db: &ScratchDb, piece_no: i32) -> Option<i64> {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT product_id FROM pieces WHERE piece_no = $1")
            .bind(piece_no)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("pieces lookup failed for piece_no {piece_no}: {e}"))
    })
}

/// Any seeded product id different from `not` (the reassign target).
fn some_other_product_id(db: &ScratchDb, not: i64) -> i64 {
    db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT id FROM products WHERE id <> $1 ORDER BY id LIMIT 1")
            .bind(not)
            .fetch_one(db.pool())
            .await
            .expect("the seed must carry more than one product")
    })
}

/// All `site_content` rows for one key: (locale, value) in locale order.
fn site_content_rows(db: &ScratchDb, key: &str) -> Vec<(String, String)> {
    db.block_on(async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT locale, value FROM site_content WHERE key = $1 ORDER BY locale",
        )
        .bind(key)
        .fetch_all(db.pool())
        .await
        .unwrap_or_else(|e| panic!("site_content lookup failed for key {key}: {e}"))
    })
}

// ---------------------------------------------------------------------------
// 1. Settings: presence-wins round-trip, rejections
// ---------------------------------------------------------------------------

/// The founder toggle spec at the DB/API layer, env-agnostic: snapshot the
/// served value BEFORE any write, then override -> EMPTY override (explicit
/// OFF: the row persists EMPTY and the endpoint serves the empty override —
/// its PRESENCE suppresses whatever default a consumer resolves) -> re-upsert
/// on the SAME single row -> DELETE reverts to EXACTLY the pre-write snapshot.
/// Every accepted write writes its audit row; nothing else may appear.
#[test]
fn settings_round_trip_presence_wins_empty_override_and_delete_revert() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_settings_round_trip");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    // DETERMINISM: the served state BEFORE any write is captured, never
    // asserted as an absolute — nothing here may depend on ambient defaults.
    let before = served_settings(&db, &router, &cookie);
    let key = SETTINGS_OPTIONAL_KEY;
    let uri = format!("/api/admin/settings/{key}");

    // (1) The override lands: the endpoint serves it and the ROW is exactly
    // (key, value).
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &uri,
        &setting_json(SETTINGS_PROBE_VALUE),
    );
    assert_eq!(status, StatusCode::OK, "upserting {key} must be accepted");
    assert_setting_upsert_response(&db, &body, key, SETTINGS_PROBE_VALUE);
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), SETTINGS_PROBE_VALUE.to_string())),
        "the settings table row must be exactly (key, the written value)"
    );
    assert_eq!(
        served_settings(&db, &router, &cookie),
        settings_entry(key, SETTINGS_PROBE_VALUE),
        "the settings endpoint must serve the override"
    );

    // (2) The empty override — the explicit OFF state (CONFIG-01
    // presence-wins): the row persists EMPTY and the endpoint serves the
    // EMPTY OVERRIDE (present and empty: the override suppresses the default
    // by existing, not by being absent).
    let (status, body) = call(&db, &router, &cookie, Method::PUT, &uri, &setting_json(""));
    assert_eq!(
        status,
        StatusCode::OK,
        "the empty override is the explicit OFF state for {key} and must be accepted"
    );
    assert_setting_upsert_response(&db, &body, key, "");
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), String::new())),
        "the row must persist EMPTY — an empty override is data, not absence"
    );
    assert_eq!(
        served_settings(&db, &router, &cookie),
        settings_entry(key, ""),
        "the endpoint must serve the empty override (present, empty)"
    );

    // (3) Re-upsert the valid value: the SAME single row is updated — never a
    // second row.
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &uri,
        &setting_json(SETTINGS_PROBE_VALUE),
    );
    assert_eq!(status, StatusCode::OK, "the re-upsert must be accepted");
    assert_setting_upsert_response(&db, &body, key, SETTINGS_PROBE_VALUE);
    assert_eq!(
        settings_key_count(&db, key),
        1,
        "re-upserting must update the SAME single row, never add a duplicate"
    );
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), SETTINGS_PROBE_VALUE.to_string())),
        "the re-upserted row must carry the written value"
    );

    // (4) Delete the override: the row is GONE (direct SQL) and the endpoint
    // serves EXACTLY the pre-write snapshot again.
    let (status, body) = call(&db, &router, &cookie, Method::DELETE, &uri, "");
    assert_eq!(status, StatusCode::OK, "deleting the override must succeed");
    assert_eq!(body, OK_BODY, "the delete must answer the ok body");
    assert_eq!(
        settings_row(&db, key),
        None,
        "the override row must be gone from the settings table"
    );
    assert_eq!(
        served_settings(&db, &router, &cookie),
        before,
        "after deleting the override the endpoint must serve EXACTLY the \
         pre-write state again (revert)"
    );

    // Every ACCEPTED write wrote exactly one audit row (actor/action/entity),
    // in the write transaction; the whole sequence, nothing else.
    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key, "value": SETTINGS_PROBE_VALUE }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key, "value": "" }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key, "value": SETTINGS_PROBE_VALUE }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "delete".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key }),
            ),
        ],
        "each accepted settings write must write exactly one audit row for \
         the acting admin (and rejected writes none)"
    );
}

/// The second company key with a value SHAPE (`currency`: `^[A-Z]{3}$`, the
/// only validated value form in `validate_setting`): snapshot -> the valid
/// form lands and serves -> the invalid form is rejected with the exact body
/// and leaves the row untouched -> `currency` has NO OFF state (empty is an
/// invalid shape, not a toggle) -> delete reverts to the snapshot.
#[test]
fn settings_currency_round_trip_valid_shape_invalid_shape_and_no_off() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_settings_currency");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let before = served_settings(&db, &router, &cookie);
    let key = "currency";
    let uri = format!("/api/admin/settings/{key}");

    // The VALID ISO form lands and serves.
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &uri,
        &setting_json(CURRENCY_VALID),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "the valid ISO form ({CURRENCY_VALID}) must be accepted"
    );
    assert_setting_upsert_response(&db, &body, key, CURRENCY_VALID);
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), CURRENCY_VALID.to_string())),
        "the currency row must be exactly (currency, the written code)"
    );
    assert_eq!(
        served_settings(&db, &router, &cookie),
        settings_entry(key, CURRENCY_VALID),
        "the settings endpoint must serve the written currency"
    );

    // The INVALID form is rejected with the exact body — and the row is
    // untouched, no audit row.
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &uri,
        &setting_json(CURRENCY_INVALID),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the invalid ISO form ({CURRENCY_INVALID}) must be rejected"
    );
    assert_eq!(
        body, CURRENCY_SHAPE_400,
        "the currency shape rejection must be the handler's exact body"
    );
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), CURRENCY_VALID.to_string())),
        "the rejected write must leave the row untouched"
    );

    // No OFF state for `currency`: the empty value is an invalid SHAPE (the
    // required no-toggle field cannot be switched off like the optional keys).
    let (status, body) = call(&db, &router, &cookie, Method::PUT, &uri, &setting_json(""));
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "currency has NO OFF state: an empty value must be rejected as an \
         invalid shape, never stored as the explicit OFF override"
    );
    assert_eq!(body, CURRENCY_SHAPE_400);
    assert_eq!(
        settings_row(&db, key),
        Some((key.to_string(), CURRENCY_VALID.to_string())),
        "the empty-value rejection must leave the row untouched"
    );

    // Delete reverts to the pre-write snapshot exactly.
    let (status, body) = call(&db, &router, &cookie, Method::DELETE, &uri, "");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, OK_BODY);
    assert_eq!(
        settings_row(&db, key),
        None,
        "the currency override row must be gone"
    );
    assert_eq!(
        served_settings(&db, &router, &cookie),
        before,
        "after deleting the override the endpoint must serve EXACTLY the \
         pre-write state again (revert)"
    );

    // Only the accepted writes audited.
    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key, "value": CURRENCY_VALID }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "delete".to_string(),
                "settings".to_string(),
                Some(key.to_string()),
                json!({ "key": key }),
            ),
        ],
        "the two accepted writes must audit; the rejected writes must not"
    );
}

/// A SECRET-LOOKING key is never admin-editable, in either direction (write
/// AND delete): the handler's exact 400 body — the SAME body a non-whitelisted
/// key gets, never confirming anything about the key — no row, no audit.
#[test]
fn settings_secret_looking_key_is_never_editable() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_settings_secret_key");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let key = "session.secret";
    let uri = format!("/api/admin/settings/{key}");
    for (method, body) in [
        (Method::PUT, setting_json("probe-value")),
        (Method::DELETE, String::new()),
    ] {
        let (status, response_body) = call(&db, &router, &cookie, method.clone(), &uri, &body);
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "a secret-looking key must be rejected on {method}"
        );
        assert_eq!(
            response_body, KEY_NOT_EDITABLE_400,
            "the rejection must be the handler's exact body (identical for \
             every non-editable key — never confirm a key exists)"
        );
    }
    assert_eq!(
        settings_row(&db, key),
        None,
        "the rejected writes must leave no settings row"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected writes must write no audit row"
    );
}

/// A key OUTSIDE the company write whitelist (SCOPE-01: the write whitelist
/// is `content::PUBLIC_SETTING_KEYS`, enforced on writes as well as reads):
/// the exact 400 body, no row, no audit.
#[test]
fn settings_key_outside_the_whitelist_is_never_editable() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_settings_whitelist");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let key = "api.url";
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/settings/{key}"),
        &setting_json("https://internal.example/"),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a key outside the whitelist must be rejected"
    );
    assert_eq!(
        body, KEY_NOT_EDITABLE_400,
        "the whitelist rejection must be the handler's exact body"
    );
    assert_eq!(
        settings_row(&db, key),
        None,
        "the rejected write must leave no settings row"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected write must write no audit row"
    );
}

/// The delete-not-found contract: deleting an override that does not exist is
/// a 404 with the handler's exact body (a "successful" delete of nothing would
/// fabricate an audit row for a mutation that never happened).
#[test]
fn settings_delete_without_override_is_404_without_audit() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_settings_delete_404");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    // `brand.name_native` IS whitelisted (so the key is accepted) but has no
    // override row on a fresh database.
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::DELETE,
        "/api/admin/settings/brand.name_native",
        "",
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, SETTINGS_OVERRIDE_NOT_FOUND_404);
    assert!(
        audit_rows(&db).is_empty(),
        "the 404 delete must write no audit row"
    );
}

// ---------------------------------------------------------------------------
// 2. Products: create with empty family, provided-fields-only updates
// ---------------------------------------------------------------------------

/// `family = ""` is a legal product value (the D2a write contract allows it;
/// the storefront catalogue parser accepts it): the create lands the row with
/// an EMPTY family and audits.
#[test]
fn product_create_with_empty_family_lands_the_row() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_product_empty_family");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::POST,
        "/api/admin/products",
        &probe_product_json(),
    );
    assert_eq!(status, StatusCode::OK);
    let created: Value = serde_json::from_str(&body).expect("create response must be JSON");
    let id = created["id"]
        .as_i64()
        .expect("create response carries the id");
    assert_eq!(
        created["family"],
        json!(""),
        "the create response must carry the EMPTY family as posted"
    );
    assert_eq!(
        product_row(&db, PROBE_SLUG),
        Some(probe_product_row(id)),
        "the products row must land with family = '' exactly as posted"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "create".to_string(),
            "products".to_string(),
            Some(id.to_string()),
            probe_create_audit_detail(),
        )],
        "the create must write exactly one audit row"
    );
}

/// Provided-fields-only update semantics (the handler is `update_product`,
/// mounted on PUT `/api/admin/products/{id}` with partial-update semantics):
/// patching `price_cents` alone changes NOTHING else in the row, and the
/// audit detail carries exactly the fields that changed.
#[test]
fn product_patch_updates_only_the_provided_fields() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_product_patch_price");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let id = create_probe_product(&db, &router, &cookie);
    let new_price = 900_000;
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/products/{id}"),
        &json!({ "price_cents": new_price }).to_string(),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "the price-only patch must be accepted"
    );
    let mut expected = probe_product_row(id);
    expected.4 = new_price;
    assert_product_update_response(&db, &body, &expected);
    assert_eq!(
        product_row(&db, PROBE_SLUG),
        Some(expected),
        "the patch must change ONLY price_cents — every other column \
         (including the empty family) must be byte-identical to the created row"
    );
    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "create".to_string(),
                "products".to_string(),
                Some(id.to_string()),
                probe_create_audit_detail(),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "update".to_string(),
                "products".to_string(),
                Some(id.to_string()),
                json!({ "price_cents": new_price }),
            ),
        ],
        "the update audit detail must carry exactly the provided fields"
    );
}

/// The `active` soft-visibility toggle round-trip: false lands `active=false`
/// with its audit, true lands `active=true` with its audit.
#[test]
fn product_patch_active_toggle_round_trips_with_audit() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_product_patch_active");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let id = create_probe_product(&db, &router, &cookie);
    for active in [false, true] {
        let (status, body) = call(
            &db,
            &router,
            &cookie,
            Method::PUT,
            &format!("/api/admin/products/{id}"),
            &json!({ "active": active }).to_string(),
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "the active={active} patch must be accepted"
        );
        let mut expected = probe_product_row(id);
        expected.8 = active;
        assert_product_update_response(&db, &body, &expected);
        assert_eq!(
            product_row(&db, PROBE_SLUG),
            Some(expected),
            "the row must carry active={active} after the patch"
        );
    }
    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "create".to_string(),
                "products".to_string(),
                Some(id.to_string()),
                probe_create_audit_detail(),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "update".to_string(),
                "products".to_string(),
                Some(id.to_string()),
                json!({ "active": false }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "update".to_string(),
                "products".to_string(),
                Some(id.to_string()),
                json!({ "active": true }),
            ),
        ],
        "each toggle must write exactly one audit row with exactly its field"
    );
}

/// An invalid field value in a patch (empty `model` — `validate_product_patch`
/// rejects it with a 400) must leave the row byte-identical and write no
/// audit row.
#[test]
fn product_patch_invalid_model_is_400_and_leaves_the_row_untouched() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_product_patch_invalid");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let id = create_probe_product(&db, &router, &cookie);
    let row_before = product_row(&db, PROBE_SLUG).expect("the probe row must exist");
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/products/{id}"),
        &json!({ "model": "" }).to_string(),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an empty model must be rejected"
    );
    assert_eq!(
        body, MODEL_EMPTY_400,
        "the model rejection must be the handler's exact body"
    );
    assert_eq!(
        product_row(&db, PROBE_SLUG),
        Some(row_before),
        "the rejected patch must leave every column untouched"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "create".to_string(),
            "products".to_string(),
            Some(id.to_string()),
            probe_create_audit_detail(),
        )],
        "the rejected patch must write no audit row (only the create did)"
    );
}

// ---------------------------------------------------------------------------
// 3. Product content: locale gating and the value contract
// ---------------------------------------------------------------------------

/// ('en','colourway') and ('zh-HK','model_alt') upserts for a seeded product
/// land their rows (upsert updates in place — the two specific rows
/// exercised here, ('en','colourway') and ('zh-HK','model_alt') for the
/// first seeded product, exist in the seed) and write their audits.
#[test]
fn product_content_upserts_land_rows_and_audits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_content_upsert");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = first_product_id(&db);
    let writes = [
        ("en", "colourway", "Probe White"),
        ("zh-HK", "model_alt", "Probe Model (zh-HK)"),
    ];
    for (locale, field, value) in writes {
        let posted = json!({
            "product_id": product_id,
            "locale": locale,
            "field": field,
            "value": value,
        })
        .to_string();
        let (status, body) = call(
            &db,
            &router,
            &cookie,
            Method::PUT,
            "/api/admin/product-content",
            &posted,
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "the ({locale},{field}) upsert must be accepted"
        );
        let echoed: Value = serde_json::from_str(&body).expect("upsert response must be JSON");
        assert_eq!(
            echoed,
            serde_json::from_str::<Value>(&posted).unwrap(),
            "the upsert response must echo the written row"
        );
    }
    for (locale, field, value) in writes {
        assert_eq!(
            product_content_value(&db, product_id, locale, field),
            Some(value.to_string()),
            "the ({locale},{field}) row must carry the written value"
        );
    }
    assert_eq!(
        audit_rows(&db),
        writes
            .iter()
            .map(|(locale, field, value)| (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "product_content".to_string(),
                Some(format!("{product_id}/{locale}/{field}")),
                json!({
                    "product_id": product_id,
                    "locale": locale,
                    "field": field,
                    "value": value,
                }),
            ))
            .collect::<Vec<_>>(),
        "each upsert must write exactly one audit row (entity_id = product/locale/field)"
    );
}

/// A locale OUTSIDE the configured pair ('fr' is not in the code-default pair
/// en, zh-HK) is rejected with the exact body and lands NO row — pinned
/// deterministic by [`boot_mutations`], which refuses an ambient `LOCALES`.
#[test]
fn product_content_locale_outside_the_pair_is_400_without_rows() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_content_locale_gate");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = first_product_id(&db);
    let count_before = product_content_count(&db);
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/product-content",
        &json!({
            "product_id": product_id,
            "locale": "fr",
            "field": "colourway",
            "value": "Blanc",
        })
        .to_string(),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a locale outside the configured pair must be rejected"
    );
    assert_eq!(
        body, LOCALE_NOT_ALLOWED_400,
        "the locale rejection must be the handler's exact body"
    );
    assert_eq!(
        product_content_value(&db, product_id, "fr", "colourway"),
        None,
        "the rejected upsert must land no row"
    );
    assert_eq!(
        product_content_count(&db),
        count_before,
        "the rejected upsert must not touch product_content"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected upsert must write no audit row"
    );
}

/// A field OUTSIDE the 0002 CHECK set is rejected with the exact body before
/// the DB (the handler validates the same set the CHECK enforces, so the
/// CHECK is defense in depth — pinned here as: no row lands).
#[test]
fn product_content_field_outside_the_check_set_is_400_without_rows() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_content_field_gate");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = first_product_id(&db);
    let count_before = product_content_count(&db);
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/product-content",
        &json!({
            "product_id": product_id,
            "locale": "en",
            "field": "price",
            "value": "42",
        })
        .to_string(),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, FIELD_NOT_ALLOWED_400);
    assert_eq!(
        product_content_value(&db, product_id, "en", "price"),
        None,
        "the rejected upsert must land no row"
    );
    assert_eq!(product_content_count(&db), count_before);
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected upsert must write no audit row"
    );
}

/// An empty value is rejected (`value cannot be empty`) and the existing row
/// keeps its value — pinned relative to a snapshot of the row, never a seed
/// constant.
#[test]
fn product_content_empty_value_is_400_without_rows() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_content_empty_value");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = first_product_id(&db);
    let value_before =
        product_content_value(&db, product_id, "en", "colourway").expect("the seed has this row");
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/product-content",
        &json!({
            "product_id": product_id,
            "locale": "en",
            "field": "colourway",
            "value": "",
        })
        .to_string(),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, VALUE_EMPTY_400);
    assert_eq!(
        product_content_value(&db, product_id, "en", "colourway"),
        Some(value_before),
        "the rejected upsert must leave the existing row untouched"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected upsert must write no audit row"
    );
}

// ---------------------------------------------------------------------------
// 4. Reference prices: validation, delete, not-found
// ---------------------------------------------------------------------------

/// A valid verified range (min <= max, a citable source, a real checked date)
/// lands the single row per product and writes its audit.
#[test]
fn reference_price_upsert_lands_row_and_audit() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_ref_upsert");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    // The seed gives exactly this product no reference row (TRUST-02: no
    // verified reference exists -> verification pending).
    let product_id = product_id_without_reference_price(&db);
    let posted = json!({
        "min_price_cents": REF_MIN_CENTS,
        "max_price_cents": REF_MAX_CENTS,
        "source_name": REF_SOURCE_NAME,
        "source_url": REF_SOURCE_URL,
        "checked_on": REF_CHECKED_ON,
    })
    .to_string();
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/reference-prices/{product_id}"),
        &posted,
    );
    assert_eq!(status, StatusCode::OK);
    let echoed: Value = serde_json::from_str(&body).expect("upsert response must be JSON");
    assert_eq!(
        echoed,
        json!({
            "product_id": product_id,
            "min_price_cents": REF_MIN_CENTS,
            "max_price_cents": REF_MAX_CENTS,
            "source_name": REF_SOURCE_NAME,
            "source_url": REF_SOURCE_URL,
            "checked_on": REF_CHECKED_ON,
        }),
        "the upsert response must be EXACTLY the reference row: the product \
         id, the min/max cents, the source name and URL, and checked_on \
         serialized as YYYY-MM-DD — nothing else"
    );
    assert_eq!(
        reference_price_row(&db, product_id),
        Some((
            product_id,
            REF_MIN_CENTS,
            REF_MAX_CENTS,
            REF_SOURCE_NAME.to_string(),
            REF_SOURCE_URL.to_string(),
            REF_CHECKED_ON.to_string(),
        )),
        "the reference_prices row must carry exactly the posted values"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "upsert".to_string(),
            "reference_prices".to_string(),
            Some(product_id.to_string()),
            json!({
                "product_id": product_id,
                "min_price_cents": REF_MIN_CENTS,
                "max_price_cents": REF_MAX_CENTS,
                "source_name": REF_SOURCE_NAME,
                "source_url": REF_SOURCE_URL,
                "checked_on": REF_CHECKED_ON,
            }),
        )],
        "the upsert must write exactly one audit row"
    );
}

/// `min > max` is rejected with the exact mapped body and lands NO row. Note
/// the actual contract (differs from the sketch's "DB CHECK mapping"): the
/// handler's SERVER-SIDE application validator `validate_reference_price`
/// rejects the range BEFORE any DB work, so the 0002
/// `min_price_cents <= max_price_cents` CHECK is defense in depth and never
/// fires for this input — the 400 is the validator's body, not a mapped
/// constraint violation. The bin unit tests pin the pure validator; this
/// suite pins the handler's DB-layer effect.
#[test]
fn reference_price_min_above_max_is_400_without_row() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_ref_min_gt_max");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = product_id_without_reference_price(&db);
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/reference-prices/{product_id}"),
        &json!({
            "min_price_cents": REF_MAX_CENTS,
            "max_price_cents": REF_MIN_CENTS,
            "source_name": REF_SOURCE_NAME,
            "source_url": REF_SOURCE_URL,
            "checked_on": REF_CHECKED_ON,
        })
        .to_string(),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "min > max must be rejected before any DB work"
    );
    assert_eq!(body, MIN_GT_MAX_400);
    assert_eq!(
        reference_price_row(&db, product_id),
        None,
        "the rejected upsert must land no row"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected upsert must write no audit row"
    );
}

/// Deleting a product's reference price removes the row (direct SQL) and
/// writes the delete audit (= verification pending again).
#[test]
fn reference_price_delete_removes_the_row_and_audits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_ref_delete");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let product_id = product_id_with_reference_price(&db);
    assert!(
        reference_price_row(&db, product_id).is_some(),
        "the seed must give this product a reference row to delete"
    );
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::DELETE,
        &format!("/api/admin/reference-prices/{product_id}"),
        "",
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, OK_BODY);
    assert_eq!(
        reference_price_row(&db, product_id),
        None,
        "the reference row must be gone"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "delete".to_string(),
            "reference_prices".to_string(),
            Some(product_id.to_string()),
            json!({ "product_id": product_id }),
        )],
        "the delete must write exactly one audit row"
    );
}

/// Delete with no row to delete is the handler's exact 404 and writes no
/// audit. The ACTUAL contract (differs from the sketch's "on a product with
/// no row"): `delete_reference_price` never checks product existence at all —
/// it answers the SAME 404 for a real product without a row AND for a product
/// id that cannot exist. Both halves pinned here.
#[test]
fn reference_price_delete_without_row_is_404_without_audit() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_ref_delete_404");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    for product_id in [product_id_without_reference_price(&db), MISSING_PRODUCT_ID] {
        let (status, body) = call(
            &db,
            &router,
            &cookie,
            Method::DELETE,
            &format!("/api/admin/reference-prices/{product_id}"),
            "",
        );
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "deleting a non-existent reference row must be 404 (product {product_id})"
        );
        assert_eq!(body, REFERENCE_PRICE_NOT_FOUND_404);
    }
    assert!(
        audit_rows(&db).is_empty(),
        "both 404 deletes must write no audit row"
    );
}

// ---------------------------------------------------------------------------
// 5. Pieces: reassign and delete
// ---------------------------------------------------------------------------

/// Reassigning a seeded piece to a different seeded product moves the row
/// (direct SQL) and writes the update audit.
#[test]
fn reassign_piece_moves_the_row_and_audits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_piece_reassign");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let piece_no = 1;
    let before = piece_product_id(&db, piece_no).expect("the seed numbers pieces 1-16 (DROP-01)");
    let target = some_other_product_id(&db, before);
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PATCH,
        &format!("/api/admin/pieces/{piece_no}"),
        &json!({ "product_id": target }).to_string(),
    );
    assert_eq!(status, StatusCode::OK, "the reassign must be accepted");
    let echoed: Value = serde_json::from_str(&body).expect("reassign response must be JSON");
    assert_eq!(
        echoed,
        json!({ "piece_no": piece_no, "product_id": target }),
        "the reassign response must echo the piece row"
    );
    assert_eq!(
        piece_product_id(&db, piece_no),
        Some(target),
        "the piece row must now point at the new product"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "update".to_string(),
            "pieces".to_string(),
            Some(piece_no.to_string()),
            json!({ "piece_no": piece_no, "product_id": target }),
        )],
        "the reassign must write exactly one audit row"
    );
}

/// Reassigning to a MISSING product is the handler's exact 404 (checked
/// inside the transaction before the UPDATE) with no mutation at all.
#[test]
fn reassign_piece_to_missing_product_is_404_without_mutation() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_piece_reassign_404");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let piece_no = 1;
    let before = piece_product_id(&db, piece_no).expect("the seed numbers pieces 1-16 (DROP-01)");
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PATCH,
        &format!("/api/admin/pieces/{piece_no}"),
        &json!({ "product_id": MISSING_PRODUCT_ID }).to_string(),
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, PRODUCT_NOT_FOUND_404);
    assert_eq!(
        piece_product_id(&db, piece_no),
        Some(before),
        "the rejected reassign must leave the piece row untouched"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "the rejected reassign must write no audit row"
    );
}

/// Deleting a piece removes the numbered row (direct SQL) and writes the
/// delete audit.
#[test]
fn delete_piece_removes_the_row_and_audits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_piece_delete");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let piece_no = 16;
    assert!(
        piece_product_id(&db, piece_no).is_some(),
        "the seed numbers pieces 1-16 (DROP-01)"
    );
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::DELETE,
        &format!("/api/admin/pieces/{piece_no}"),
        "",
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, OK_BODY);
    assert_eq!(
        piece_product_id(&db, piece_no),
        None,
        "the piece row must be gone"
    );
    assert_eq!(
        pieces_snapshot(&db).len(),
        15,
        "exactly one of the 16 seeded pieces (DROP-01) must be removed"
    );
    assert_eq!(
        audit_rows(&db),
        vec![(
            ADMIN_USERNAME.to_string(),
            "delete".to_string(),
            "pieces".to_string(),
            Some(piece_no.to_string()),
            json!({ "piece_no": piece_no }),
        )],
        "the delete must write exactly one audit row"
    );
}

// ---------------------------------------------------------------------------
// 6. Site content: lifecycle and the transport-unsafe-key contract
// ---------------------------------------------------------------------------

/// The site-content row lifecycle: a new (key, locale, value) lands + audits,
/// re-upserting the same (key, locale) UPDATES the single row (no duplicate)
/// + audits, the delete removes the row + audits.
#[test]
fn site_content_upsert_update_delete_lifecycle_with_audits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_site_content_lifecycle");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let locale = "en";
    let entity_id = format!("{SITE_KEY}/{locale}");
    for value in ["First copy", "Second copy"] {
        let (status, body) = call(
            &db,
            &router,
            &cookie,
            Method::PUT,
            "/api/admin/site-content",
            &json!({ "key": SITE_KEY, "locale": locale, "value": value }).to_string(),
        );
        assert_eq!(status, StatusCode::OK, "the upsert of {value:?} must land");
        let echoed: Value = serde_json::from_str(&body).expect("upsert response must be JSON");
        assert_eq!(
            echoed,
            json!({ "key": SITE_KEY, "locale": locale, "value": value }),
            "the upsert response must echo the written row"
        );
        assert_eq!(
            site_content_rows(&db, SITE_KEY),
            vec![(locale.to_string(), value.to_string())],
            "the (key, locale) row must be a SINGLE updated row — never a duplicate"
        );
    }

    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::DELETE,
        &format!("/api/admin/site-content/{SITE_KEY}/{locale}"),
        "",
    );
    assert_eq!(status, StatusCode::OK, "the delete must be accepted");
    assert_eq!(body, OK_BODY);
    assert!(
        site_content_rows(&db, SITE_KEY).is_empty(),
        "the row must be gone after the delete"
    );

    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "site_content".to_string(),
                Some(entity_id.clone()),
                json!({ "key": SITE_KEY, "locale": locale, "value": "First copy" }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "site_content".to_string(),
                Some(entity_id.clone()),
                json!({ "key": SITE_KEY, "locale": locale, "value": "Second copy" }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "delete".to_string(),
                "site_content".to_string(),
                Some(entity_id.clone()),
                json!({ "key": SITE_KEY, "locale": locale }),
            ),
        ],
        "each accepted write must write exactly one audit row"
    );
}

/// Delete with no row to delete is the handler's exact 404 and writes no
/// audit — the shared "a delete of nothing is never a success" contract.
#[test]
fn site_content_delete_without_row_is_404_without_audit() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_site_content_delete_404");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::DELETE,
        &format!("/api/admin/site-content/{SITE_KEY}/en"),
        "",
    );
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, SITE_CONTENT_NOT_FOUND_404);
    assert!(
        audit_rows(&db).is_empty(),
        "the 404 delete must write no audit row"
    );
}

/// The ACTUAL transport-unsafe-key contract of the API: `.` and `..` are
/// ACCEPTED as site-content keys. `validate_site_content_key` only rejects
/// empty and NUL-bearing keys — transport safety is the WEB tier's
/// `isTransportSafeKey` (the D2b notes put the rejection there), so this test
/// pins the API's acceptance honestly and does NOT invent an API rejection.
/// The delete path segments carry the key verbatim and route normally.
#[test]
fn site_content_transport_unsafe_keys_the_actual_api_contract() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "mut_site_content_unsafe_key");
    db.apply_migrations();
    let state = boot_mutations(&db);
    let cookie = minted_session_cookie(&state);
    let router = admin_router(state);

    let locale = "en";
    for key in [".", ".."] {
        let (status, _) = call(
            &db,
            &router,
            &cookie,
            Method::PUT,
            "/api/admin/site-content",
            &json!({ "key": key, "locale": locale, "value": "probe" }).to_string(),
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "the API ACCEPTS the transport-unsafe key {key:?} (transport \
             safety is web-side isTransportSafeKey's job) — if this fails the \
             API contract changed, update this pin deliberately"
        );
        assert_eq!(
            site_content_rows(&db, key),
            vec![(locale.to_string(), "probe".to_string())],
            "the {key:?} row must land like any other key"
        );

        let (status, body) = call(
            &db,
            &router,
            &cookie,
            Method::DELETE,
            &format!("/api/admin/site-content/{key}/{locale}"),
            "",
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "the delete route must route the {key:?} key segment verbatim"
        );
        assert_eq!(body, OK_BODY);
        assert!(
            site_content_rows(&db, key).is_empty(),
            "the {key:?} row must be gone after the delete"
        );
    }
    assert_eq!(
        audit_rows(&db),
        vec![
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "site_content".to_string(),
                Some(format!("./{locale}")),
                json!({ "key": ".", "locale": locale, "value": "probe" }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "delete".to_string(),
                "site_content".to_string(),
                Some(format!("./{locale}")),
                json!({ "key": ".", "locale": locale }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "upsert".to_string(),
                "site_content".to_string(),
                Some(format!("../{locale}")),
                json!({ "key": "..", "locale": locale, "value": "probe" }),
            ),
            (
                ADMIN_USERNAME.to_string(),
                "delete".to_string(),
                "site_content".to_string(),
                Some(format!("../{locale}")),
                json!({ "key": "..", "locale": locale }),
            ),
        ],
        "both accepted upserts and both accepted deletes must audit, exactly \
         one row each in write order (upsert then delete per key, the loop's \
         actual sequence), nothing else"
    );
}

// ---------------------------------------------------------------------------
// 7. Network-free ambient-LOCALES policy regressions (mock lookup, no env)
// ---------------------------------------------------------------------------

/// NETWORK-FREE ambient-env policy regression: [`common::assert_no_locales_env`]
/// is pure over its lookup, so the tests drive a MOCK environment — mutating
/// the process env from a test would race the parallel test threads, and
/// libtest captures each test's panic, so `should_panic` on a mock is the
/// honest way to pin the refusal. A set `LOCALES` must refuse the run; the
/// live wiring (one std::env closure in [`boot_mutations`]) is exercised by
/// the architect's live gate, not by these.
#[test]
#[should_panic(expected = "LOCALES")]
fn locales_env_set_refuses_the_run() {
    assert_no_locales_env(|name| (name == "LOCALES").then(|| "fr,de".to_string()));
}

/// The refusal must name the VARIABLE and never echo the VALUE (same
/// secret-hygiene rule as the revalidation knobs: ambient configuration must
/// never reach test output). `should_panic` only checks a substring, so this
/// drives the pure fn under `catch_unwind` and inspects the FULL panic
/// payload for both properties.
#[test]
fn locales_refusal_names_variable_but_never_value() {
    const FAKE_VALUE: &str = "fr,de";
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_no_locales_env(|name| (name == "LOCALES").then(|| FAKE_VALUE.to_string()));
    }));
    let payload = outcome.expect_err("a set LOCALES must panic");
    let message = if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else {
        panic!("the panic payload must carry a string message");
    };
    assert!(
        message.contains("LOCALES"),
        "the refusal must name the variable to unset: {message}"
    );
    assert!(
        !message.contains(FAKE_VALUE),
        "the refusal must never echo the value: {message}"
    );
}

/// The clean half: a mock lookup that finds no `LOCALES` passes — the live
/// wiring counts empty values as unset (mirroring `config::locales()`, which
/// falls back to the default pair for empty/unparseable values).
#[test]
fn locales_env_clean_passes() {
    assert_no_locales_env(|_| None);
}
