//! D2c R1 — the flagship full-product admin endpoints against the scratch
//! Postgres harness (`common::ScratchDb`).
//!
//! What these tests pin down (the handlers' DB-backed behavior; the pure
//! validation rules are unit-pinned in `api/src/admin_crud.rs`):
//!
//! * `get_product_full_returns_the_complete_aggregate`: `GET
//!   /api/admin/products/{id}` answers ONE aggregate (product row incl. the
//!   0005 media/spec columns and `stock_quarantined`, per-locale content,
//!   reference price, piece numbers) for an ACTIVE SKU, carries
//!   `Cache-Control: no-store`, and keeps serving an INACTIVE SKU (the admin
//!   view never hides a row).
//! * `get_product_full_is_404_for_a_missing_id`: the exact `product not
//!   found` body, never an empty aggregate.
//! * `update_product_full_saves_fields_content_and_reference_together`:
//!   `PUT /api/admin/products/{id}/full` writes product fields, upserts
//!   content rows and upserts the reference price in ONE save, answers with
//!   the read-after-write aggregate and writes exactly ONE `audit_log` row
//!   (`update_full` / `products` / the id).
//! * `update_product_full_deletes_the_reference_price_when_absent`: both
//!   `null` and a MISSING `reference_price` key remove the row (TRUST-02:
//!   back to "verification pending"), never leave a stale reference behind.
//! * `update_product_full_enforces_the_stock_invariant_and_validates_first`:
//!   `stock_total >= stock_reserved + stock_quarantined` is rejected with a
//!   400 BEFORE anything is written (row untouched, no audit row), and a
//!   missing product is a clean 404.
//!
//! Every test gates on `common::scratch_db_url()` (loud skip without the
//! scratch server) and drives the harness runtime through
//! `ScratchDb::block_on` (see `common` for why tests stay plain `#[test]`).

mod common;

use axum::http::{Method, StatusCode};
use common::{
    admin_request, admin_router, audit_rows, boot_admin, minted_session_cookie, send, ScratchDb,
    MISSING_PRODUCT_ID, PRODUCT_NOT_FOUND_404,
};
use serde_json::{json, Value};

/// The seeded SKU these tests read and edit (0003; specs/media from 0005).
const SLUG: &str = "panthella-160-v3-beige";

/// The exact 400 body of a rejected stock invariant (the handler's contract).
const STOCK_INVARIANT_400: &str =
    r#"{"error":"stock_total must be >= stock_reserved + stock_quarantined"}"#;

/// The exact 400 body of a rejected `art_variant` (the handler's contract).
const ART_VARIANT_400: &str = r#"{"error":"art_variant must be one of \"dome\", \"shades\", \"shades-tall\", \"piano\", \"none\""}"#;

/// Drives one admin request on the harness runtime (cookie + CSRF header
/// included by `admin_request`), returning (status, body).
fn call(
    db: &ScratchDb,
    router: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: &str,
) -> (StatusCode, String) {
    let (status, _headers, body) =
        db.block_on(send(router, admin_request(method, uri, Some(cookie), body)));
    (status, body)
}

/// The `products` row as JSON (`row_to_json`), read with direct SQL: the DB
/// is the witness for what a mutation wrote, never the handler's response.
fn product_json(db: &ScratchDb, slug: &str) -> Value {
    db.block_on(async {
        sqlx::query_scalar::<_, Value>("SELECT row_to_json(products) FROM products WHERE slug = $1")
            .bind(slug)
            .fetch_optional(db.pool())
            .await
            .unwrap_or_else(|e| panic!("products lookup failed for slug {slug}: {e}"))
            .unwrap_or_else(|| panic!("product row for slug {slug} must exist"))
    })
}

/// One `product_content` value by (slug, locale, field), or `None` when the
/// row is absent (direct SQL).
fn content_value(db: &ScratchDb, slug: &str, locale: &str, field: &str) -> Option<String> {
    db.block_on(async {
        sqlx::query_scalar::<_, String>(
            "SELECT c.value FROM product_content c \
             JOIN products p ON p.id = c.product_id \
             WHERE p.slug = $1 AND c.locale = $2 AND c.field = $3",
        )
        .bind(slug)
        .bind(locale)
        .bind(field)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("product_content lookup failed: {e}"))
    })
}

