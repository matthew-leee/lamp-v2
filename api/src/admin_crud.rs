//! `GET/POST/PUT/PATCH/DELETE /api/admin/*` — the D2a admin CRUD surface
//! (ADMIN-01, DATA-02, CONFIG-01) over products, product_content,
//! reference_prices, pieces, site_content and settings, plus the audit log.
//!
//! Contract every mutation follows, in this order (and WHY):
//! 1. **Validate FIRST, before any DB work.** All validation is pure
//!    functions in this module: a bad request is a 400 with a specific
//!    `{"error": "..."}` body and must never open a connection — the tests
//!    run the handlers with no pool at all to pin that ordering.
//! 2. **Mutate + write the `audit_log` row in ONE transaction**
//!    (commitment 2). The audit row IS part of the mutation, not a follow-up:
//!    a crash between the two would silently lose the accountability record
//!    the trust architecture promises. Actor comes from the `CurrentUser`
//!    the auth middleware put in request extensions — never from the body.
//! 3. **Commit, then fire the revalidation webhook** (commitment 5b) with
//!    the matching cache tag (`catalogue` / `site-content` / `settings`) so
//!    the Next tier purges its data cache within seconds. Fire-and-forget in
//!    a spawned task: the HTTP round trip must never delay the admin
//!    response, and the documented 60s ISR window is the fallback — one
//!    warn! on failure, no retries.
//!
//! Products are data: there is no DELETE for them (soft `active` toggle
//! instead). Deletes that DO exist (reference_prices, pieces, site_content,
//! settings overrides) answer 404 when no row matched: a "successful" delete
//! of nothing would fabricate an audit row for a mutation that never
//! happened.
//!
//! Settings writes enforce the SAME whitelist as the public reads
//! (`content::PUBLIC_SETTING_KEYS`), and additionally reject secret-looking
//! keys (defense in depth): D1 audited that the whitelist was only enforced
//! on reads — D2 closes the write side (D1 reviewer finding).
//!
//! Free-text values reject NUL bytes client-side (Postgres TEXT cannot hold
//! them; a 503 from the DB would be the wrong answer to a client mistake),
//! and locale-carrying writes validate against the configured pair (LOCALE-
//! 02: exactly two distinct tags, mirroring the web tier's parser).
//!
//! JSON field names are snake_case, exactly like the catalogue DTOs and the
//! `/api/products` rows; the web tier's client maps them to camelCase.

use std::collections::BTreeMap;

use axum::{
    extract::{rejection::JsonRejection, Extension, Path, State},
    http::{header, StatusCode},
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use tracing::warn;

use crate::admin::CurrentUser;
use crate::config;
use crate::content;
use crate::state::{self, ApiError, AppState, ErrorBody};

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

/// 400: validation rejected the request (a specific message, per the D2a
/// contract).
fn bad_request(msg: &str) -> ApiError {
    error_body(StatusCode::BAD_REQUEST, msg)
}

/// 401: no (or invalid) session — same generic body as `admin::whoami`.
fn unauthenticated() -> ApiError {
    error_body(StatusCode::UNAUTHORIZED, "not authenticated")
}

/// 404: the target row does not exist (product/piece/row/override).
fn not_found(msg: &str) -> ApiError {
    error_body(StatusCode::NOT_FOUND, msg)
}

/// 409: a uniqueness conflict (duplicate slug / piece_no).
fn conflict(msg: &str) -> ApiError {
    error_body(StatusCode::CONFLICT, msg)
}

/// Maps a mutation error: a UNIQUE/PK violation on the given constraint is a
/// client-fixable conflict (409); everything else is the standard generic
/// 503 (driver detail stays in the server log — never to the client).
fn mutation_err(
    e: sqlx::Error,
    conflict_constraint: Option<(&str, &'static str)>,
    context: &str,
) -> ApiError {
    if let (sqlx::Error::Database(db), Some((constraint, msg))) = (&e, conflict_constraint) {
        if db.constraint() == Some(constraint) {
            return conflict(msg);
        }
    }
    state::database_unavailable(context, &e)
}

/// Unwraps the `Json` extractor rejection (malformed JSON, bad field types)
/// into the standard 400 body. A form problem, never a 5xx.
fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|_| bad_request("invalid JSON body"))
}

/// The acting admin. Defensively read from request extensions (the
/// `require_admin` middleware always inserts it): a missing user is a 401,
/// never a 500 and never a value taken from the request body.
fn actor(user: Option<Extension<CurrentUser>>) -> Result<String, ApiError> {
    user.map(|Extension(user)| user.username)
        .ok_or_else(unauthenticated)
}

// ---------------------------------------------------------------------------
// DTOs (snake_case JSON, like the catalogue DTOs)
// ---------------------------------------------------------------------------

/// Full product row, ADMIN view: includes `stock_total`/`stock_reserved`
/// (deliberately — the catalogue withholds stock, the admin console must not).
#[derive(Debug, Serialize, FromRow)]
pub struct AdminProduct {
    pub id: i64,
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub art_variant: String,
    pub art_hue: i32,
    pub active: bool,
    pub stock_total: i32,
    pub stock_reserved: i32,
    pub created_at: DateTime<Utc>,
}

/// `POST /api/admin/products` body: every field required.
#[derive(Debug, Deserialize)]
pub struct CreateProduct {
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub art_variant: String,
    pub art_hue: i32,
    pub active: bool,
}

/// `PUT /api/admin/products/{id}` body: partial update. `None` (missing or
/// `null`) = leave unchanged; `Some` = validate and write.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateProduct {
    pub slug: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub price_cents: Option<i64>,
    pub family: Option<String>,
    pub art_variant: Option<String>,
    pub art_hue: Option<i32>,
    pub active: Option<bool>,
}

/// The complete product aggregate for the flagship admin editor (R1):
/// returned by `GET /api/admin/products/{id}` and
/// `PUT /api/admin/products/{id}/full` — every editable field of one SKU in
/// one response, for `active` AND `inactive` products alike (the admin view
/// never hides a row).
///
/// Stock is included in full, `stock_quarantined` included (STOCK-01:
/// returned or damaged units are quarantined, never silently restocked).
/// This is the ADMIN view; the public catalogue withholds stock entirely.
#[derive(Debug, Serialize)]
pub struct AdminProductFull {
    pub id: i64,
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub art_variant: String,
    pub art_hue: i32,
    pub active: bool,
    pub stock_total: i32,
    pub stock_reserved: i32,
    pub stock_quarantined: i32,
    pub image_day_url: String,
    pub image_lit_url: String,
    pub designer: String,
    pub design_year: String,
    pub dimensions: String,
    pub light_source: String,
    pub created_at: DateTime<Utc>,
    /// `locale -> (field -> value)`, exactly the rows that exist (no copy is
    /// ever invented for a missing row).
    pub content: BTreeMap<String, BTreeMap<String, String>>,
    pub reference_price: Option<AdminReferencePrice>,
    /// Piece numbers owned by this SKU, ascending.
    pub pieces: Vec<i32>,
    /// Deterministic content fingerprint of the editable state (see
    /// [`compute_product_version_hash`]): the flagship editor sends it back as
    /// `expected_version_hash` so a stale tab can never overwrite a newer
    /// save (409 instead).
    pub version_hash: String,
}

/// The content-bearing state [`compute_product_version_hash`] covers. Stock
/// counts are deliberately EXCLUDED: the money path moves `stock_reserved`
/// and `stock_total` on every order, and a stock move must not invalidate an
/// admin tab's content fingerprint (concurrent-stock protection is the CSV
/// sync's snapshot job).
#[derive(Debug, Clone)]
pub struct ProductVersionState {
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub art_variant: String,
    pub art_hue: i32,
    pub active: bool,
    pub image_day_url: String,
    pub image_lit_url: String,
    pub designer: String,
    pub design_year: String,
    pub dimensions: String,
    pub light_source: String,
    pub content: BTreeMap<String, BTreeMap<String, String>>,
    pub reference_price: Option<AdminReferencePrice>,
}

/// Deterministic content fingerprint: SHA-256 over a length-prefixed canonical
/// encoding of every content-bearing field (locale/field order comes from the
/// `BTreeMap`s, so the same state always hashes the same), returned as a
/// 16-character lowercase hex prefix. The alias-free length prefixes mean two
/// different states can never serialise to the same canonical string, and the
/// short prefix keeps the admin/CSV payloads readable (collision odds remain
/// negligible for an optimistic-concurrency token).
///
/// Reused by `csv_sync.rs` for the `row_version_hash` CSV column.
pub fn compute_product_version_hash(state: &ProductVersionState) -> String {
    use std::fmt::Write as _;

    // Length-prefixed field write: `{len}:{value}\u{1f}`. Lengths make the
    // encoding injective even when values contain separators themselves.
    fn push_field(buf: &mut String, value: &str) {
        let _ = write!(buf, "{}:", value.len());
        buf.push_str(value);
        buf.push('\u{1f}');
    }

    let mut canonical = String::new();
    push_field(&mut canonical, &state.slug);
    push_field(&mut canonical, &state.brand);
    push_field(&mut canonical, &state.model);
    push_field(&mut canonical, &state.price_cents.to_string());
    push_field(&mut canonical, &state.family);
    push_field(&mut canonical, &state.art_variant);
    push_field(&mut canonical, &state.art_hue.to_string());
    push_field(&mut canonical, if state.active { "1" } else { "0" });
    push_field(&mut canonical, &state.image_day_url);
    push_field(&mut canonical, &state.image_lit_url);
    push_field(&mut canonical, &state.designer);
    push_field(&mut canonical, &state.design_year);
    push_field(&mut canonical, &state.dimensions);
    push_field(&mut canonical, &state.light_source);
    for (locale, fields) in &state.content {
        push_field(&mut canonical, locale);
        for (field, value) in fields {
            push_field(&mut canonical, field);
            push_field(&mut canonical, value);
        }
    }
    match &state.reference_price {
        Some(reference) => {
            push_field(&mut canonical, "reference_price:some");
            push_field(&mut canonical, &reference.min_price_cents.to_string());
            push_field(&mut canonical, &reference.max_price_cents.to_string());
            push_field(&mut canonical, &reference.source_name);
            push_field(&mut canonical, &reference.source_url);
            push_field(&mut canonical, &reference.checked_on.to_string());
        }
        None => push_field(&mut canonical, "reference_price:none"),
    }

    let digest = Sha256::digest(canonical.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex.truncate(16);
    hex
}

/// The scalar `products` columns behind [`AdminProductFull`]. The nested
/// content map is assembled in Rust (same as the catalogue): the driver
/// never decodes a map type.
#[derive(Debug, FromRow)]
struct ProductFullRow {
    id: i64,
    slug: String,
    brand: String,
    model: String,
    price_cents: i64,
    family: String,
    art_variant: String,
    art_hue: i32,
    active: bool,
    stock_total: i32,
    stock_reserved: i32,
    stock_quarantined: i32,
    image_day_url: String,
    image_lit_url: String,
    designer: String,
    design_year: String,
    dimensions: String,
    light_source: String,
    created_at: DateTime<Utc>,
}

/// `PUT /api/admin/products/{id}/full` body: the flagship editor saves its
/// whole form in one request. `stock_total` / `stock_quarantined` are
/// OPTIONAL (missing or `null` = keep the count the row has; reservations
/// are the money path's and are never posted from a form).
/// `reference_price` is deliberately tri-state: `Some` upserts the verified
/// reference range, `None` (missing or `null`) deletes the row (the SKU goes
/// back to "verification pending"). `content` upserts per-(locale, field)
/// rows and must pass the single-row rules for every entry.
#[derive(Debug, Deserialize)]
pub struct UpdateProductFull {
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub art_variant: String,
    pub art_hue: i32,
    pub active: bool,
    pub stock_total: Option<i32>,
    pub stock_quarantined: Option<i32>,
    pub image_day_url: String,
    pub image_lit_url: String,
    pub designer: String,
    pub design_year: String,
    pub dimensions: String,
    pub light_source: String,
    pub content: BTreeMap<String, BTreeMap<String, String>>,
    pub reference_price: Option<ReferencePriceInput>,
    /// Stale-tab guard: when set (non-empty after trimming), the save only
    /// lands if the row's CURRENT [`AdminProductFull::version_hash`] equals
    /// it; a mismatch is a 409 (another session saved first).
    #[serde(default)]
    pub expected_version_hash: Option<String>,
    /// Concurrent stock-edit guard: when the operator explicitly supplies
    /// `stock_total` or `stock_quarantined`, this carries the `stock_total`
    /// the studio tab saw on load; if a customer order or another session
    /// changed `stock_total` in the meantime, the stock override is refused
    /// with a 409 so a stale tab cannot restore sold stock.
    #[serde(default)]
    pub expected_stock_total: Option<i32>,
}

/// `PUT /api/admin/product-content` body and response row.
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct ProductContentInput {
    pub product_id: i64,
    pub locale: String,
    pub field: String,
    pub value: String,
}

/// `PUT /api/admin/reference-prices/{product_id}` body.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReferencePriceInput {
    pub min_price_cents: i64,
    pub max_price_cents: i64,
    pub source_name: String,
    pub source_url: String,
    /// Checked date as `YYYY-MM-DD`, validated as a real calendar date.
    pub checked_on: String,
}