/// The `reference_prices` row for a slug as JSON, or `None` (direct SQL).
fn reference_json(db: &ScratchDb, slug: &str) -> Option<Value> {
    db.block_on(async {
        sqlx::query_scalar::<_, Value>(
            "SELECT row_to_json(r) FROM reference_prices r \
             JOIN products p ON p.id = r.product_id WHERE p.slug = $1",
        )
        .bind(slug)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("reference_prices lookup failed: {e}"))
    })
}

/// A complete `PUT /api/admin/products/{id}/full` body with the given
/// headline values (the flagship editor's one form).
fn full_body(model: &str, price_cents: i64, reference_price: Value) -> String {
    json!({
        "slug": SLUG,
        "brand": "Louis Poulsen",
        "model": model,
        "price_cents": price_cents,
        "family": "panthella-160-portable",
        "art_variant": "piano",
        "art_hue": 35,
        "active": true,
        "stock_total": 2,
        "stock_quarantined": 1,
        "image_day_url": "https://cdn.example.com/new-day.png",
        "image_lit_url": "https://cdn.example.com/new-lit.webp",
        "designer": "Verner Panton",
        "design_year": "1971",
        "dimensions": "Ø 160 × H 238 mm",
        "light_source": "2700K Integrated LED · 2.5W",
        "content": {
            "en": { "colourway": "Beige Prime" },
            "zh-HK": { "heritage": "新版傳承文字。" }
        },
        "reference_price": reference_price,
    })
    .to_string()
}

/// A verified reference-price input (the `Some` upsert case).
fn reference_input() -> Value {
    json!({
        "min_price_cents": 100000,
        "max_price_cents": 200000,
        "source_name": "Test Shop",
        "source_url": "https://shop.example.com/x",
        "checked_on": "2026-02-28",
    })
}

#[test]
fn get_product_full_returns_the_complete_aggregate() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "product_full_get");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());
    let cookie = minted_session_cookie(&state);

    let (status, headers, body) = db.block_on(send(
        &router,
        admin_request(Method::GET, "/api/admin/products/1", Some(&cookie), ""),
    ));
    assert_eq!(status, StatusCode::OK, "GET the full aggregate: {body}");
    // Admin data (stock included) is never cacheable — same rule as the list.
    assert_eq!(
        headers.get("cache-control").map(|v| v.to_str().unwrap()),
        Some("no-store"),
        "the full aggregate must carry Cache-Control: no-store"
    );
    let aggregate: Value = serde_json::from_str(&body).expect("aggregate must be JSON");
    assert_eq!(aggregate["id"], 1);
    assert_eq!(aggregate["slug"], SLUG);
    assert_eq!(aggregate["active"], true);
    // The 0005 media/spec columns and the quarantine count travel verbatim.
    assert_eq!(
        aggregate["image_day_url"],
        "/demo-assets/panthella-160-v3-beige.png"
    );
    assert_eq!(aggregate["designer"], "Verner Panton");
    assert_eq!(aggregate["design_year"], "1971");
    assert_eq!(aggregate["dimensions"], "Ø 160 × H 238 mm");
    assert!(aggregate["light_source"]
        .as_str()
        .unwrap()
        .contains("2700K"));
    assert_eq!(aggregate["stock_total"], 2);
    assert_eq!(aggregate["stock_reserved"], 0);
    assert_eq!(aggregate["stock_quarantined"], 0);
    // Content is the stored rows, keyed locale -> field -> value.
    assert_eq!(
        aggregate["content"]["en"]["colourway"], "Beige",
        "the seeded colourway must come back exactly as stored"
    );
    assert!(aggregate["content"]["zh-HK"]["heritage"].is_string());
    // TRUST-02: the seeded Panthella carries a verified reference (0003).
    assert_eq!(
        aggregate["reference_price"]["source_name"],
        "MyConcept Hong Kong"
    );
    // DROP-01: this SKU holds pieces 1 and 2 of the numbered drop.
    assert_eq!(aggregate["pieces"], json!([1, 2]));

    // An INACTIVE SKU stays readable in the admin view (never hidden). The
    // toggle goes through the LEGACY partial update on purpose: the R1 full
    // save and the D2a patch endpoint coexist on purpose.
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1",
        r#"{"active":false}"#,
    );
    assert_eq!(status, StatusCode::OK, "deactivate the SKU: {body}");
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::GET,
        "/api/admin/products/1",
        "",
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "inactive SKU stays readable: {body}"
    );
    let aggregate: Value = serde_json::from_str(&body).expect("aggregate must be JSON");
    assert_eq!(aggregate["active"], false);
    assert_eq!(aggregate["slug"], SLUG);
}

#[test]
fn get_product_full_is_404_for_a_missing_id() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "product_full_get_404");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());
    let cookie = minted_session_cookie(&state);

    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::GET,
        &format!("/api/admin/products/{MISSING_PRODUCT_ID}"),
        "",
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "missing id: {body}");
    assert_eq!(body, PRODUCT_NOT_FOUND_404, "the 404 body is exact");
}

#[test]
fn update_product_full_saves_fields_content_and_reference_together() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "product_full_save");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());
    let cookie = minted_session_cookie(&state);

    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &full_body(
            "Panthella 160 V3 Portable (Edited)",
            160_000,
            reference_input(),
        ),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "the full save must be accepted: {body}"
    );
    let aggregate: Value = serde_json::from_str(&body).expect("the response must be JSON");
    // The response is the read-after-write aggregate: exactly what was saved.
    assert_eq!(aggregate["model"], "Panthella 160 V3 Portable (Edited)");
    assert_eq!(aggregate["price_cents"], 160_000);
    assert_eq!(aggregate["art_variant"], "piano");
    assert_eq!(aggregate["stock_total"], 2);
    assert_eq!(aggregate["stock_quarantined"], 1);
    assert_eq!(
        aggregate["image_day_url"],
        "https://cdn.example.com/new-day.png"
    );
    assert_eq!(
        aggregate["image_lit_url"],
        "https://cdn.example.com/new-lit.webp"
    );
    assert_eq!(aggregate["content"]["en"]["colourway"], "Beige Prime");
    assert_eq!(aggregate["content"]["zh-HK"]["heritage"], "新版傳承文字。");
    assert_eq!(aggregate["reference_price"]["min_price_cents"], 100_000);
    assert_eq!(aggregate["reference_price"]["checked_on"], "2026-02-28");

    // Direct-SQL witnesses: the DB row, the content rows and the reference.
    let row = product_json(&db, SLUG);
    assert_eq!(row["model"], "Panthella 160 V3 Portable (Edited)");
    assert_eq!(row["price_cents"], 160_000);
    assert_eq!(row["art_variant"], "piano");
    assert_eq!(row["stock_total"], 2);
    assert_eq!(row["stock_reserved"], 0);
    assert_eq!(row["stock_quarantined"], 1);
    assert_eq!(row["image_day_url"], "https://cdn.example.com/new-day.png");
    assert_eq!(row["designer"], "Verner Panton");
    assert_eq!(row["light_source"], "2700K Integrated LED · 2.5W");
    assert_eq!(
        content_value(&db, SLUG, "en", "colourway").as_deref(),
        Some("Beige Prime"),
        "the existing content row is upserted, not duplicated"
    );
    assert_eq!(
        content_value(&db, SLUG, "zh-HK", "heritage").as_deref(),
        Some("新版傳承文字。"),
        "the zh-HK heritage row is upserted"
    );
    let reference = reference_json(&db, SLUG).expect("the reference row must exist");
    assert_eq!(reference["min_price_cents"], 100_000);
    assert_eq!(reference["max_price_cents"], 200_000);
    assert_eq!(reference["source_name"], "Test Shop");

    // Exactly ONE audit row for the whole save (the audit IS the mutation's
    // accountability record), at the handler's documented action/entity/id.
    let audits = audit_rows(&db);
    assert_eq!(audits.len(), 1, "one save = one audit row: {audits:?}");
    let (actor, action, entity, entity_id, detail) = &audits[0];
    assert_eq!(actor, common::ADMIN_USERNAME);
    assert_eq!(action, "update_full");
    assert_eq!(entity, "products");
    assert_eq!(entity_id.as_deref(), Some("1"));
    assert_eq!(detail["model"], "Panthella 160 V3 Portable (Edited)");
    assert_eq!(detail["stock_quarantined"], 1);
}