/// `reference_prices` row (response).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminReferencePrice {
    pub product_id: i64,
    pub min_price_cents: i64,
    pub max_price_cents: i64,
    pub source_name: String,
    pub source_url: String,
    /// DB DATE, serialized as `YYYY-MM-DD` (chrono serde, like the catalogue).
    pub checked_on: NaiveDate,
}

/// `POST /api/admin/pieces` body.
#[derive(Debug, Deserialize)]
pub struct PieceInput {
    pub piece_no: i32,
    pub product_id: i64,
}

/// `PATCH /api/admin/pieces/{piece_no}` body: the new owning product.
#[derive(Debug, Deserialize)]
pub struct PiecePatch {
    pub product_id: i64,
}

/// `pieces` row (response).
#[derive(Debug, Serialize, FromRow)]
pub struct AdminPiece {
    pub piece_no: i32,
    pub product_id: i64,
}

/// `PUT /api/admin/site-content` body and response row.
#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct SiteContentInput {
    pub key: String,
    pub locale: String,
    pub value: String,
}

/// `PUT /api/admin/settings/{key}` body.
#[derive(Debug, Deserialize)]
pub struct SettingInput {
    pub value: String,
}

/// `settings` row (response).
#[derive(Debug, Serialize, FromRow)]
pub struct AdminSetting {
    pub key: String,
    pub value: String,
    pub updated_at: DateTime<Utc>,
}

/// One `audit_log` row for `GET /api/admin/audit-log`.
#[derive(Debug, Serialize, FromRow)]
pub struct AuditLogEntry {
    pub id: i64,
    pub actor: String,
    pub action: String,
    pub entity: String,
    pub entity_id: Option<String>,
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Small success body for deletes.
#[derive(Debug, Serialize)]
pub struct OkBody {
    pub ok: bool,
}

fn ok() -> Json<OkBody> {
    Json(OkBody { ok: true })
}

// ---------------------------------------------------------------------------
// Validation (pure — called BEFORE any DB work)
// ---------------------------------------------------------------------------

/// Secret-looking settings keys are never admin-editable, in either direction
/// (defense in depth; every one of them is also outside the whitelist).
fn is_secret_looking(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["secret", "password", "token", "credential", "private"]
        .iter()
        .any(|part| key.contains(part))
}

/// Settings write whitelist (CONFIG-01 + SCOPE-01 + the D1 reviewer finding:
/// D1 enforced `PUBLIC_SETTING_KEYS` only on reads — writes are enforced here).
/// Secret-looking keys get the SAME 400 message (never confirm a key exists).
fn validate_setting_key(key: &str) -> Result<(), &'static str> {
    if is_secret_looking(key) || !content::PUBLIC_SETTING_KEYS.contains(&key) {
        return Err("settings key not admin-editable");
    }
    Ok(())
}

/// `currency` has NO OFF state (founder toggle spec): three uppercase
/// letters, `^[A-Z]{3}$`.
fn validate_currency(value: &str) -> Result<(), &'static str> {
    if value.len() == 3 && value.chars().all(|c| c.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err("currency must be three uppercase letters (ISO 4217)")
    }
}

/// Settings write validation. `brand.name` and `site.url` are no-toggle
/// required fields (empty = 400); `currency` is shape-checked; every other
/// whitelisted key may be written EMPTY — an empty override present is the
/// explicit OFF state (CONFIG-01 presence-wins).
fn validate_setting(key: &str, value: &str) -> Result<(), &'static str> {
    validate_setting_key(key)?;
    reject_nul(value)?;
    match key {
        "brand.name" if value.trim().is_empty() => Err("brand.name cannot be empty"),
        "site.url" if value.trim().is_empty() => Err("site.url cannot be empty"),
        "currency" => validate_currency(value),
        _ => Ok(()),
    }
}

/// Postgres TEXT cannot hold a NUL byte: free-text values carrying one would
/// sail through validation, die at the DB, and come back as the generic 503.
/// Reject them client-side as the 400 they are (reviewer finding).
fn reject_nul(value: &str) -> Result<(), &'static str> {
    if value.contains('\0') {
        Err("value contains a NUL byte")
    } else {
        Ok(())
    }
}

/// Locale membership (LOCALE-01): the locale must be in the configured set
/// (`config::locales()`), so no content is written for a locale the web tier
/// never serves.
fn locale_allowed(locale: &str, locales: &[String]) -> bool {
    locales.iter().any(|l| l == locale)
}

fn validate_product_id(product_id: i64) -> Result<(), &'static str> {
    if product_id >= 1 {
        Ok(())
    } else {
        Err("product_id must be >= 1")
    }
}

fn validate_piece_no(piece_no: i32) -> Result<(), &'static str> {
    if piece_no >= 1 {
        Ok(())
    } else {
        Err("piece_no must be >= 1")
    }
}

fn validate_price_cents(price_cents: i64) -> Result<(), &'static str> {
    if price_cents >= 0 {
        Ok(())
    } else {
        Err("price_cents must be >= 0")
    }
}

pub(crate) fn validate_art_variant(art_variant: &str) -> Result<(), &'static str> {
    match art_variant {
        "dome" | "shades" | "shades-tall" | "piano" | "none" => Ok(()),
        _ => Err(
            "art_variant must be one of \"dome\", \"shades\", \"shades-tall\", \"piano\", \"none\"",
        ),
    }
}

pub(crate) fn validate_art_hue(art_hue: i32) -> Result<(), &'static str> {
    if (0..=359).contains(&art_hue) {
        Ok(())
    } else {
        Err("art_hue must be between 0 and 359")
    }
}

pub(crate) fn validate_slug_input(slug: &str) -> Result<(), &'static str> {
    if crate::slug::validate_slug(slug) {
        Ok(())
    } else {
        Err("slug must be 1-64 lowercase letters, digits or hyphens")
    }
}

/// Create-product validation: every field, same rules as the DB CHECKs so the
/// client gets a 400 instead of a 503 from a constraint violation.
fn validate_product_create(input: &CreateProduct) -> Result<(), &'static str> {
    validate_slug_input(&input.slug)?;
    if input.brand.trim().is_empty() {
        return Err("brand cannot be empty");
    }
    reject_nul(&input.brand)?;
    if input.model.trim().is_empty() {
        return Err("model cannot be empty");
    }
    reject_nul(&input.model)?;
    reject_nul(&input.family)?;
    validate_price_cents(input.price_cents)?;
    validate_art_variant(&input.art_variant)?;
    validate_art_hue(input.art_hue)
}

/// Partial-update validation: each `Some` field passes the same rules as on
/// create; `None` fields are untouched. At least one field must be present —
/// an empty patch must not fabricate an audit row / webhook event for a
/// mutation that changed nothing.
fn validate_product_patch(patch: &UpdateProduct) -> Result<(), &'static str> {
    let any = patch.slug.is_some()
        || patch.brand.is_some()
        || patch.model.is_some()
        || patch.price_cents.is_some()
        || patch.family.is_some()
        || patch.art_variant.is_some()
        || patch.art_hue.is_some()
        || patch.active.is_some();
    if !any {
        return Err("no fields to update");
    }
    if let Some(slug) = &patch.slug {
        validate_slug_input(slug)?;
    }
    if let Some(brand) = &patch.brand {
        if brand.trim().is_empty() {
            return Err("brand cannot be empty");
        }
        reject_nul(brand)?;
    }
    if let Some(model) = &patch.model {
        if model.trim().is_empty() {
            return Err("model cannot be empty");
        }
        reject_nul(model)?;
    }
    if let Some(family) = &patch.family {
        reject_nul(family)?;
    }
    if let Some(price) = patch.price_cents {
        validate_price_cents(price)?;
    }
    if let Some(art_variant) = &patch.art_variant {
        validate_art_variant(art_variant)?;
    }
    if let Some(art_hue) = patch.art_hue {
        validate_art_hue(art_hue)?;
    }
    Ok(())
}

/// Content-map validation for `PUT .../products/{id}/full`: every
/// (locale, field, value) entry passes the SAME rules as the single-row
/// upsert, and the request is rejected on the first violation. The flagship
/// editor posts the complete map, so silently SKIPPING a mistyped row would
/// lose copy without a word: a bad entry is a 400, never a drop.
///
/// An EMPTY (or whitespace-only) value is deliberately legal here: the full
/// save is the form's whole state, and clearing a text box means DELETE the
/// stored copy row (the handler does that), so emptying `zhColourway` must
/// not resurrect the old text.
fn validate_content_map(
    content: &BTreeMap<String, BTreeMap<String, String>>,
    locales: &[String],
) -> Result<(), &'static str> {
    for (locale, fields) in content {
        if !locale_allowed(locale, locales) {
            return Err("locale not in the configured locale set");
        }
        for (field, value) in fields {
            match field.as_str() {
                "model_alt" | "colourway" | "heritage" | "electrical" => {}
                _ => {
                    return Err("field must be one of model_alt, colourway, heritage, electrical");
                }
            }
            reject_nul(value)?;
        }
    }
    Ok(())
}

/// Full-update validation: every field of [`UpdateProductFull`], same rules
/// as the create/patch validators plus the R1 media/spec fields. Pure, runs
/// BEFORE any DB work. The stock cross-column invariant is the one rule that
/// needs the row's current `stock_reserved` (reservations are written by the
/// money path, never posted from a form) — [`validate_full_stock`] resolves
/// it against the locked row inside the transaction.
fn validate_product_full(
    input: &UpdateProductFull,
    locales: &[String],
) -> Result<(), &'static str> {
    validate_slug_input(&input.slug)?;
    if input.brand.trim().is_empty() {
        return Err("brand cannot be empty");
    }
    reject_nul(&input.brand)?;
    if input.model.trim().is_empty() {
        return Err("model cannot be empty");
    }
    reject_nul(&input.model)?;
    reject_nul(&input.family)?;
    validate_price_cents(input.price_cents)?;
    validate_art_variant(&input.art_variant)?;
    validate_art_hue(input.art_hue)?;
    if let Some(stock_total) = input.stock_total {
        if stock_total < 0 {
            return Err("stock_total must be >= 0");
        }
    }
    if let Some(stock_quarantined) = input.stock_quarantined {
        if stock_quarantined < 0 {
            return Err("stock_quarantined must be >= 0");
        }
    }
    // Free-text spec/media fields: the shape is the admin's to describe, but
    // a NUL byte can never reach Postgres TEXT (same rule as every free-text
    // write in this module).
    for text in [
        &input.image_day_url,
        &input.image_lit_url,
        &input.designer,
        &input.design_year,
        &input.dimensions,
        &input.light_source,
    ] {
        reject_nul(text)?;
    }
    validate_content_map(&input.content, locales)?;
    if let Some(reference) = &input.reference_price {
        validate_reference_price(reference)?;
    }
    Ok(())
}

/// STOCK-01 cross-column invariant for the full update, resolved against the
/// row's CURRENT counts: the effective `stock_total` (posted or kept) must
/// cover `stock_reserved + stock_quarantined` (both effective), so an update
/// can never oversell the drop or quietly un-quarantine a unit. Returns the
/// effective `(stock_total, stock_quarantined)` the update writes. The
/// arithmetic runs in i64 so two large i32 counts can never overflow the
/// comparison.
fn validate_full_stock(
    stock_total: Option<i32>,
    stock_quarantined: Option<i32>,
    current_total: i32,
    current_reserved: i32,
    current_quarantined: i32,
) -> Result<(i32, i32), &'static str> {
    if let Some(total) = stock_total {
        if total < 0 {
            return Err("stock_total must be >= 0");
        }
    }
    if let Some(quarantined) = stock_quarantined {
        if quarantined < 0 {
            return Err("stock_quarantined must be >= 0");
        }
    }
    let effective_total = stock_total.unwrap_or(current_total);
    let effective_quarantined = stock_quarantined.unwrap_or(current_quarantined);
    if i64::from(effective_total) < i64::from(current_reserved) + i64::from(effective_quarantined) {
        return Err("stock_total must be >= stock_reserved + stock_quarantined");
    }
    Ok((effective_total, effective_quarantined))
}

fn validate_product_content(
    input: &ProductContentInput,
    locales: &[String],
) -> Result<(), &'static str> {
    validate_product_id(input.product_id)?;
    match input.field.as_str() {
        "model_alt" | "colourway" | "heritage" | "electrical" => {}
        _ => {
            return Err("field must be one of model_alt, colourway, heritage, electrical");
        }
    }
    if !locale_allowed(&input.locale, locales) {
        return Err("locale not in the configured locale set");
    }
    if input.value.trim().is_empty() {
        return Err("value cannot be empty");
    }
    reject_nul(&input.value)?;
    Ok(())
}

/// `checked_on` must be a real calendar date in strict `YYYY-MM-DD` shape
/// (`NaiveDate::parse_from_str` alone accepts `2026-2-3`; the shape check
/// pins the width the contract promises, the parse rejects `2026-02-30`).
fn validate_checked_on(raw: &str) -> Result<NaiveDate, &'static str> {
    const BAD: &str = "checked_on must be a real date in YYYY-MM-DD form";
    let shape_ok = raw.len() == 10
        && raw.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    if !shape_ok {
        return Err(BAD);
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| BAD)
}