#[test]
fn update_product_full_deletes_the_reference_price_when_absent() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "product_full_reference");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());
    let cookie = minted_session_cookie(&state);

    // `null` deletes the seeded reference (verification pending again).
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &full_body("Panthella 160 V3 Portable", 161_500, Value::Null),
    );
    assert_eq!(status, StatusCode::OK, "the save must be accepted: {body}");
    let aggregate: Value = serde_json::from_str(&body).expect("the response must be JSON");
    assert!(aggregate["reference_price"].is_null());
    assert!(
        reference_json(&db, SLUG).is_none(),
        "null reference_price must delete the row"
    );

    // A MISSING key has the same meaning as null (both = delete): the form
    // always posts its whole state, so silence is not "keep".
    let (status, body) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &full_body("Panthella 160 V3 Portable", 161_500, reference_input()),
    );
    assert_eq!(status, StatusCode::OK, "re-add the reference: {body}");
    assert!(reference_json(&db, SLUG).is_some());

    let mut body: Value = serde_json::from_str(&full_body(
        "Panthella 160 V3 Portable",
        161_500,
        Value::Null,
    ))
    .expect("body parses");
    body.as_object_mut()
        .expect("the body is a JSON object")
        .remove("reference_price");
    let (status, body_text) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &body.to_string(),
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "a save without the key must be accepted: {body_text}"
    );
    assert!(
        reference_json(&db, SLUG).is_none(),
        "a missing reference_price key must delete the row too"
    );
}

#[test]
fn update_product_full_enforces_the_stock_invariant_and_validates_first() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "product_full_stock");
    db.apply_migrations();
    let state = boot_admin(&db);
    let router = admin_router(state.clone());
    let cookie = minted_session_cookie(&state);

    // One unit is reserved (the money path's row; no form can post it), so
    // total = quarantined = 1 would break total >= reserved + quarantined.
    db.block_on(async {
        sqlx::query("UPDATE products SET stock_reserved = 1 WHERE slug = $1")
            .bind(SLUG)
            .execute(db.pool())
            .await
            .expect("reserving a unit failed");
    });

    let mut body: Value = serde_json::from_str(&full_body(
        "Panthella 160 V3 Portable",
        161_500,
        reference_input(),
    ))
    .expect("body parses");
    body["stock_total"] = json!(1);
    body["stock_quarantined"] = json!(1);
    let (status, response) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &body.to_string(),
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invariant violation: {response}"
    );
    assert_eq!(response, STOCK_INVARIANT_400, "the 400 body is exact");

    // Nothing was written: the row keeps its values and no audit row exists
    // (the transaction rolled back before any write landed).
    let row = product_json(&db, SLUG);
    assert_eq!(row["stock_total"], 2, "the failed save must not move stock");
    assert_eq!(row["stock_quarantined"], 0);
    assert_eq!(
        row["model"], "Panthella 160 V3 Portable",
        "the failed save must not edit the row"
    );
    assert!(
        audit_rows(&db).is_empty(),
        "a rejected save must not fabricate an audit row"
    );

    // An invalid art_variant is rejected with the exact message BEFORE any
    // DB work (the same contract as the single-row validators).
    let mut body: Value = serde_json::from_str(&full_body(
        "Panthella 160 V3 Portable",
        161_500,
        reference_input(),
    ))
    .expect("body parses");
    body["art_variant"] = json!("shadestall");
    let object = body.as_object_mut().expect("the body is a JSON object");
    object.remove("stock_total");
    object.remove("stock_quarantined");
    let (status, response) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        "/api/admin/products/1/full",
        &body.to_string(),
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "bad variant: {response}");
    assert_eq!(response, ART_VARIANT_400, "the 400 body is exact");

    // A missing product is a clean 404, never a 503 or a fabricated save.
    let (status, response) = call(
        &db,
        &router,
        &cookie,
        Method::PUT,
        &format!("/api/admin/products/{MISSING_PRODUCT_ID}/full"),
        &full_body("Panthella 160 V3 Portable", 161_500, Value::Null),
    );
    assert_eq!(status, StatusCode::NOT_FOUND, "missing id: {response}");
    assert_eq!(response, PRODUCT_NOT_FOUND_404, "the 404 body is exact");
    assert!(
        audit_rows(&db).is_empty(),
        "a 404 save must not fabricate an audit row"
    );
}