/// Reference-price sources must be absolute `http://` or `https://` URLs
/// (checked ranges cite their source; a bare host is not citable). No
/// userinfo: a URL carrying credentials must not land in the audit trail.
/// `host[:port]` form only: host non-empty, port a valid u16 — `https://:443/x`
/// and `https://host:notaport/x` are NOT URLs (reviewer finding: both
/// previously reached the DB). IPv6 bracket literals are out of scope for
/// retail source cites; document + code-change if ever needed.
fn validate_source_url(raw: &str) -> Result<&str, &'static str> {
    const BAD: &str = "source_url must be an absolute http:// or https:// URL";
    let raw = raw.trim();
    reject_nul(raw)?;
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .ok_or(BAD)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(char::is_whitespace) || authority.contains('@') {
        return Err(BAD);
    }
    if let Some((host, port)) = authority.split_once(':') {
        if host.is_empty() || port.parse::<u16>().is_err() {
            return Err(BAD);
        }
    }
    Ok(raw)
}

fn validate_reference_price(input: &ReferencePriceInput) -> Result<(), &'static str> {
    if input.min_price_cents < 0 {
        return Err("min_price_cents must be >= 0");
    }
    if input.max_price_cents < 0 {
        return Err("max_price_cents must be >= 0");
    }
    if input.min_price_cents > input.max_price_cents {
        return Err("min_price_cents must be <= max_price_cents");
    }
    if input.source_name.trim().is_empty() {
        return Err("source_name cannot be empty");
    }
    reject_nul(&input.source_name)?;
    validate_source_url(&input.source_url)?;
    validate_checked_on(&input.checked_on)?;
    Ok(())
}

fn validate_piece_numbers(piece_no: i32, product_id: i64) -> Result<(), &'static str> {
    validate_piece_no(piece_no)?;
    validate_product_id(product_id)
}

/// Key + locale rules shared by the site_content upsert and its delete (the
/// delete accepts no value; the row identity rules are the same).
fn validate_site_content_key(
    key: &str,
    locale: &str,
    locales: &[String],
) -> Result<(), &'static str> {
    if key.trim().is_empty() {
        return Err("key cannot be empty");
    }
    // Shared by upsert AND delete: a NUL-bearing key (e.g. percent-encoded
    // %00 in the path) dies at Postgres TEXT — reject it here (reviewer
    // finding: the delete path previously bypassed the NUL check).
    reject_nul(key)?;
    if !locale_allowed(locale, locales) {
        return Err("locale not in the configured locale set");
    }
    Ok(())
}

fn validate_site_content(input: &SiteContentInput, locales: &[String]) -> Result<(), &'static str> {
    validate_site_content_key(&input.key, &input.locale, locales)?;
    if input.value.trim().is_empty() {
        return Err("value cannot be empty");
    }
    reject_nul(&input.value)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Revalidation webhook (commitment 5b)
// ---------------------------------------------------------------------------

/// Whole-webhook budget: fire-and-forget, but a hung connect must not leak a
/// task per mutation.
const WEBHOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Split `WebhookTarget` pieces for the HTTP/1 request: authority for `Host`,
/// host/port for the TCP connect, origin-form target for the request line.
/// Pure; kept in sync with `config::parse_revalidate_url` (which validates
/// the accepted shape at boot).
struct WebhookTarget {
    authority: String,
    host: String,
    port: u16,
    target: String,
}

fn webhook_target(url: &str) -> Option<WebhookTarget> {
    let rest = url.strip_prefix("http://")?;
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // Second line of defense after `config::parse_revalidate_url` (which
    // already rejects these): whitespace and userinfo never reach the wire.
    if authority.is_empty() || authority.contains(char::is_whitespace) || authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, 80),
    };
    if host.is_empty() {
        return None;
    }
    // A fragment is never sent in an HTTP request line (RFC 9110): drop it.
    let tail = match tail.find('#') {
        Some(i) => &tail[..i],
        None => tail,
    };
    // `?query` alone still needs the `/` for origin-form; an empty path is `/`.
    let target = if tail.is_empty() || tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Some(WebhookTarget {
        authority: authority.to_string(),
        host: host.to_string(),
        port,
        target,
    })
}

fn webhook_body(secret: &str, tag: &str) -> String {
    serde_json::json!({ "secret": secret, "tags": [tag] }).to_string()
}

/// Fire the revalidation webhook AFTER a committed mutation: one POST per
/// mutation, `tokio::spawn`ed so the response never waits for the web tier.
/// The 60s ISR revalidate window is the documented 5b fallback — on failure
/// (or timeout) warn! once and do NOT retry. Webhook off (knobs unset or
/// misconfigured): return silently; boot already warned once.
pub(crate) fn fire_revalidation(tag: &'static str) {
    tokio::spawn(async move {
        // Read at call time via the config fns (env-knob pattern): the URL is
        // needed here, not at boot.
        let Some(url) = config::revalidate_url() else {
            return;
        };
        let Some(secret) = config::revalidate_secret() else {
            return;
        };
        match tokio::time::timeout(WEBHOOK_TIMEOUT, post_webhook(&url, &secret, tag)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                warn!(error = %err, "revalidation webhook failed; the 60s ISR window is the fallback");
            }
            Err(_) => {
                warn!("revalidation webhook timed out; the 60s ISR window is the fallback");
            }
        }
    });
}

/// One webhook POST over a plain HTTP/1 connection (hyper client, no TLS —
/// that is exactly why `REVALIDATE_URL` rejects `https://`). The connection
/// driver is spawned alongside; only the response status is checked.
async fn post_webhook(url: &str, secret: &str, tag: &str) -> Result<(), String> {
    let target =
        webhook_target(url).ok_or_else(|| "REVALIDATE_URL is not a valid http URL".to_string())?;
    let stream = tokio::net::TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|e| format!("connect to revalidate_url failed: {e}"))?;
    // hyper 1 reads its own IO traits; TokioIo is the tokio bridge.
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::tokio::TokioIo::new(stream))
            .await
            .map_err(|e| format!("HTTP handshake with revalidate_url failed: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            warn!(error = %e, "revalidation webhook connection error");
        }
    });
    let request = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri(&target.target)
        .header(hyper::header::HOST, &target.authority)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(webhook_body(secret, tag)))
        .map_err(|e| format!("building the revalidation request failed: {e}"))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| format!("revalidation request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "revalidate_url answered HTTP {}",
            response.status()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Audit (commitment 2): one row per successful mutation, same transaction
// ---------------------------------------------------------------------------

/// One `audit_log` row for a successful mutation. Shared with the money-path
/// handlers in `orders` (same transaction, same contract: accountability is
/// part of the mutation, not a follow-up), hence `pub(crate)`.
pub(crate) async fn insert_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &str,
    action: &str,
    entity: &str,
    entity_id: Option<&str>,
    detail: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (actor, action, entity, entity_id, detail) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(entity)
    .bind(entity_id)
    .bind(sqlx::types::Json(detail))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Does the product exist? Checked inside the mutation's transaction so a
/// write for a missing product is a clean 404 instead of an FK violation
/// surfacing as the generic 503 (products have no DELETE route, so the row
/// cannot vanish under the transaction).
async fn product_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_id: i64,
) -> Result<bool, sqlx::Error> {
    let (exists,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM products WHERE id = $1)")
        .bind(product_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(exists)
}

/// Loads the complete [`AdminProductFull`] aggregate inside an OPEN
/// transaction, so a read-after-write (the full update's response) sees its
/// own committed-to-be rows and a standalone read (the full GET) sees one
/// coherent product state.
async fn load_product_full(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
) -> Result<Option<AdminProductFull>, sqlx::Error> {
    let Some(product) = sqlx::query_as::<_, ProductFullRow>(
        "SELECT id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, stock_quarantined, \
             image_day_url, image_lit_url, designer, design_year, dimensions, \
             light_source, created_at \
         FROM products WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };

    // Content rows keyed `locale -> (field -> value)`, exactly as stored.
    let mut content: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT locale, field, value FROM product_content \
         WHERE product_id = $1 ORDER BY locale, field",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    for (locale, field, value) in rows {
        content.entry(locale).or_default().insert(field, value);
    }

    let reference_price = sqlx::query_as::<_, AdminReferencePrice>(
        "SELECT product_id, min_price_cents, max_price_cents, source_name, source_url, checked_on \
         FROM reference_prices WHERE product_id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;

    let pieces = sqlx::query_scalar::<_, i32>(
        "SELECT piece_no FROM pieces WHERE product_id = $1 ORDER BY piece_no",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;

    // Fingerprint the exact content state the response carries: stock counts
    // are excluded on purpose (the money path moves them without changing
    // what the editor form edits).
    let version_hash = compute_product_version_hash(&ProductVersionState {
        slug: product.slug.clone(),
        brand: product.brand.clone(),
        model: product.model.clone(),
        price_cents: product.price_cents,
        family: product.family.clone(),
        art_variant: product.art_variant.clone(),
        art_hue: product.art_hue,
        active: product.active,
        image_day_url: product.image_day_url.clone(),
        image_lit_url: product.image_lit_url.clone(),
        designer: product.designer.clone(),
        design_year: product.design_year.clone(),
        dimensions: product.dimensions.clone(),
        light_source: product.light_source.clone(),
        content: content.clone(),
        reference_price: reference_price.clone(),
    });

    Ok(Some(AdminProductFull {
        id: product.id,
        slug: product.slug,
        brand: product.brand,
        model: product.model,
        price_cents: product.price_cents,
        family: product.family,
        art_variant: product.art_variant,
        art_hue: product.art_hue,
        active: product.active,
        stock_total: product.stock_total,
        stock_reserved: product.stock_reserved,
        stock_quarantined: product.stock_quarantined,
        image_day_url: product.image_day_url,
        image_lit_url: product.image_lit_url,
        designer: product.designer,
        design_year: product.design_year,
        dimensions: product.dimensions,
        light_source: product.light_source,
        created_at: product.created_at,
        content,
        reference_price,
        pieces,
        version_hash,
    }))
}

// ---------------------------------------------------------------------------
// Handlers: products
// ---------------------------------------------------------------------------

/// `GET /api/admin/products` — ALL products (inactive included), ordered by
/// id, full rows including stock: this is the admin view.
pub async fn list_products(
    State(app): State<AppState>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Vec<AdminProduct>>,
    ),
    ApiError,
> {
    let pool = state::require_pool(&app)?;
    let rows = sqlx::query_as::<_, AdminProduct>(
        "SELECT id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, created_at \
         FROM products ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| state::database_unavailable("failed to load products", &e))?;
    // no-store: admin data (esp. stock) is never cacheable (reviewer finding).
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(rows)))
}

/// `POST /api/admin/products` — create (stock starts at 0; stock is managed
/// by the money-path slice, not the catalogue CRUD).
pub async fn create_product(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<CreateProduct>, JsonRejection>,
) -> Result<Json<AdminProduct>, ApiError> {
    let actor = actor(user)?;
    let input = json_body(body)?;
    validate_product_create(&input).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open product transaction", &e))?;
    let row = sqlx::query_as::<_, AdminProduct>(
        "INSERT INTO products \
             (slug, brand, model, price_cents, family, art_variant, art_hue, active, stock_total) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0) \
         RETURNING id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, created_at",
    )
    .bind(&input.slug)
    .bind(&input.brand)
    .bind(&input.model)
    .bind(input.price_cents)
    .bind(&input.family)
    .bind(&input.art_variant)
    .bind(input.art_hue)
    .bind(input.active)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        mutation_err(
            e,
            Some(("products_slug_key", "slug already in use")),
            "failed to create product",
        )
    })?;

    let detail = serde_json::json!({
        "slug": input.slug,
        "brand": input.brand,
        "model": input.model,
        "price_cents": input.price_cents,
        "family": input.family,
        "art_variant": input.art_variant,
        "art_hue": input.art_hue,
        "active": input.active,
    });
    insert_audit(
        &mut tx,
        &actor,
        "create",
        "products",
        Some(&row.id.to_string()),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit product create", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit product create", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

/// Cascade a product slug rename onto every historical/active order line,
/// inside the caller's transaction (the caller already locked the product
/// row). An admin rename must never orphan the slug an order page displays:
/// both `orders.product_slug` and `order_items.product_slug` are the order
/// contract's own snapshot and are rewritten in the same atomic write.
///
/// Shared by the partial update, the full save and `csv_sync.rs`.
pub(crate) async fn cascade_product_slug(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_id: i64,
    new_slug: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE orders SET product_slug = $1 WHERE product_id = $2 AND product_slug <> $1")
        .bind(new_slug)
        .bind(product_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE order_items SET product_slug = $1 \
         WHERE product_id = $2 AND product_slug <> $1",
    )
    .bind(new_slug)
    .bind(product_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// `PUT /api/admin/products/{id}` — partial update; `None` fields unchanged.
/// Audit detail carries exactly the fields that changed.
pub async fn update_product(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    body: Result<Json<UpdateProduct>, JsonRejection>,
) -> Result<Json<AdminProduct>, ApiError> {
    let actor = actor(user)?;
    let id: i64 = id.parse().map_err(|_| bad_request("invalid product id"))?;
    // Same positive-ID rule as every other handler (reviewer finding: this
    // was the one that skipped it): 0/-1 are client-fixable, not DB hits.
    validate_product_id(id).map_err(bad_request)?;
    let patch = json_body(body)?;
    validate_product_patch(&patch).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open product transaction", &e))?;
    // The rename cascade needs the slug BEFORE the update: the row is locked
    // here, so the read/update pair cannot interleave with another save.
    let current_slug: String =
        sqlx::query_scalar("SELECT slug FROM products WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to load product", &e))?
            .ok_or_else(|| not_found("product not found"))?;
    let row = sqlx::query_as::<_, AdminProduct>(
        "UPDATE products SET \
             slug = COALESCE($1, slug), \
             brand = COALESCE($2, brand), \
             model = COALESCE($3, model), \
             price_cents = COALESCE($4, price_cents), \
             family = COALESCE($5, family), \
             art_variant = COALESCE($6, art_variant), \
             art_hue = COALESCE($7, art_hue), \
             active = COALESCE($8, active) \
         WHERE id = $9 \
         RETURNING id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, created_at",
    )
    .bind(patch.slug.as_deref())
    .bind(patch.brand.as_deref())
    .bind(patch.model.as_deref())
    .bind(patch.price_cents)
    .bind(patch.family.as_deref())
    .bind(patch.art_variant.as_deref())
    .bind(patch.art_hue)
    .bind(patch.active)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        mutation_err(
            e,
            Some(("products_slug_key", "slug already in use")),
            "failed to update product",
        )
    })?
    .ok_or_else(|| not_found("product not found"))?;

    // Slug rename: rewrite the order links in the same transaction so no
    // active/historical order keeps pointing at the old slug.
    if let Some(new_slug) = patch.slug.as_deref() {
        if new_slug != current_slug {
            cascade_product_slug(&mut tx, id, new_slug)
                .await
                .map_err(|e| state::database_unavailable("failed to cascade slug rename", &e))?;
        }
    }

    let mut detail = serde_json::Map::new();
    if let Some(v) = &patch.slug {
        detail.insert("slug".into(), v.as_str().into());
    }
    if let Some(v) = &patch.brand {
        detail.insert("brand".into(), v.as_str().into());
    }
    if let Some(v) = &patch.model {
        detail.insert("model".into(), v.as_str().into());
    }
    if let Some(v) = patch.price_cents {
        detail.insert("price_cents".into(), v.into());
    }
    if let Some(v) = &patch.family {
        detail.insert("family".into(), v.as_str().into());
    }
    if let Some(v) = &patch.art_variant {
        detail.insert("art_variant".into(), v.as_str().into());
    }
    if let Some(v) = patch.art_hue {
        detail.insert("art_hue".into(), v.into());
    }
    if let Some(v) = patch.active {
        detail.insert("active".into(), v.into());
    }
    insert_audit(
        &mut tx,
        &actor,
        "update",
        "products",
        Some(&id.to_string()),
        serde_json::Value::Object(detail),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit product update", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit product update", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Handlers: products (full aggregate — the flagship editor's single form)
// ---------------------------------------------------------------------------

/// `GET /api/admin/products/{id}` — the complete product aggregate for the
/// flagship editor: every editable field in one response, `active` and
/// `inactive` SKUs alike (the admin view never hides a row). 404
/// `product not found` when the id does not exist.
///
/// `no-store`: admin data (stock included) is never cacheable — the same
/// rule as the admin product list.
pub async fn get_product_full(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<AdminProductFull>,
    ),
    ApiError,
> {
    let id: i64 = id.parse().map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(id).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open product transaction", &e))?;
    // REPEATABLE READ: the aggregate joins four tables, and the admin console
    // mutates them concurrently (same coherence rule as the catalogue read).
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to set product isolation", &e))?;
    let product = load_product_full(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load product", &e))?
        .ok_or_else(|| not_found("product not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit product read", &e))?;

    Ok(([(header::CACHE_CONTROL, "no-store")], Json(product)))
}

/// `PUT /api/admin/products/{id}/full` — the flagship editor's single save:
/// product fields, content rows and the reference price in ONE transaction,
/// ONE `audit_log` row (`update_full`), then the revalidation webhook.
///
/// Validation runs before any DB work, except the stock cross-column
/// invariant: only the row's current `stock_reserved` can resolve it, so the
/// row is locked first (`FOR UPDATE`), the invariant checked, and a violation
/// rejected with the same 400 shape BEFORE anything is written.
pub async fn update_product_full(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    body: Result<Json<UpdateProductFull>, JsonRejection>,
) -> Result<Json<AdminProductFull>, ApiError> {
    let actor = actor(user)?;
    let id: i64 = id.parse().map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(id).map_err(bad_request)?;
    let input = json_body(body)?;
    validate_product_full(&input, &config::locales()).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open product transaction", &e))?;
    let current_stock = sqlx::query_as::<_, (String, i32, i32, i32)>(
        "SELECT slug, stock_total, stock_reserved, stock_quarantined \
         FROM products WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to load product stock", &e))?;
    let Some((current_slug, current_total, current_reserved, current_quarantined)) = current_stock
    else {
        return Err(not_found("product not found"));
    };
    // Stale-tab guard: the row is locked, so this comparison is against the
    // exact state the save will overwrite. A non-empty expected hash that no
    // longer matches means another session saved first — 409, never a silent
    // clobber of the newer edit.
    if let Some(expected) = input
        .expected_version_hash
        .as_deref()
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
    {
        let current = load_product_full(&mut tx, id)
            .await
            .map_err(|e| state::database_unavailable("failed to load current product", &e))?
            .ok_or_else(|| not_found("product not found"))?;
        if current.version_hash != expected {
            return Err(conflict(
                "product was modified by another session; reload before saving",
            ));
        }
    }
    if input.stock_total.is_some() || input.stock_quarantined.is_some() {
        if let Some(expected_stock) = input.expected_stock_total {
            if expected_stock != current_total {
                return Err(conflict(
                    "stock changed while this tab was open; reload before overriding stock",
                ));
            }
        }
    }
    let (stock_total, stock_quarantined) = validate_full_stock(
        input.stock_total,
        input.stock_quarantined,
        current_total,
        current_reserved,
        current_quarantined,
    )
    .map_err(bad_request)?;

    sqlx::query(
        "UPDATE products SET \
             slug = $1, brand = $2, model = $3, price_cents = $4, family = $5, \
             art_variant = $6, art_hue = $7, active = $8, \
             stock_total = $9, stock_quarantined = $10, \
             image_day_url = $11, image_lit_url = $12, designer = $13, \
             design_year = $14, dimensions = $15, light_source = $16 \
         WHERE id = $17",
    )
    .bind(&input.slug)
    .bind(&input.brand)
    .bind(&input.model)
    .bind(input.price_cents)
    .bind(&input.family)
    .bind(&input.art_variant)
    .bind(input.art_hue)
    .bind(input.active)
    .bind(stock_total)
    .bind(stock_quarantined)
    .bind(&input.image_day_url)
    .bind(&input.image_lit_url)
    .bind(&input.designer)
    .bind(&input.design_year)
    .bind(&input.dimensions)
    .bind(&input.light_source)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        mutation_err(
            e,
            Some(("products_slug_key", "slug already in use")),
            "failed to update product",
        )
    })?;

    // Slug rename: rewrite the order links in the same transaction so no
    // active/historical order keeps pointing at the old slug.
    if input.slug != current_slug {
        cascade_product_slug(&mut tx, id, &input.slug)
            .await
            .map_err(|e| state::database_unavailable("failed to cascade slug rename", &e))?;
    }

    // Content rows are the form's WHOLE state: every stored (locale, field)
    // the request omits or sends blank is DELETED, then every non-blank entry
    // is upserted. Clearing a text box in Admin Studio must remove the old
    // copy row, never leave it resurrected for the storefront to serve.
    let stored_content: Vec<(String, String)> =
        sqlx::query_as("SELECT locale, field FROM product_content WHERE product_id = $1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to load product content", &e))?;
    let is_cleared = |locale: &str, field: &str| -> bool {
        input
            .content
            .get(locale)
            .and_then(|fields| fields.get(field))
            .is_none_or(|value| value.trim().is_empty())
    };
    for (locale, field) in &stored_content {
        if is_cleared(locale, field) {
            sqlx::query(
                "DELETE FROM product_content WHERE product_id = $1 AND locale = $2 AND field = $3",
            )
            .bind(id)
            .bind(locale)
            .bind(field)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to delete product content", &e))?;
        }
    }
    for (locale, fields) in &input.content {
        for (field, value) in fields {
            if value.trim().is_empty() {
                continue; // already handled by the delete pass above
            }
            sqlx::query(
                "INSERT INTO product_content (product_id, locale, field, value) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (product_id, locale, field) DO UPDATE SET value = EXCLUDED.value",
            )
            .bind(id)
            .bind(locale)
            .bind(field)
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to upsert product content", &e))?;
        }
    }

    // Reference price: `Some` upserts the verified range, `None` deletes the
    // row (TRUST-02: back to "verification pending", never an invented price).
    match &input.reference_price {
        Some(reference) => {
            // Validated in validate_product_full: the parse is infallible here
            // (no unwrap in handlers).
            let checked_on = validate_checked_on(&reference.checked_on).map_err(bad_request)?;
            sqlx::query(
                "INSERT INTO reference_prices \
                     (product_id, min_price_cents, max_price_cents, source_name, \
                      source_url, checked_on) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 ON CONFLICT (product_id) DO UPDATE SET \
                     min_price_cents = EXCLUDED.min_price_cents, \
                     max_price_cents = EXCLUDED.max_price_cents, \
                     source_name = EXCLUDED.source_name, \
                     source_url = EXCLUDED.source_url, \
                     checked_on = EXCLUDED.checked_on",
            )
            .bind(id)
            .bind(reference.min_price_cents)
            .bind(reference.max_price_cents)
            .bind(&reference.source_name)
            .bind(&reference.source_url)
            .bind(checked_on)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to upsert reference price", &e))?;
        }
        None => {
            sqlx::query("DELETE FROM reference_prices WHERE product_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| state::database_unavailable("failed to delete reference price", &e))?;
        }
    }

    // ONE audit row for the whole save (the audit IS part of the mutation).
    let detail = serde_json::json!({
        "slug": input.slug,
        "brand": input.brand,
        "model": input.model,
        "price_cents": input.price_cents,
        "family": input.family,
        "art_variant": input.art_variant,
        "art_hue": input.art_hue,
        "active": input.active,
        "stock_total": input.stock_total,
        "stock_quarantined": input.stock_quarantined,
        "image_day_url": input.image_day_url,
        "image_lit_url": input.image_lit_url,
        "designer": input.designer,
        "design_year": input.design_year,
        "dimensions": input.dimensions,
        "light_source": input.light_source,
        "content": input.content,
        "reference_price": input.reference_price,
    });
    insert_audit(
        &mut tx,
        &actor,
        "update_full",
        "products",
        Some(&id.to_string()),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit product full update", &e))?;

    // Read-after-write on the SAME transaction: the response is exactly the
    // state just committed (never a second, stale read).
    let product = load_product_full(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load updated product", &e))?
        .ok_or_else(|| not_found("product not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit product full update", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(product))
}

/// Request body for `POST /api/admin/products/{id}/inspect-quarantine`.
#[derive(Debug, Deserialize)]
pub struct InspectQuarantineInput {
    /// `"restock"` (pass inspection -> return to saleable stock) or
    /// `"write_off"` (damaged -> remove from both stock_quarantined and stock_total).
    pub action: String,
    #[serde(default = "default_one")]
    pub quantity: i32,
}

fn default_one() -> i32 {
    1
}

/// `POST /api/admin/products/{id}/clone` — 1-click duplicate of an existing
/// SKU (copies family specs, media slots, reference price and all bilingual
/// `product_content` rows into a new inactive SKU so the operator only edits
/// the new colourway/slug).
pub async fn clone_product(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
) -> Result<Json<AdminProductFull>, ApiError> {
    let actor = actor(user)?;
    let id: i64 = id.parse().map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(id).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open clone transaction", &e))?;
    let Some(source) = load_product_full(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load source product", &e))?
    else {
        return Err(not_found("product not found"));
    };

    let suffix = (Utc::now().timestamp_millis() % 100_000).abs();
    let base_slug = if source.slug.len() > 50 {
        &source.slug[..50]
    } else {
        &source.slug
    };
    let new_slug = format!("{base_slug}-copy-{suffix}");

    let new_id: i64 = sqlx::query_scalar(
        "INSERT INTO products \
             (slug, brand, model, price_cents, family, art_variant, art_hue, active, \
              stock_total, stock_reserved, stock_quarantined, image_day_url, image_lit_url, \
              designer, design_year, dimensions, light_source) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, 0, 0, 0, $8, $9, $10, $11, $12, $13) \
         RETURNING id",
    )
    .bind(&new_slug)
    .bind(&source.brand)
    .bind(format!("{} (Copy)", source.model))
    .bind(source.price_cents)
    .bind(&source.family)
    .bind(&source.art_variant)
    .bind(source.art_hue)
    .bind(&source.image_day_url)
    .bind(&source.image_lit_url)
    .bind(&source.designer)
    .bind(&source.design_year)
    .bind(&source.dimensions)
    .bind(&source.light_source)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to insert cloned product", &e))?;

    for (locale, fields) in &source.content {
        for (field, value) in fields {
            sqlx::query(
                "INSERT INTO product_content (product_id, locale, field, value) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(new_id)
            .bind(locale)
            .bind(field)
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to copy product content", &e))?;
        }
    }

    if let Some(ref_price) = &source.reference_price {
        sqlx::query(
            "INSERT INTO reference_prices \
                 (product_id, min_price_cents, max_price_cents, source_name, source_url, checked_on) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(new_id)
        .bind(ref_price.min_price_cents)
        .bind(ref_price.max_price_cents)
        .bind(&ref_price.source_name)
        .bind(&ref_price.source_url)
        .bind(ref_price.checked_on)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to copy reference price", &e))?;
    }

    insert_audit(
        &mut tx,
        &actor,
        "clone",
        "products",
        Some(&new_id.to_string()),
        serde_json::json!({ "source_id": id, "new_slug": new_slug }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit product clone", &e))?;

    let cloned = load_product_full(&mut tx, new_id)
        .await
        .map_err(|e| state::database_unavailable("failed to load cloned product", &e))?
        .ok_or_else(|| not_found("product not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit product clone", &e))?;

    Ok(Json(cloned))
}

/// `POST /api/admin/products/{id}/inspect-quarantine` — resolves quarantined
/// units after physical inspection (`"restock"` returns them to saleable
/// stock; `"write_off"` removes them from both `stock_quarantined` and
/// `stock_total`).
pub async fn inspect_quarantine(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    body: Result<Json<InspectQuarantineInput>, JsonRejection>,
) -> Result<Json<AdminProductFull>, ApiError> {
    let actor = actor(user)?;
    let id: i64 = id.parse().map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(id).map_err(bad_request)?;
    let input = json_body(body)?;
    if input.quantity < 1 {
        return Err(bad_request("quantity must be >= 1"));
    }
    if !matches!(input.action.as_str(), "restock" | "write_off") {
        return Err(bad_request("action must be \"restock\" or \"write_off\""));
    }
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open quarantine transaction", &e))?;
    let Some((stock_total, stock_quarantined)): Option<(i32, i32)> = sqlx::query_as(
        "SELECT stock_total, stock_quarantined FROM products WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to lock product", &e))?
    else {
        return Err(not_found("product not found"));
    };

    if stock_quarantined < input.quantity {
        return Err(conflict("insufficient quarantined stock"));
    }

    if input.action == "restock" {
        sqlx::query("UPDATE products SET stock_quarantined = stock_quarantined - $1 WHERE id = $2")
            .bind(input.quantity)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| state::database_unavailable("failed to restock from quarantine", &e))?;
    } else {
        sqlx::query(
            "UPDATE products SET stock_quarantined = stock_quarantined - $1, \
                 stock_total = stock_total - $1 WHERE id = $2",
        )
        .bind(input.quantity)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to write off quarantined stock", &e))?;
    }

    insert_audit(
        &mut tx,
        &actor,
        "inspect_quarantine",
        "products",
        Some(&id.to_string()),
        serde_json::json!({
            "action": input.action,
            "quantity": input.quantity,
            "previous_total": stock_total,
            "previous_quarantined": stock_quarantined,
        }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit quarantine inspection", &e))?;

    let updated = load_product_full(&mut tx, id)
        .await
        .map_err(|e| state::database_unavailable("failed to load updated product", &e))?
        .ok_or_else(|| not_found("product not found"))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit quarantine inspection", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Handlers: product_content
// ---------------------------------------------------------------------------

/// `PUT /api/admin/product-content` — upsert one (product, locale, field) row.
pub async fn upsert_product_content(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<ProductContentInput>, JsonRejection>,
) -> Result<Json<ProductContentInput>, ApiError> {
    let actor = actor(user)?;
    let input = json_body(body)?;
    validate_product_content(&input, &config::locales()).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open content transaction", &e))?;
    if !product_exists(&mut tx, input.product_id)
        .await
        .map_err(|e| state::database_unavailable("failed to check product", &e))?
    {
        return Err(not_found("product not found"));
    }
    let row = sqlx::query_as::<_, ProductContentInput>(
        "INSERT INTO product_content (product_id, locale, field, value) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (product_id, locale, field) DO UPDATE SET value = EXCLUDED.value \
         RETURNING product_id, locale, field, value",
    )
    .bind(input.product_id)
    .bind(&input.locale)
    .bind(&input.field)
    .bind(&input.value)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to upsert product content", &e))?;

    let entity_id = format!("{}/{}/{}", input.product_id, input.locale, input.field);
    let detail = serde_json::json!({
        "product_id": input.product_id,
        "locale": input.locale,
        "field": input.field,
        "value": input.value,
    });
    insert_audit(
        &mut tx,
        &actor,
        "upsert",
        "product_content",
        Some(&entity_id),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit content upsert", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit content upsert", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Handlers: reference_prices
// ---------------------------------------------------------------------------

/// `PUT /api/admin/reference-prices/{product_id}` — upsert the single verified
/// reference range for a product.
pub async fn upsert_reference_price(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(product_id): Path<String>,
    body: Result<Json<ReferencePriceInput>, JsonRejection>,
) -> Result<Json<AdminReferencePrice>, ApiError> {
    let actor = actor(user)?;
    let product_id: i64 = product_id
        .parse()
        .map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(product_id).map_err(bad_request)?;
    let input = json_body(body)?;
    validate_reference_price(&input).map_err(bad_request)?;
    // Validated above: the parse is infallible here (no unwrap in handlers).
    let checked_on = validate_checked_on(&input.checked_on).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open reference transaction", &e))?;
    if !product_exists(&mut tx, product_id)
        .await
        .map_err(|e| state::database_unavailable("failed to check product", &e))?
    {
        return Err(not_found("product not found"));
    }
    let row = sqlx::query_as::<_, AdminReferencePrice>(
        "INSERT INTO reference_prices \
             (product_id, min_price_cents, max_price_cents, source_name, source_url, checked_on) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (product_id) DO UPDATE SET \
             min_price_cents = EXCLUDED.min_price_cents, \
             max_price_cents = EXCLUDED.max_price_cents, \
             source_name = EXCLUDED.source_name, \
             source_url = EXCLUDED.source_url, \
             checked_on = EXCLUDED.checked_on \
         RETURNING product_id, min_price_cents, max_price_cents, source_name, source_url, checked_on",
    )
    .bind(product_id)
    .bind(input.min_price_cents)
    .bind(input.max_price_cents)
    .bind(&input.source_name)
    .bind(&input.source_url)
    .bind(checked_on)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to upsert reference price", &e))?;

    let detail = serde_json::json!({
        "product_id": product_id,
        "min_price_cents": input.min_price_cents,
        "max_price_cents": input.max_price_cents,
        "source_name": input.source_name,
        "source_url": input.source_url,
        "checked_on": input.checked_on,
    });
    insert_audit(
        &mut tx,
        &actor,
        "upsert",
        "reference_prices",
        Some(&product_id.to_string()),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit reference upsert", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit reference upsert", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

/// `DELETE /api/admin/reference-prices/{product_id}` — remove the verified
/// reference (= verification pending again).
pub async fn delete_reference_price(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(product_id): Path<String>,
) -> Result<Json<OkBody>, ApiError> {
    let actor = actor(user)?;
    let product_id: i64 = product_id
        .parse()
        .map_err(|_| bad_request("invalid product id"))?;
    validate_product_id(product_id).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open reference transaction", &e))?;
    let deleted = sqlx::query("DELETE FROM reference_prices WHERE product_id = $1")
        .bind(product_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to delete reference price", &e))?
        .rows_affected();
    if deleted == 0 {
        return Err(not_found("reference price not found"));
    }
    insert_audit(
        &mut tx,
        &actor,
        "delete",
        "reference_prices",
        Some(&product_id.to_string()),
        serde_json::json!({ "product_id": product_id }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit reference delete", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit reference delete", &e))?;

    fire_revalidation("catalogue");
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Handlers: pieces
// ---------------------------------------------------------------------------

/// `POST /api/admin/pieces` — number a new physical piece (DROP-01).
pub async fn create_piece(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<PieceInput>, JsonRejection>,
) -> Result<Json<AdminPiece>, ApiError> {
    let actor = actor(user)?;
    let input = json_body(body)?;
    validate_piece_numbers(input.piece_no, input.product_id).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open piece transaction", &e))?;
    if !product_exists(&mut tx, input.product_id)
        .await
        .map_err(|e| state::database_unavailable("failed to check product", &e))?
    {
        return Err(not_found("product not found"));
    }
    let row = sqlx::query_as::<_, AdminPiece>(
        "INSERT INTO pieces (piece_no, product_id) VALUES ($1, $2) \
         RETURNING piece_no, product_id",
    )
    .bind(input.piece_no)
    .bind(input.product_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        mutation_err(
            e,
            Some(("pieces_pkey", "piece_no already exists")),
            "failed to create piece",
        )
    })?;

    let detail = serde_json::json!({ "piece_no": input.piece_no, "product_id": input.product_id });
    insert_audit(
        &mut tx,
        &actor,
        "create",
        "pieces",
        Some(&input.piece_no.to_string()),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit piece create", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit piece create", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

/// `PATCH /api/admin/pieces/{piece_no}` — reassign a piece to another product.
pub async fn reassign_piece(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(piece_no): Path<String>,
    body: Result<Json<PiecePatch>, JsonRejection>,
) -> Result<Json<AdminPiece>, ApiError> {
    let actor = actor(user)?;
    let piece_no: i32 = piece_no
        .parse()
        .map_err(|_| bad_request("invalid piece_no"))?;
    let input = json_body(body)?;
    validate_piece_numbers(piece_no, input.product_id).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open piece transaction", &e))?;
    if !product_exists(&mut tx, input.product_id)
        .await
        .map_err(|e| state::database_unavailable("failed to check product", &e))?
    {
        return Err(not_found("product not found"));
    }
    let row = sqlx::query_as::<_, AdminPiece>(
        "UPDATE pieces SET product_id = $1 WHERE piece_no = $2 \
         RETURNING piece_no, product_id",
    )
    .bind(input.product_id)
    .bind(piece_no)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to reassign piece", &e))?
    .ok_or_else(|| not_found("piece not found"))?;

    let detail = serde_json::json!({ "piece_no": piece_no, "product_id": input.product_id });
    insert_audit(
        &mut tx,
        &actor,
        "update",
        "pieces",
        Some(&piece_no.to_string()),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit piece update", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit piece update", &e))?;

    fire_revalidation("catalogue");
    Ok(Json(row))
}

/// `DELETE /api/admin/pieces/{piece_no}` — remove a piece number from the drop.
pub async fn delete_piece(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(piece_no): Path<String>,
) -> Result<Json<OkBody>, ApiError> {
    let actor = actor(user)?;
    let piece_no: i32 = piece_no
        .parse()
        .map_err(|_| bad_request("invalid piece_no"))?;
    validate_piece_no(piece_no).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open piece transaction", &e))?;
    let deleted = sqlx::query("DELETE FROM pieces WHERE piece_no = $1")
        .bind(piece_no)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to delete piece", &e))?
        .rows_affected();
    if deleted == 0 {
        return Err(not_found("piece not found"));
    }
    insert_audit(
        &mut tx,
        &actor,
        "delete",
        "pieces",
        Some(&piece_no.to_string()),
        serde_json::json!({ "piece_no": piece_no }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit piece delete", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit piece delete", &e))?;

    fire_revalidation("catalogue");
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Handlers: site_content
// ---------------------------------------------------------------------------

/// `PUT /api/admin/site-content` — upsert one site copy (key, locale) row.
pub async fn upsert_site_content(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<SiteContentInput>, JsonRejection>,
) -> Result<Json<SiteContentInput>, ApiError> {
    let actor = actor(user)?;
    let input = json_body(body)?;
    validate_site_content(&input, &config::locales()).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open site-content transaction", &e))?;
    let row = sqlx::query_as::<_, SiteContentInput>(
        "INSERT INTO site_content (key, locale, value) VALUES ($1, $2, $3) \
         ON CONFLICT (key, locale) DO UPDATE SET value = EXCLUDED.value \
         RETURNING key, locale, value",
    )
    .bind(&input.key)
    .bind(&input.locale)
    .bind(&input.value)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to upsert site content", &e))?;

    let entity_id = format!("{}/{}", input.key, input.locale);
    let detail =
        serde_json::json!({ "key": input.key, "locale": input.locale, "value": input.value });
    insert_audit(
        &mut tx,
        &actor,
        "upsert",
        "site_content",
        Some(&entity_id),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit site-content upsert", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit site-content upsert", &e))?;

    fire_revalidation("site-content");
    Ok(Json(row))
}

/// `DELETE /api/admin/site-content/{key}/{locale}` — remove one copy row
/// (path segments: site content keys contain dots, and the row identity is
/// the (key, locale) pair).
pub async fn delete_site_content(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path((key, locale)): Path<(String, String)>,
) -> Result<Json<OkBody>, ApiError> {
    let actor = actor(user)?;
    // Same rules as the upsert minus the value: never delete under a key or
    // locale the write path would not have accepted.
    validate_site_content_key(&key, &locale, &config::locales()).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open site-content transaction", &e))?;
    let deleted = sqlx::query("DELETE FROM site_content WHERE key = $1 AND locale = $2")
        .bind(&key)
        .bind(&locale)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to delete site content", &e))?
        .rows_affected();
    if deleted == 0 {
        return Err(not_found("site content row not found"));
    }
    let entity_id = format!("{}/{}", key, locale);
    let detail = serde_json::json!({ "key": key, "locale": locale });
    insert_audit(
        &mut tx,
        &actor,
        "delete",
        "site_content",
        Some(&entity_id),
        detail,
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit site-content delete", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit site-content delete", &e))?;

    fire_revalidation("site-content");
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Handlers: settings
// ---------------------------------------------------------------------------

/// `PUT /api/admin/settings/{key}` — upsert a runtime override. Empty value
/// is the explicit OFF state for optional keys (CONFIG-01 presence-wins);
/// the audit detail logs the KEY and the new value (company keys, non-secret
/// — secret-looking keys never reach this code).
pub async fn upsert_setting(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(key): Path<String>,
    body: Result<Json<SettingInput>, JsonRejection>,
) -> Result<Json<AdminSetting>, ApiError> {
    let actor = actor(user)?;
    let input = json_body(body)?;
    validate_setting(&key, &input.value).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open settings transaction", &e))?;
    let row = sqlx::query_as::<_, AdminSetting>(
        "INSERT INTO settings (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now() \
         RETURNING key, value, updated_at",
    )
    .bind(&key)
    .bind(&input.value)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to upsert setting", &e))?;

    let detail = serde_json::json!({ "key": key, "value": input.value });
    insert_audit(&mut tx, &actor, "upsert", "settings", Some(&key), detail)
        .await
        .map_err(|e| state::database_unavailable("failed to audit settings upsert", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit settings upsert", &e))?;

    fire_revalidation("settings");
    Ok(Json(row))
}

/// `DELETE /api/admin/settings/{key}` — delete the override row: the key
/// reverts to its env default (CONFIG-01). The write whitelist applies to
/// deletes too — a delete is a settings write.
pub async fn delete_setting(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(key): Path<String>,
) -> Result<Json<OkBody>, ApiError> {
    let actor = actor(user)?;
    validate_setting_key(&key).map_err(bad_request)?;
    let pool = state::require_pool(&app)?;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open settings transaction", &e))?;
    let deleted = sqlx::query("DELETE FROM settings WHERE key = $1")
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to delete setting", &e))?
        .rows_affected();
    if deleted == 0 {
        return Err(not_found("settings override not found"));
    }
    insert_audit(
        &mut tx,
        &actor,
        "delete",
        "settings",
        Some(&key),
        serde_json::json!({ "key": key }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit settings delete", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit settings delete", &e))?;

    fire_revalidation("settings");
    Ok(ok())
}

// ---------------------------------------------------------------------------
// Handlers: audit log
// ---------------------------------------------------------------------------

/// `GET /api/admin/audit-log` — the last 200 audit rows, newest first
/// (`created_at DESC, id DESC` keeps ties deterministic).
pub async fn list_audit_log(
    State(app): State<AppState>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<Vec<AuditLogEntry>>,
    ),
    ApiError,
> {
    let pool = state::require_pool(&app)?;
    let rows = sqlx::query_as::<_, AuditLogEntry>(
        "SELECT id, actor, action, entity, entity_id, detail, created_at \
         FROM audit_log ORDER BY created_at DESC, id DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| state::database_unavailable("failed to load audit log", &e))?;
    // no-store: the audit trail is accountability data, never cacheable.
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(rows)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn locales() -> Vec<String> {
        vec!["en".to_string(), "zh-HK".to_string()]
    }

    // -- settings ---------------------------------------------------------

    #[test]
    fn validate_setting_key_accepts_the_whitelist() {
        for key in content::PUBLIC_SETTING_KEYS {
            assert_eq!(validate_setting_key(key), Ok(()), "{key}");
        }
    }

    #[test]
    fn validate_setting_key_rejects_keys_outside_the_whitelist() {
        assert_eq!(
            validate_setting_key("api.url"),
            Err("settings key not admin-editable")
        );
        assert_eq!(
            validate_setting_key("database.url"),
            Err("settings key not admin-editable")
        );
        assert_eq!(
            validate_setting_key(""),
            Err("settings key not admin-editable")
        );
        assert_eq!(
            validate_setting_key("brand.name."),
            Err("settings key not admin-editable")
        );
    }

    #[test]
    fn validate_setting_key_rejects_secret_looking_keys() {
        for key in [
            "session.secret",
            "smtp.password",
            "api.token",
            "stripe.credential",
            "vault.private",
            "SESSION.SECRET", // case-insensitive
            "my_Password",
            "brand.name_token",
        ] {
            assert_eq!(
                validate_setting_key(key),
                Err("settings key not admin-editable"),
                "{key}"
            );
        }
    }

    #[test]
    fn is_secret_looking_matches_only_secret_substrings() {
        assert!(is_secret_looking("session.secret"));
        assert!(is_secret_looking("SMTP.PASSWORD"));
        assert!(is_secret_looking("api.token"));
        assert!(is_secret_looking("x.credential"));
        assert!(is_secret_looking("a.private"));
        assert!(!is_secret_looking("brand.name"));
        assert!(!is_secret_looking("currency"));
        assert!(!is_secret_looking("contact.whatsapp"));
        assert!(!is_secret_looking("")); // empty is rejected elsewhere
    }

    #[test]
    fn validate_setting_keeps_required_fields_non_empty() {
        assert_eq!(validate_setting("brand.name", "ATELIER"), Ok(()));
        assert_eq!(
            validate_setting("brand.name", ""),
            Err("brand.name cannot be empty")
        );
        assert_eq!(
            validate_setting("brand.name", "   "),
            Err("brand.name cannot be empty")
        );
        assert_eq!(
            validate_setting("site.url", ""),
            Err("site.url cannot be empty")
        );
        assert_eq!(
            validate_setting("site.url", "https://shop.example.com"),
            Ok(())
        );
    }

    #[test]
    fn validate_setting_checks_currency_shape() {
        assert_eq!(validate_setting("currency", "HKD"), Ok(()));
        assert_eq!(validate_setting("currency", "USD"), Ok(()));
        for value in ["usd", "hk", "HKD1", "HK", "HKDD", "HK D", ""] {
            assert_eq!(
                validate_setting("currency", value),
                Err("currency must be three uppercase letters (ISO 4217)"),
                "{value}"
            );
        }
    }

    #[test]
    fn validate_setting_allows_explicit_off_for_optional_keys() {
        // CONFIG-01 presence-wins: an empty override is the OFF state.
        for key in [
            "brand.name_native",
            "contact.whatsapp",
            "analytics.ga_measurement_id",
            "analytics.gsc_verification",
        ] {
            assert_eq!(validate_setting(key, ""), Ok(()), "{key}");
        }
    }

    #[test]
    fn validate_currency_matches_the_iso_shape() {
        assert_eq!(validate_currency("HKD"), Ok(()));
        assert_eq!(validate_currency("ZZZ"), Ok(()));
        assert_eq!(
            validate_currency("hkd"),
            Err("currency must be three uppercase letters (ISO 4217)")
        );
        assert_eq!(
            validate_currency("HK1"),
            Err("currency must be three uppercase letters (ISO 4217)")
        );
        assert_eq!(
            validate_currency("HKDD"),
            Err("currency must be three uppercase letters (ISO 4217)")
        );
    }

    // -- products ---------------------------------------------------------

    #[test]
    fn validate_slug_input_accepts_valid_slugs() {
        assert_eq!(validate_slug_input("ph-5"), Ok(()));
        assert_eq!(validate_slug_input("artichoke-1958"), Ok(()));
    }

    #[test]
    fn validate_slug_input_rejects_invalid_slugs() {
        for slug in ["", "Ph-5", "ph_5", "ph 5", "ph.5"] {
            assert_eq!(
                validate_slug_input(slug),
                Err("slug must be 1-64 lowercase letters, digits or hyphens"),
                "{slug}"
            );
        }
    }

    #[test]
    fn validate_price_cents_accepts_zero_and_positive() {
        assert_eq!(validate_price_cents(0), Ok(()));
        assert_eq!(validate_price_cents(1), Ok(()));
        assert_eq!(validate_price_cents(i64::MAX), Ok(()));
    }

    #[test]
    fn validate_price_cents_rejects_negative() {
        assert_eq!(validate_price_cents(-1), Err("price_cents must be >= 0"));
        assert_eq!(
            validate_price_cents(i64::MIN),
            Err("price_cents must be >= 0")
        );
    }

    #[test]
    fn validate_art_variant_accepts_the_five_variants() {
        assert_eq!(validate_art_variant("dome"), Ok(()));
        assert_eq!(validate_art_variant("shades"), Ok(()));
        assert_eq!(validate_art_variant("shades-tall"), Ok(()));
        assert_eq!(validate_art_variant("piano"), Ok(()));
        assert_eq!(validate_art_variant("none"), Ok(()));
    }

    #[test]
    fn validate_art_variant_rejects_other_shapes() {
        for variant in [
            "",
            "orb",
            "Dome",
            "dome ",
            "shades,shades",
            "shadestall",
            "shades tall",
            "Piano",
            "None",
        ] {
            assert_eq!(
                validate_art_variant(variant),
                Err("art_variant must be one of \"dome\", \"shades\", \"shades-tall\", \"piano\", \"none\""),
                "{variant}"
            );
        }
    }

    #[test]
    fn validate_art_hue_accepts_the_full_circle() {
        assert_eq!(validate_art_hue(0), Ok(()));
        assert_eq!(validate_art_hue(35), Ok(()));
        assert_eq!(validate_art_hue(359), Ok(()));
    }

    #[test]
    fn validate_art_hue_rejects_out_of_range() {
        for hue in [-1, 360, 720, i32::MIN, i32::MAX] {
            assert_eq!(
                validate_art_hue(hue),
                Err("art_hue must be between 0 and 359"),
                "{hue}"
            );
        }
    }

    #[test]
    fn validate_product_create_accepts_a_full_row() {
        let input = CreateProduct {
            slug: "ph-5".to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "PH 5".to_string(),
            price_cents: 745_000,
            family: "ph-5".to_string(),
            art_variant: "dome".to_string(),
            art_hue: 35,
            active: true,
        };
        assert_eq!(validate_product_create(&input), Ok(()));
    }

    #[test]
    fn validate_product_create_rejects_each_bad_field() {
        let base = || CreateProduct {
            slug: "ph-5".to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "PH 5".to_string(),
            price_cents: 100,
            family: "".to_string(),
            art_variant: "dome".to_string(),
            art_hue: 35,
            active: true,
        };

        let mut bad = base();
        bad.slug = "Ph-5".to_string();
        assert!(validate_product_create(&bad).is_err());

        let mut bad = base();
        bad.brand = "  ".to_string();
        assert_eq!(validate_product_create(&bad), Err("brand cannot be empty"));

        let mut bad = base();
        bad.model = "".to_string();
        assert_eq!(validate_product_create(&bad), Err("model cannot be empty"));

        // Reviewer finding: NUL in brand/model on CREATE reached the DB.
        let mut bad = base();
        bad.brand = "B\u{0}".to_string();
        assert_eq!(
            validate_product_create(&bad),
            Err("value contains a NUL byte")
        );

        let mut bad = base();
        bad.model = "M\u{0}".to_string();
        assert_eq!(
            validate_product_create(&bad),
            Err("value contains a NUL byte")
        );

        let mut bad = base();
        bad.price_cents = -1;
        assert_eq!(
            validate_product_create(&bad),
            Err("price_cents must be >= 0")
        );

        let mut bad = base();
        bad.art_variant = "orb".to_string();
        assert!(validate_product_create(&bad).is_err());

        let mut bad = base();
        bad.art_hue = 360;
        assert!(validate_product_create(&bad).is_err());
    }

    #[test]
    fn validate_product_patch_accepts_partial_updates() {
        // Every single-field patch is valid; None fields are not checked.
        let patch = UpdateProduct {
            price_cents: Some(0),
            ..Default::default()
        };
        assert_eq!(validate_product_patch(&patch), Ok(()));

        let patch = UpdateProduct {
            active: Some(false),
            ..Default::default()
        };
        assert_eq!(validate_product_patch(&patch), Ok(()));

        let patch = UpdateProduct {
            art_variant: Some("shades".to_string()),
            art_hue: Some(0),
            ..Default::default()
        };
        assert_eq!(validate_product_patch(&patch), Ok(()));
    }

    #[test]
    fn validate_product_patch_rejects_invalid_some_fields() {
        let patch = UpdateProduct {
            slug: Some("BAD SLUG".to_string()),
            ..Default::default()
        };
        assert!(validate_product_patch(&patch).is_err());

        let patch = UpdateProduct {
            brand: Some("".to_string()),
            ..Default::default()
        };
        assert_eq!(validate_product_patch(&patch), Err("brand cannot be empty"));

        let patch = UpdateProduct {
            model: Some(" ".to_string()),
            ..Default::default()
        };
        assert_eq!(validate_product_patch(&patch), Err("model cannot be empty"));

        let patch = UpdateProduct {
            price_cents: Some(-5),
            ..Default::default()
        };
        assert_eq!(
            validate_product_patch(&patch),
            Err("price_cents must be >= 0")
        );

        let patch = UpdateProduct {
            art_hue: Some(360),
            ..Default::default()
        };
        assert!(validate_product_patch(&patch).is_err());
    }

    #[test]
    fn validate_product_patch_rejects_the_empty_patch() {
        // A patch changing nothing must not fabricate an audit/webhook event.
        assert_eq!(
            validate_product_patch(&UpdateProduct::default()),
            Err("no fields to update")
        );
    }

    // -- UpdateProductFull (the flagship editor's single save) ------------

    /// A valid complete request: the happy shape every rejection test below
    /// mutates exactly one field of.
    fn full_input() -> UpdateProductFull {
        UpdateProductFull {
            slug: "ph-1-1-piano-table-limited".to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "PH 1/1 Piano Table".to_string(),
            price_cents: 1_079_500,
            family: "ph-table".to_string(),
            art_variant: "piano".to_string(),
            art_hue: 45,
            active: true,
            stock_total: Some(1),
            stock_quarantined: Some(0),
            image_day_url: "https://cdn.example.com/day.webp".to_string(),
            image_lit_url: "https://cdn.example.com/lit.png".to_string(),
            designer: "Poul Henningsen".to_string(),
            design_year: "1931 / 2026 Collector Issue".to_string(),
            dimensions: "Ø 140 × H 350 mm".to_string(),
            light_source: "Integrated Warm LED · 2700K".to_string(),
            content: BTreeMap::from([(
                "zh-HK".to_string(),
                BTreeMap::from([("heritage".to_string(), "限量復刻。".to_string())]),
            )]),
            reference_price: Some(ReferencePriceInput {
                min_price_cents: 100,
                max_price_cents: 200,
                source_name: "Some Shop".to_string(),
                source_url: "https://shop.example.com/x".to_string(),
                checked_on: "2026-02-28".to_string(),
            }),
            expected_version_hash: None,
            expected_stock_total: None,
        }
    }

    #[test]
    fn validate_product_full_accepts_a_complete_request() {
        assert_eq!(validate_product_full(&full_input(), &locales()), Ok(()));
        // Both optional counts absent = keep the row's counts; no reference
        // price = delete the row (verification pending again).
        let minimal = UpdateProductFull {
            stock_total: None,
            stock_quarantined: None,
            content: BTreeMap::new(),
            reference_price: None,
            ..full_input()
        };
        assert_eq!(validate_product_full(&minimal, &locales()), Ok(()));
    }

    #[test]
    fn validate_product_full_rejects_each_bad_field() {
        let mut bad = full_input();
        bad.slug = "Ph-1/1".to_string();
        assert!(validate_product_full(&bad, &locales()).is_err());

        let mut bad = full_input();
        bad.brand = " ".to_string();
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("brand cannot be empty")
        );

        let mut bad = full_input();
        bad.model = "".to_string();
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("model cannot be empty")
        );

        let mut bad = full_input();
        bad.price_cents = -1;
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("price_cents must be >= 0")
        );

        let mut bad = full_input();
        bad.art_variant = "shadestall".to_string();
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("art_variant must be one of \"dome\", \"shades\", \"shades-tall\", \"piano\", \"none\"")
        );

        let mut bad = full_input();
        bad.art_hue = 360;
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("art_hue must be between 0 and 359")
        );

        let mut bad = full_input();
        bad.stock_total = Some(-1);
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("stock_total must be >= 0")
        );

        let mut bad = full_input();
        bad.stock_quarantined = Some(-1);
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("stock_quarantined must be >= 0")
        );

        // A NUL byte can reach any free-text field: never past validation.
        for field in [
            "image_day_url",
            "image_lit_url",
            "designer",
            "design_year",
            "dimensions",
            "light_source",
        ] {
            let mut bad = full_input();
            match field {
                "image_day_url" => bad.image_day_url = "a\u{0}".to_string(),
                "image_lit_url" => bad.image_lit_url = "a\u{0}".to_string(),
                "designer" => bad.designer = "a\u{0}".to_string(),
                "design_year" => bad.design_year = "a\u{0}".to_string(),
                "dimensions" => bad.dimensions = "a\u{0}".to_string(),
                "light_source" => bad.light_source = "a\u{0}".to_string(),
                _ => unreachable!(),
            }
            assert_eq!(
                validate_product_full(&bad, &locales()),
                Err("value contains a NUL byte"),
                "{field}"
            );
        }

        // A bad reference price is a full-request rejection, exactly like the
        // single-row reference endpoint.
        let mut bad = full_input();
        bad.reference_price = Some(ReferencePriceInput {
            min_price_cents: 201,
            max_price_cents: 200,
            source_name: "Some Shop".to_string(),
            source_url: "https://shop.example.com/x".to_string(),
            checked_on: "2026-02-28".to_string(),
        });
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("min_price_cents must be <= max_price_cents")
        );
    }

    #[test]
    fn validate_product_full_rejects_bad_content_entries() {
        let mut bad = full_input();
        bad.content = BTreeMap::from([(
            "fr".to_string(),
            BTreeMap::from([("heritage".to_string(), "x".to_string())]),
        )]);
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("locale not in the configured locale set")
        );

        let mut bad = full_input();
        bad.content = BTreeMap::from([(
            "en".to_string(),
            BTreeMap::from([("price".to_string(), "x".to_string())]),
        )]);
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("field must be one of model_alt, colourway, heritage, electrical")
        );

        // A blank value is LEGAL in the full save: it is the form clearing
        // that copy slot (the handler deletes the stored row). NUL bytes stay
        // rejected below.
        let mut blank = full_input();
        blank.content = BTreeMap::from([(
            "en".to_string(),
            BTreeMap::from([("electrical".to_string(), "  ".to_string())]),
        )]);
        assert_eq!(validate_product_full(&blank, &locales()), Ok(()));

        let mut bad = full_input();
        bad.content = BTreeMap::from([(
            "en".to_string(),
            BTreeMap::from([("electrical".to_string(), "a\u{0}".to_string())]),
        )]);
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("value contains a NUL byte")
        );

        // One bad entry among good ones rejects the WHOLE request (the editor
        // posts the complete map; a silently skipped row would lose copy).
        let mut bad = full_input();
        bad.content.insert(
            "en".to_string(),
            BTreeMap::from([("colourway".to_string(), "Chrome".to_string())]),
        );
        bad.content.insert(
            "fr".to_string(),
            BTreeMap::from([("colourway".to_string(), "Chrome".to_string())]),
        );
        assert_eq!(
            validate_product_full(&bad, &locales()),
            Err("locale not in the configured locale set")
        );
    }

    #[test]
    fn admin_product_full_json_has_exact_keys() {
        // The flagship editor's response contract: exactly these keys, in one
        // aggregate (pinned so a field cannot silently vanish from the form).
        let product = AdminProductFull {
            id: 8,
            slug: "ph-1-1-piano-table-limited".to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "PH 1/1 Piano Table".to_string(),
            price_cents: 1_079_500,
            family: "ph-table".to_string(),
            art_variant: "piano".to_string(),
            art_hue: 45,
            active: true,
            stock_total: 1,
            stock_reserved: 0,
            stock_quarantined: 0,
            image_day_url: "https://cdn.example.com/day.webp".to_string(),
            image_lit_url: "https://cdn.example.com/lit.png".to_string(),
            designer: "Poul Henningsen".to_string(),
            design_year: "1931 / 2026 Collector Issue".to_string(),
            dimensions: "Ø 140 × H 350 mm".to_string(),
            light_source: "Integrated Warm LED · 2700K".to_string(),
            created_at: Utc::now(),
            content: BTreeMap::new(),
            reference_price: None,
            pieces: vec![16],
            version_hash: "0123456789abcdef".to_string(),
        };
        let value = serde_json::to_value(&product).unwrap();
        let keys: std::collections::BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let expected: std::collections::BTreeSet<&str> = [
            "id",
            "slug",
            "brand",
            "model",
            "price_cents",
            "family",
            "art_variant",
            "art_hue",
            "active",
            "stock_total",
            "stock_reserved",
            "stock_quarantined",
            "image_day_url",
            "image_lit_url",
            "designer",
            "design_year",
            "dimensions",
            "light_source",
            "created_at",
            "content",
            "reference_price",
            "pieces",
            "version_hash",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected);
        // The aggregate is the ADMIN view: unlike the catalogue it does carry
        // stock, and a missing reference price stays null (verification
        // pending), never invented.
        assert_eq!(value["stock_quarantined"], 0);
        assert!(value["reference_price"].is_null());
    }

    // -- version hash (stale-tab concurrency guard) ------------------------

    /// A fixed, fully-populated state the hash tests mutate one field at a
    /// time. Deliberately includes content rows, a reference price and text
    /// that could collide with a naive delimiter join.
    fn version_state() -> ProductVersionState {
        ProductVersionState {
            slug: "ph-5".to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "PH 5".to_string(),
            price_cents: 745_000,
            family: "ph-5".to_string(),
            art_variant: "dome".to_string(),
            art_hue: 35,
            active: true,
            image_day_url: "https://cdn.example.com/day.webp".to_string(),
            image_lit_url: "https://cdn.example.com/lit.webp".to_string(),
            designer: "Poul Henningsen".to_string(),
            design_year: "1958".to_string(),
            dimensions: "Ø 500 mm".to_string(),
            light_source: "E27 · 2700K".to_string(),
            content: BTreeMap::from([(
                "en".to_string(),
                BTreeMap::from([("colourway".to_string(), "White".to_string())]),
            )]),
            reference_price: Some(AdminReferencePrice {
                product_id: 1,
                min_price_cents: 100,
                max_price_cents: 200,
                source_name: "Some Shop".to_string(),
                source_url: "https://shop.example.com/x".to_string(),
                checked_on: NaiveDate::from_ymd_opt(2026, 2, 28).unwrap(),
            }),
        }
    }

    #[test]
    fn version_hash_is_deterministic_and_content_sensitive() {
        let state = version_state();
        let hash = compute_product_version_hash(&state);
        assert_eq!(hash.len(), 16, "the contract pins a 16-char prefix");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex only: {hash}"
        );
        assert_eq!(
            compute_product_version_hash(&version_state()),
            hash,
            "the same state must always hash the same"
        );

        // Every hashed dimension must move the fingerprint. Stock is NOT one
        // of them (it is not part of ProductVersionState) — that is exactly
        // why a customer order cannot invalidate an open admin tab.
        type Mutation = fn(&mut ProductVersionState);
        let mutations: &[Mutation] = &[
            |s| s.slug = "ph-5x".to_string(),
            |s| s.brand = "Other".to_string(),
            |s| s.model = "PH 6".to_string(),
            |s| s.price_cents += 1,
            |s| s.family = "other".to_string(),
            |s| s.art_variant = "shades".to_string(),
            |s| s.art_hue = 36,
            |s| s.active = false,
            |s| s.image_day_url = "day2".to_string(),
            |s| s.image_lit_url = "lit2".to_string(),
            |s| s.designer = "Other".to_string(),
            |s| s.design_year = "1959".to_string(),
            |s| s.dimensions = "other".to_string(),
            |s| s.light_source = "other".to_string(),
            |s| {
                s.content
                    .get_mut("en")
                    .unwrap()
                    .insert("colourway".to_string(), "Black".to_string());
            },
            |s| {
                s.content
                    .entry("zh-HK".to_string())
                    .or_default()
                    .insert("heritage".to_string(), "傳承".to_string());
            },
            |s| s.reference_price = None,
            |s| s.reference_price.as_mut().unwrap().min_price_cents = 101,
        ];
        for mutate in mutations {
            let mut changed = version_state();
            mutate(&mut changed);
            assert_ne!(
                compute_product_version_hash(&changed),
                hash,
                "a changed field must move the fingerprint"
            );
        }
    }

    #[test]
    fn version_hash_has_a_pinned_known_answer() {
        // Computed once from the fixed `version_state()` and frozen here:
        // the encoding (order, length prefixes, selection of fields) is a
        // client-visible contract (`row_version_hash` in the CSV and
        // `expected_version_hash` on save), so a refactor that silently
        // changes it must fail this test, not invalidate every open tab.
        assert_eq!(
            compute_product_version_hash(&version_state()),
            "01c3c978a3455d37"
        );
    }

    #[test]
    fn validate_full_stock_keeps_current_counts_when_absent() {
        assert_eq!(validate_full_stock(None, None, 16, 12, 3), Ok((16, 3)));
        assert_eq!(validate_full_stock(Some(15), None, 16, 12, 3), Ok((15, 3)));
        assert_eq!(validate_full_stock(None, Some(4), 16, 12, 3), Ok((16, 4)));
    }

    #[test]
    fn validate_full_stock_accepts_the_exact_boundary() {
        // stock_total == stock_reserved + stock_quarantined is the tightest
        // legal state (STOCK-01: no oversell, no silent un-quarantine).
        assert_eq!(
            validate_full_stock(Some(16), Some(4), 16, 12, 3),
            Ok((16, 4))
        );
    }

    #[test]
    fn validate_full_stock_rejects_totals_below_reserved_plus_quarantined() {
        assert_eq!(
            validate_full_stock(Some(11), Some(4), 16, 12, 3),
            Err("stock_total must be >= stock_reserved + stock_quarantined")
        );
        // The invariant holds even when stock_total is untouched: raising the
        // quarantine count above the remaining stock is a rejection too.
        assert_eq!(
            validate_full_stock(None, Some(5), 16, 12, 3),
            Err("stock_total must be >= stock_reserved + stock_quarantined")
        );
    }

    #[test]
    fn validate_full_stock_rejects_negative_counts() {
        assert_eq!(
            validate_full_stock(Some(-1), None, 16, 12, 3),
            Err("stock_total must be >= 0")
        );
        assert_eq!(
            validate_full_stock(None, Some(-1), 16, 12, 3),
            Err("stock_quarantined must be >= 0")
        );
    }

    #[test]
    fn validate_full_stock_never_overflows_the_comparison() {
        // i64 arithmetic inside: two large i32 counts add up exactly instead
        // of wrapping into an accidental pass.
        assert_eq!(
            validate_full_stock(Some(1), Some(1), 5, i32::MAX, i32::MAX),
            Err("stock_total must be >= stock_reserved + stock_quarantined")
        );
    }

    // -- product_content --------------------------------------------------

    #[test]
    fn validate_product_content_accepts_valid_input() {
        let input = ProductContentInput {
            product_id: 1,
            locale: "zh-HK".to_string(),
            field: "heritage".to_string(),
            value: "1958 年設計".to_string(),
        };
        assert_eq!(validate_product_content(&input, &locales()), Ok(()));
    }

    #[test]
    fn validate_product_content_rejects_bad_input() {
        let base = || ProductContentInput {
            product_id: 1,
            locale: "en".to_string(),
            field: "colourway".to_string(),
            value: "white".to_string(),
        };

        let mut bad = base();
        bad.product_id = 0;
        assert_eq!(
            validate_product_content(&bad, &locales()),
            Err("product_id must be >= 1")
        );

        let mut bad = base();
        bad.field = "price".to_string();
        assert_eq!(
            validate_product_content(&bad, &locales()),
            Err("field must be one of model_alt, colourway, heritage, electrical")
        );

        let mut bad = base();
        bad.locale = "fr".to_string();
        assert_eq!(
            validate_product_content(&bad, &locales()),
            Err("locale not in the configured locale set")
        );

        let mut bad = base();
        bad.value = "   ".to_string();
        assert_eq!(
            validate_product_content(&bad, &locales()),
            Err("value cannot be empty")
        );
    }

    // -- reference_prices -------------------------------------------------

    #[test]
    fn validate_reference_price_accepts_a_verified_range() {
        let input = ReferencePriceInput {
            min_price_cents: 600_000,
            max_price_cents: 800_000,
            source_name: "Nordic Nest".to_string(),
            source_url: "https://www.nordicnest.com/louis-poulsen/ph-5/".to_string(),
            checked_on: "2026-02-28".to_string(),
        };
        assert_eq!(validate_reference_price(&input), Ok(()));
        // http:// is fine too.
        let input = ReferencePriceInput {
            source_url: "http://shop.example.com/ph-5".to_string(),
            ..input
        };
        assert_eq!(validate_reference_price(&input), Ok(()));
    }

    #[test]
    fn validate_reference_price_rejects_bad_ranges_and_sources() {
        let base = || ReferencePriceInput {
            min_price_cents: 100,
            max_price_cents: 200,
            source_name: "Some Shop".to_string(),
            source_url: "https://shop.example.com/x".to_string(),
            checked_on: "2026-02-28".to_string(),
        };

        let mut bad = base();
        bad.min_price_cents = -1;
        assert_eq!(
            validate_reference_price(&bad),
            Err("min_price_cents must be >= 0")
        );

        let mut bad = base();
        bad.max_price_cents = -1;
        assert_eq!(
            validate_reference_price(&bad),
            Err("max_price_cents must be >= 0")
        );

        let mut bad = base();
        bad.min_price_cents = 201;
        assert_eq!(
            validate_reference_price(&bad),
            Err("min_price_cents must be <= max_price_cents")
        );

        let mut bad = base();
        bad.source_name = " ".to_string();
        assert_eq!(
            validate_reference_price(&bad),
            Err("source_name cannot be empty")
        );

        let mut bad = base();
        bad.source_url = "shop.example.com".to_string();
        assert!(validate_reference_price(&bad).is_err());

        let mut bad = base();
        bad.checked_on = "2026-02-30".to_string();
        assert!(validate_reference_price(&bad).is_err());
    }

    #[test]
    fn validate_checked_on_accepts_real_dates() {
        assert_eq!(
            validate_checked_on("2026-02-28"),
            Ok(NaiveDate::from_ymd_opt(2026, 2, 28).unwrap())
        );
        assert_eq!(
            validate_checked_on("2024-02-29"),
            Ok(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()) // leap year
        );
    }

    #[test]
    fn validate_checked_on_rejects_fake_dates() {
        for raw in [
            "2026-02-30", // not a calendar date
            "2026-13-01", // no month 13
            "2026-00-10",
            "2023-02-29", // not a leap year
            "2026-2-3",   // wrong width
            "2026-2-28",
            "28-02-2026",  // wrong order
            "2026/02/28",  // wrong separators
            "2026-02-28 ", // trailing junk
            "",
            "not-a-date",
        ] {
            assert_eq!(
                validate_checked_on(raw),
                Err("checked_on must be a real date in YYYY-MM-DD form"),
                "{raw}"
            );
        }
    }

    #[test]
    fn validate_source_url_accepts_absolute_http_urls() {
        assert_eq!(
            validate_source_url("https://www.nordicnest.com/x"),
            Ok("https://www.nordicnest.com/x")
        );
        assert_eq!(
            validate_source_url("http://shop.example.com"),
            Ok("http://shop.example.com")
        );
        assert_eq!(
            validate_source_url(" https://shop.example.com/x?q=1 "),
            Ok("https://shop.example.com/x?q=1") // trimmed
        );
    }

    #[test]
    fn validate_source_url_rejects_empty_host_and_bad_port() {
        // Reviewer finding: both of these previously reached the DB.
        assert_eq!(
            validate_source_url("https://:443/x"),
            Err("source_url must be an absolute http:// or https:// URL")
        );
        assert_eq!(
            validate_source_url("https://shop.example:notaport/x"),
            Err("source_url must be an absolute http:// or https:// URL")
        );
        // Valid explicit ports stay fine.
        assert_eq!(
            validate_source_url("http://shop.example:8080/x"),
            Ok("http://shop.example:8080/x")
        );
    }

    #[test]
    fn reject_nul_rejects_nul_bytes() {
        assert_eq!(reject_nul("ok"), Ok(()));
        assert_eq!(reject_nul("bad\u{0}"), Err("value contains a NUL byte"));
        // The explicit OFF empty string stays legal (CONFIG-01 presence-wins).
        assert_eq!(reject_nul(""), Ok(()));
    }

    #[test]
    fn validate_source_url_rejects_non_urls() {
        for raw in [
            "",
            "shop.example.com",
            "//shop.example.com",
            "ftp://shop.example.com",
            "file:///etc/passwd",
            "http://",
            "https://",
            "http:// /x",
            "http://user:pass@shop.example.com", // no credentials in URLs
        ] {
            assert_eq!(
                validate_source_url(raw),
                Err("source_url must be an absolute http:// or https:// URL"),
                "{raw}"
            );
        }
    }

    // -- pieces / site_content / ids --------------------------------------

    #[test]
    fn locale_allowed_matches_the_configured_set() {
        assert!(locale_allowed("en", &locales()));
        assert!(locale_allowed("zh-HK", &locales()));
        assert!(!locale_allowed("fr", &locales()));
        assert!(!locale_allowed("EN", &locales())); // exact match, case-sensitive
        assert!(!locale_allowed("", &locales()));
        assert!(!locale_allowed("en", &[])); // empty configured set
    }

    #[test]
    fn validate_piece_no_accepts_positive_numbers() {
        assert_eq!(validate_piece_no(1), Ok(()));
        assert_eq!(validate_piece_no(16), Ok(()));
        assert_eq!(validate_piece_no(i32::MAX), Ok(()));
    }

    #[test]
    fn validate_piece_no_rejects_zero_and_negative() {
        assert_eq!(validate_piece_no(0), Err("piece_no must be >= 1"));
        assert_eq!(validate_piece_no(-1), Err("piece_no must be >= 1"));
        assert_eq!(validate_piece_no(i32::MIN), Err("piece_no must be >= 1"));
    }

    #[test]
    fn validate_product_id_accepts_positive_ids() {
        assert_eq!(validate_product_id(1), Ok(()));
        assert_eq!(validate_product_id(i64::MAX), Ok(()));
    }

    #[test]
    fn validate_product_id_rejects_zero_and_negative() {
        assert_eq!(validate_product_id(0), Err("product_id must be >= 1"));
        assert_eq!(validate_product_id(-1), Err("product_id must be >= 1"));
    }

    #[test]
    fn validate_piece_numbers_accepts_real_pieces() {
        assert_eq!(validate_piece_numbers(1, 1), Ok(()));
        assert_eq!(validate_piece_numbers(16, 8), Ok(()));
    }

    #[test]
    fn validate_piece_numbers_rejects_bad_numbers() {
        assert_eq!(validate_piece_numbers(0, 1), Err("piece_no must be >= 1"));
        assert_eq!(validate_piece_numbers(-3, 1), Err("piece_no must be >= 1"));
        assert_eq!(validate_piece_numbers(1, 0), Err("product_id must be >= 1"));
    }

    #[test]
    fn validate_site_content_key_accepts_valid_rows() {
        assert_eq!(
            validate_site_content_key("tagline", "en", &locales()),
            Ok(())
        );
        assert_eq!(
            validate_site_content_key("policy.warranty", "zh-HK", &locales()),
            Ok(())
        );
    }

    #[test]
    fn validate_site_content_key_rejects_bad_keys_and_locales() {
        assert_eq!(
            validate_site_content_key(" ", "en", &locales()),
            Err("key cannot be empty")
        );
        assert_eq!(
            validate_site_content_key("", "en", &locales()),
            Err("key cannot be empty")
        );
        assert_eq!(
            validate_site_content_key("tagline", "fr", &locales()),
            Err("locale not in the configured locale set")
        );
        // Reviewer finding: the delete path shared this validator without a
        // NUL check — pin the shared behavior.
        assert_eq!(
            validate_site_content_key("tag\u{0}line", "en", &locales()),
            Err("value contains a NUL byte")
        );
    }

    #[test]
    fn validate_site_content_accepts_valid_input() {
        let input = SiteContentInput {
            key: "policy.warranty".to_string(),
            locale: "en".to_string(),
            value: "Pending".to_string(),
        };
        assert_eq!(validate_site_content(&input, &locales()), Ok(()));
    }

    #[test]
    fn validate_site_content_rejects_bad_input() {
        let base = || SiteContentInput {
            key: "tagline".to_string(),
            locale: "en".to_string(),
            value: "Curated designer homeware".to_string(),
        };

        let mut bad = base();
        bad.key = " ".to_string();
        assert_eq!(
            validate_site_content(&bad, &locales()),
            Err("key cannot be empty")
        );

        let mut bad = base();
        bad.locale = "fr".to_string();
        assert_eq!(
            validate_site_content(&bad, &locales()),
            Err("locale not in the configured locale set")
        );

        let mut bad = base();
        bad.value = "".to_string();
        assert_eq!(
            validate_site_content(&bad, &locales()),
            Err("value cannot be empty")
        );
    }

    // -- webhook helpers --------------------------------------------------

    #[test]
    fn webhook_target_splits_host_port_and_target() {
        let t = webhook_target("http://localhost:3000/api/revalidate").unwrap();
        assert_eq!(t.host, "localhost");
        assert_eq!(t.port, 3000);
        assert_eq!(t.target, "/api/revalidate");
        assert_eq!(t.authority, "localhost:3000");

        let t = webhook_target("http://web:3000").unwrap();
        assert_eq!(t.target, "/");
        assert_eq!(t.authority, "web:3000");

        let t = webhook_target("http://127.0.0.1:8080/api/revalidate?x=1").unwrap();
        assert_eq!(t.host, "127.0.0.1");
        assert_eq!(t.port, 8080);
        assert_eq!(t.target, "/api/revalidate?x=1");

        // No port -> HTTP default 80; bare query still gets a path.
        let t = webhook_target("http://web").unwrap();
        assert_eq!(t.port, 80);
        let t = webhook_target("http://web?x=1").unwrap();
        assert_eq!(t.target, "/?x=1");

        // A fragment is never sent in a request line (RFC 9110): dropped,
        // never target text.
        let t = webhook_target("http://web#frag").unwrap();
        assert_eq!(t.target, "/");
        let t = webhook_target("http://web/x#frag").unwrap();
        assert_eq!(t.target, "/x");
        // Query AND fragment: fragment dropped, query kept.
        let t = webhook_target("http://web/x?q=1#f").unwrap();
        assert_eq!(t.target, "/x?q=1");
    }

    #[test]
    fn webhook_target_rejects_bad_urls() {
        assert!(webhook_target("").is_none());
        assert!(webhook_target("https://web/x").is_none()); // no TLS client
        assert!(webhook_target("web:3000/x").is_none()); // no scheme
        assert!(webhook_target("http://").is_none()); // no host
        assert!(webhook_target("http://:3000/x").is_none()); // empty host
        assert!(webhook_target("http:// /x").is_none()); // whitespace authority
        assert!(webhook_target("http://user:pass@web/x").is_none()); // no credentials
        assert!(webhook_target("http://web:notaport/x").is_none()); // bad port
        assert!(webhook_target("http://web:99999/x").is_none()); // port out of range
    }

    #[test]
    fn webhook_body_carries_secret_and_tags() {
        let body: serde_json::Value =
            serde_json::from_str(&webhook_body("s3cr3t", "catalogue")).unwrap();
        assert_eq!(body["secret"], "s3cr3t");
        assert_eq!(body["tags"], serde_json::json!(["catalogue"]));
    }
}
