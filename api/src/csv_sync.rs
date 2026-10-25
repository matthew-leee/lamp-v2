//! 1-Click Excel/CSV Bulk Sync (DATA-01 / ADMIN-01).
//!
//! Six routes, all mounted behind `require_admin` + CSRF in `main.rs`:
//!
//! * `GET  /api/admin/csv/products`             — [export_products_csv]
//! * `POST /api/admin/csv/products/preview`     — [preview_products_csv]
//! * `POST /api/admin/csv/products/commit`      — [commit_products_csv]
//! * `GET  /api/admin/csv/site-content`         — [export_site_content_csv]
//! * `POST /api/admin/csv/site-content/preview` — [preview_site_content_csv]
//! * `POST /api/admin/csv/site-content/commit`  — [commit_site_content_csv]
//!
//! Contract, and WHY each piece exists:
//!
//! 1. **Excel-first export.** Every export starts with a UTF-8 BOM so
//!    Microsoft Excel detects UTF-8 and Traditional Chinese copy stops
//!    mojibaking; line endings are CRLF (RFC 4180). Piece numbers are written
//!    `#01;#02` (never `01-02`) because Excel silently rewrites `01-02` into
//!    a date (`Jan-02`), corrupting the drop numbering on round-trip.
//! 2. **Dry-run preview, then atomic commit.** The preview is the same pure
//!    planning pass the commit runs: rows, per-row diffs, validation errors
//!    and stock conflicts are shown BEFORE anything is written. Commit plans
//!    inside its transaction and writes everything (products, content,
//!    pieces, audit row) or nothing.
//! 3. **Concurrent-order stock protection.** The export carries
//!    `snapshot_stock_total` (the live count at export time) and
//!    `stock_total`/`stock_delta` (what the partner edits). A CSV that does
//!    not edit stock can NEVER clobber a live count: the effective stock is
//!    the locked database value. When the operator DID edit stock, the live
//!    database count is compared with the snapshot; on a mismatch the
//!    configured `stock_conflict_mode` resolves it and the row reports a
//!    `stock_conflict` warning. `stock_total` must always stay
//!    `>= stock_reserved + stock_quarantined` (STOCK-01).
//! 4. **Scientific notation is rejected.** Excel can present a large integer
//!    as `1.23E+10`; importing that would silently divide/mangle a price or
//!    stock count, so any such cell is a row error instead.
//! 5. **One audit row per committed sync** (`csv_bulk_sync`, in the same
//!    transaction) plus the `catalogue` / `site-content` revalidation
//!    webhook on the committed tags (commitment 5b).
//!
//! JSON field names are snake_case, exactly like the rest of the admin API.

use std::collections::{BTreeMap, HashMap};

use axum::{
    body::Body,
    extract::{rejection::JsonRejection, Extension, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};

use crate::admin::CurrentUser;
use crate::admin_crud;
use crate::config;
use crate::state::{self, ApiError, AppState, ErrorBody};

// ---------------------------------------------------------------------------
// Shared constants / helpers
// ---------------------------------------------------------------------------

/// UTF-8 BOM (`U+FEFF`) that prefixes every export so Excel detects UTF-8.
pub const CSV_BOM: char = '\u{FEFF}';

/// Exact export header for the products CSV: 30 columns. `pieces` sits
/// between the spec fields and the locale copy so the whole product record
/// reads left-to-right like the Admin Studio form.
///
/// Import is order-insensitive but requires every column except `pieces`
/// (which older/hand-built files may omit; omitting it leaves the drop
/// numbering untouched).
pub const PRODUCTS_CSV_COLUMNS: &[&str] = &[
    "id",
    "slug",
    "brand",
    "model",
    "family",
    "active",
    "price_hkd",
    "snapshot_stock_total",
    "stock_total",
    "stock_delta",
    "stock_reserved",
    "stock_quarantined",
    "art_variant",
    "art_hue",
    "image_day_url",
    "image_lit_url",
    "designer",
    "design_year",
    "dimensions",
    "light_source",
    "pieces",
    "en_model_alt",
    "en_colourway",
    "en_heritage",
    "en_electrical",
    "zh_hk_model_alt",
    "zh_hk_colourway",
    "zh_hk_heritage",
    "zh_hk_electrical",
    "row_version_hash",
];

/// The locale-carrying columns: (CSV column, locale, `product_content.field`).
/// The pair is the fixed en/zh-HK contract (LOCALE-01) pinned by the export
/// header; `site-content` uses `config::locales()` instead.
const CONTENT_COLUMNS: &[(&str, &str, &str)] = &[
    ("en_model_alt", "en", "model_alt"),
    ("en_colourway", "en", "colourway"),
    ("en_heritage", "en", "heritage"),
    ("en_electrical", "en", "electrical"),
    ("zh_hk_model_alt", "zh-HK", "model_alt"),
    ("zh_hk_colourway", "zh-HK", "colourway"),
    ("zh_hk_heritage", "zh-HK", "heritage"),
    ("zh_hk_electrical", "zh-HK", "electrical"),
];

/// Stock conflict modes. The first three are the canonical contract; the last
/// three are compatibility aliases for the Next.js proxy's option names:
/// `skip` == `keep_db`, `overwrite` == `force_override`, and `snapshot_aware`
/// applies the CSV count only when the live count still equals the snapshot.
const STOCK_CONFLICT_MODES: &[&str] = &[
    "keep_db",
    "apply_delta",
    "force_override",
    "snapshot_aware",
    "overwrite",
    "skip",
];

fn default_stock_conflict_mode() -> String {
    "keep_db".to_string()
}

fn push_error(errors: &mut Vec<CsvRowError>, row: usize, column: &str, message: impl Into<String>) {
    errors.push(CsvRowError {
        row,
        column: column.to_string(),
        message: message.into(),
    });
}

/// 400: validation rejected the request (same body shape as `admin_crud`).
fn bad_request(msg: &str) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: msg.to_string(),
        }),
    )
}

/// 500 for the impossible response-builder failures; never used for a DB
/// error (those keep the generic 503 body via [`state::database_unavailable`]).
fn internal_error(msg: &str) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: msg.to_string(),
        }),
    )
}

/// The acting admin, from the `CurrentUser` the auth middleware inserted.
fn actor(user: Option<Extension<CurrentUser>>) -> Result<String, ApiError> {
    user.map(|Extension(user)| user.username).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "not authenticated".to_string(),
            }),
        )
    })
}

/// Unwraps the `Json` extractor rejection (malformed JSON, bad field types)
/// into the standard 400 body. A form problem, never a 5xx.
fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|_| bad_request("invalid JSON body"))
}

/// Validates the conflict-mode string before any parsing/writing.
fn validate_conflict_mode(mode: &str) -> Result<(), ApiError> {
    if STOCK_CONFLICT_MODES.contains(&mode) {
        Ok(())
    } else {
        Err(bad_request(
            "stock_conflict_mode must be one of keep_db, apply_delta, force_override \
             (aliases: snapshot_aware, overwrite, skip)",
        ))
    }
}

/// Maps a uniqueness violation to a client-fixable 409, everything else to
/// the standard generic 503 (driver detail stays in the server log).
fn mutation_error(e: sqlx::Error, constraint: &str, message: &str, context: &str) -> ApiError {
    if let sqlx::Error::Database(db) = &e {
        if db.constraint() == Some(constraint) {
            return (
                StatusCode::CONFLICT,
                Json(ErrorBody {
                    error: message.to_string(),
                }),
            );
        }
    }
    state::database_unavailable(context, &e)
}

/// Builds the CSV attachment response: UTF-8 BOM + RFC 4180 body, the exact
/// download contract (type, filename, no-store).
fn csv_http_response(filename: &str, body: String) -> Result<Response, ApiError> {
    let mut bytes = Vec::with_capacity(body.len() + 3);
    bytes.extend_from_slice(CSV_BOM.to_string().as_bytes());
    bytes.extend_from_slice(body.as_bytes());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .map_err(|_| internal_error("failed to build the CSV response"))
}

// ---------------------------------------------------------------------------
// RFC 4180 parser / serializer
// ---------------------------------------------------------------------------

/// One parsed record: the 1-based physical line it started on (what Excel
/// shows, so row errors point at the right spreadsheet row) plus its fields.
#[derive(Debug)]
struct CsvRecord {
    line: usize,
    fields: Vec<String>,
}

/// RFC 4180 CSV parser: comma delimiter, CRLF or LF record separators, quoted
/// fields with `""` escapes, embedded commas/newlines, BOM not yet stripped.
/// Blank records (a line with no characters at all) are skipped — Excel
/// authors leave trailing blank lines. Malformed quoting is an error, never a
/// silently mangled cell.
fn parse_csv(input: &str) -> Result<Vec<CsvRecord>, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum FieldState {
        Start,
        Unquoted,
        Quoted,
        AfterQuote,
    }

    let mut records: Vec<CsvRecord> = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut state = FieldState::Start;
    let mut record_started = false;
    let mut line = 1usize;
    let mut record_line = 1usize;
    let mut chars = input.chars().peekable();

    macro_rules! finish_record {
        () => {{
            if record_started || !field.is_empty() || !fields.is_empty() {
                fields.push(std::mem::take(&mut field));
                records.push(CsvRecord {
                    line: record_line,
                    fields: std::mem::take(&mut fields),
                });
            }
            record_started = false;
            line += 1;
            record_line = line;
            state = FieldState::Start;
        }};
    }

    while let Some(c) = chars.next() {
        match state {
            FieldState::Start => match c {
                '"' => {
                    state = FieldState::Quoted;
                    record_started = true;
                }
                ',' => {
                    fields.push(String::new());
                    record_started = true;
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    finish_record!();
                }
                '\n' => finish_record!(),
                _ => {
                    field.push(c);
                    state = FieldState::Unquoted;
                    record_started = true;
                }
            },
            FieldState::Unquoted => match c {
                ',' => {
                    fields.push(std::mem::take(&mut field));
                    state = FieldState::Start;
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    finish_record!();
                }
                '\n' => finish_record!(),
                '"' => {
                    return Err(format!(
                        "unexpected quote inside an unquoted field on line {line}"
                    ))
                }
                _ => field.push(c),
            },
            FieldState::Quoted => match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        state = FieldState::AfterQuote;
                    }
                }
                '\n' => {
                    field.push('\n');
                    line += 1;
                }
                '\r' => {
                    // CRLF (or a lone CR) inside a quoted value is one line.
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    field.push('\n');
                    line += 1;
                }
                _ => field.push(c),
            },
            FieldState::AfterQuote => match c {
                ',' => {
                    fields.push(std::mem::take(&mut field));
                    state = FieldState::Start;
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    finish_record!();
                }
                '\n' => finish_record!(),
                _ => {
                    return Err(format!(
                        "unexpected character after a closing quote on line {line}"
                    ))
                }
            },
        }
    }
    if state == FieldState::Quoted {
        return Err(format!(
            "unterminated quoted field starting on line {record_line}"
        ));
    }
    if record_started || !field.is_empty() || !fields.is_empty() {
        fields.push(field);
        records.push(CsvRecord {
            line: record_line,
            fields,
        });
    }
    Ok(records)
}

/// Escapes one cell for RFC 4180 output: quotes only when the value contains
/// the delimiter, a quote or a newline (so numeric cells stay bare and Excel
/// keeps treating them as numbers).
fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Joins one record with commas + CRLF.
fn csv_line(cells: &[String]) -> String {
    let mut line = cells
        .iter()
        .map(|cell| csv_escape(cell))
        .collect::<Vec<_>>()
        .join(",");
    line.push_str("\r\n");
    line
}

fn strip_utf8_bom(text: &str) -> &str {
    text.strip_prefix(CSV_BOM).unwrap_or(text)
}

/// A parsed table: the trimmed header, a column-name index and the data
/// records. Row field counts are NOT normalized here — the planner reports a
/// per-row error instead of guessing what a short row meant.
#[derive(Debug)]
struct CsvTable {
    header: Vec<String>,
    index: HashMap<String, usize>,
    rows: Vec<CsvRecord>,
}

impl CsvTable {
    fn get<'a>(&self, row: &'a [String], column: &str) -> &'a str {
        self.index
            .get(column)
            .and_then(|&i| row.get(i))
            .map(String::as_str)
            .unwrap_or("")
    }

    fn has_column(&self, column: &str) -> bool {
        self.index.contains_key(column)
    }
}

/// Splits the first record off as the header, returning the data records.
fn split_header(records: Vec<CsvRecord>) -> Result<(CsvRecord, Vec<CsvRecord>), String> {
    let mut records = records;
    if records.is_empty() {
        return Err("CSV is empty (no header row)".to_string());
    }
    let header = records.remove(0);
    Ok((header, records))
}

fn build_table(header_record: CsvRecord, rows: Vec<CsvRecord>) -> Result<CsvTable, String> {
    let header: Vec<String> = header_record
        .fields
        .into_iter()
        .map(|name| name.trim().to_string())
        .collect();
    let mut index = HashMap::new();
    for (i, name) in header.iter().enumerate() {
        if index.insert(name.clone(), i).is_some() {
            return Err(format!("duplicate column {name:?} in the header"));
        }
    }
    Ok(CsvTable { header, index, rows })
}

/// Parses the products CSV: strips the BOM, checks the header (exact column
/// set; `pieces` may be absent for files written before it existed; unknown
/// columns are rejected so a wrong file cannot half-import).
fn parse_products_csv(csv: &str) -> Result<CsvTable, String> {
    let records = parse_csv(strip_utf8_bom(csv))?;
    let (header, rows) = split_header(records)?;
    let table = build_table(header, rows)?;
    for name in table.header.iter() {
        if !PRODUCTS_CSV_COLUMNS.contains(&name.as_str()) {
            return Err(format!("unknown column {name:?} in the header"));
        }
    }
    let missing: Vec<&str> = PRODUCTS_CSV_COLUMNS
        .iter()
        .copied()
        .filter(|column| *column != "pieces" && !table.has_column(column))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "CSV is missing required column(s): {}",
            missing.join(", ")
        ));
    }
    Ok(table)
}

/// Parses the site-content CSV: `key` first, then exactly the configured
/// locale set as columns.
fn parse_site_content_csv(csv: &str, locales: &[String]) -> Result<CsvTable, String> {
    let records = parse_csv(strip_utf8_bom(csv))?;
    let (header, rows) = split_header(records)?;
    let table = build_table(header, rows)?;
    if table.header.first().map(String::as_str) != Some("key") {
        return Err("site-content CSV must start with the \"key\" column".to_string());
    }
    let locale_columns = &table.header[1..];
    let expected: Vec<&str> = locales.iter().map(String::as_str).collect();
    let mut actual: Vec<&str> = locale_columns.iter().map(String::as_str).collect();
    let mut expected_sorted = expected.clone();
    actual.sort_unstable();
    expected_sorted.sort_unstable();
    if actual != expected_sorted {
        return Err(format!(
            "site-content CSV locale columns must be exactly: {}",
            expected.join(", ")
        ));
    }
    Ok(table)
}

// ---------------------------------------------------------------------------
// Cell parsing helpers
// ---------------------------------------------------------------------------

/// Detects a cell Excel may have converted to scientific notation
/// (`1.23E+10`, `8.52E+10`): optional sign, digits with an optional
/// fractional part, an `e`/`E`, an optional sign and an exponent. Returns
/// false for everything else, including plain decimals.
fn looks_like_scientific_notation(cell: &str) -> bool {
    let s = cell.trim();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mantissa_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == mantissa_start {
        return false;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let fraction_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == fraction_start {
            return false;
        }
    }
    if i >= bytes.len() || (bytes[i] != b'e' && bytes[i] != b'E') {
        return false;
    }
    i += 1;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let exponent_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i == bytes.len() && i > exponent_start
}

/// Formats integer HKD cents as a human dollar amount: `1615` for a whole
/// dollar, `1615.50` when cents are present. Money is never a float.
fn format_price_hkd(cents: i64) -> String {
    if cents % 100 == 0 {
        format!("{}", cents / 100)
    } else {
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

/// Parses a human dollar amount into integer cents: `1615`, `1615.5` and
/// `1615.50` are all accepted; negative amounts, more than two decimals and
/// thousands separators are rejected (export never writes them).
fn parse_price_hkd(raw: &str) -> Result<i64, String> {
    const BAD: &str = "price_hkd must be a dollar amount like 1615 or 1615.50";
    let s = raw.trim();
    if s.is_empty() {
        return Err("price_hkd cannot be empty".to_string());
    }
    let (whole, fraction) = match s.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (s, None),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BAD.to_string());
    }
    let cent_part = match fraction {
        None => 0i64,
        Some(fraction)
            if !fraction.is_empty()
                && fraction.len() <= 2
                && fraction.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let mut cents: i64 = fraction.parse().map_err(|_| BAD.to_string())?;
            if fraction.len() == 1 {
                cents *= 10;
            }
            cents
        }
        Some(_) => return Err(BAD.to_string()),
    };
    let whole: i64 = whole
        .parse()
        .map_err(|_| "price_hkd is too large".to_string())?;
    whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(cent_part))
        .ok_or_else(|| "price_hkd is too large".to_string())
}

/// Formats piece numbers the Excel-safe way: `#01;#02` (never `01-02`, which
/// Excel rewrites into `Jan-02`). Sorted ascending for stable diffs.
fn format_pieces(pieces: &[i32]) -> String {
    let mut sorted = pieces.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .map(|piece| format!("#{piece:02}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Parses a pieces cell: `#01;#02`, `1;2`, `1,2` and `#1, 2` all work;
/// duplicates inside the cell, zero/negative numbers and non-numeric entries
/// are rejected.
fn parse_pieces(raw: &str) -> Result<Vec<i32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut pieces: Vec<i32> = Vec::new();
    for part in raw.split([';', ',']) {
        let part = part.trim().trim_start_matches('#').trim();
        if part.is_empty() {
            return Err(format!("pieces contains an empty entry in {raw:?}"));
        }
        let piece: i32 = part
            .parse()
            .map_err(|_| format!("piece {part:?} is not a positive whole number"))?;
        if piece < 1 {
            return Err(format!("piece {piece} must be >= 1"));
        }
        if pieces.contains(&piece) {
            return Err(format!("piece {piece} is listed twice in one row"));
        }
        pieces.push(piece);
    }
    pieces.sort_unstable();
    Ok(pieces)
}

/// Parses an optional whole number; empty means "not provided".
fn parse_optional_i32(raw: &str) -> Result<Option<i32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<i32>()
        .map(Some)
        .map_err(|_| format!("{raw:?} is not a whole number"))
}

/// Parses an optional boolean: `true/false`, `1/0`, `yes/no`, `on/off`
/// (case-insensitive); empty means "not provided".
fn parse_optional_bool(raw: &str) -> Result<Option<bool>, String> {
    let raw = raw.trim().to_ascii_lowercase();
    match raw.as_str() {
        "" => Ok(None),
        "true" | "1" | "yes" | "y" | "on" => Ok(Some(true)),
        "false" | "0" | "no" | "n" | "off" => Ok(Some(false)),
        _ => Err(format!("{raw:?} is not a boolean (use true or false)")),
    }
}

fn content_key(locale: &str, field: &str) -> String {
    // `\u{1f}` (unit separator) cannot appear in a locale or a field name and
    // keeps the map key unambiguous for BTree ordering.
    format!("{locale}\u{1f}{field}")
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// `POST /api/admin/csv/products/{preview,commit}` body.
#[derive(Debug, Deserialize)]
pub struct ProductsCsvRequest {
    /// Raw spreadsheet text (BOM optional — stripped before parsing).
    pub csv: String,
    /// Whether the CSV's stock columns may move live stock. Default false:
    /// a text/price-only import never touches stock.
    #[serde(default)]
    pub apply_stock: bool,
    /// `keep_db` | `apply_delta` | `force_override` (aliases accepted, see
    /// [`STOCK_CONFLICT_MODES`]).
    #[serde(default = "default_stock_conflict_mode")]
    pub stock_conflict_mode: String,
    /// Whether a row whose `slug` differs from the stored slug may rename
    /// the product (cascading the rename to orders/order_items). Default
    /// false: the slug is locked unless the operator opts in.
    #[serde(default)]
    pub allow_slug_rename: bool,
}

/// `POST /api/admin/csv/site-content/{preview,commit}` body.
#[derive(Debug, Deserialize)]
pub struct SiteContentCsvRequest {
    /// Raw spreadsheet text (BOM optional — stripped before parsing).
    pub csv: String,
}

/// One field-level diff entry: CSV column name, stored value, CSV value.
#[derive(Debug, Clone, Serialize)]
pub struct CsvFieldChange {
    pub field: String,
    pub before: String,
    pub after: String,
}

/// One row-level validation error. `row` is the spreadsheet's physical line
/// (header is line 1) so the operator can jump straight to the cell.
#[derive(Debug, Clone, Serialize)]
pub struct CsvRowError {
    pub row: usize,
    pub column: String,
    pub message: String,
}

/// One planned products edit in the dry-run preview.
#[derive(Debug, Clone, Serialize)]
pub struct CsvPreviewRow {
    pub row_num: usize,
    /// `create` | `update` | `unchanged` | `delete`.
    pub action: String,
    pub slug: String,
    pub model: String,
    pub changes: Vec<CsvFieldChange>,
    pub stock_conflict: bool,
    pub warning: Option<String>,
}

/// Flat change entry + summary: the shape the Next.js proxy's diff table
/// consumes (`{ok, dry_run, changes, errors, summary}`). Kept alongside the
/// canonical `rows` so either client rendering works.
#[derive(Debug, Clone, Serialize)]
pub struct CsvSyncChange {
    pub row: String,
    pub slug: String,
    pub field: String,
    pub before: String,
    pub after: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CsvSyncSummary {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: usize,
}

/// Canonical dry-run / commit response for products.
#[derive(Debug, Clone, Serialize)]
pub struct CsvPreviewResponse {
    pub valid: bool,
    pub total_rows: usize,
    pub new_count: usize,
    pub updated_count: usize,
    pub unchanged_count: usize,
    pub conflict_count: usize,
    pub errors: Vec<CsvRowError>,
    pub rows: Vec<CsvPreviewRow>,
    // Compatibility aliases for the admin proxy's normalizer.
    pub ok: bool,
    pub dry_run: bool,
    pub changes: Vec<CsvSyncChange>,
    pub summary: CsvSyncSummary,
}

/// One planned site-copy edit in the dry-run preview.
#[derive(Debug, Clone, Serialize)]
pub struct SiteContentPreviewRow {
    pub row_num: usize,
    /// `create` | `update` | `delete` | `unchanged`.
    pub action: String,
    pub key: String,
    pub changes: Vec<CsvFieldChange>,
}

/// Canonical dry-run / commit response for site content.
#[derive(Debug, Clone, Serialize)]
pub struct SiteContentPreviewResponse {
    pub valid: bool,
    pub total_rows: usize,
    pub new_count: usize,
    pub updated_count: usize,
    pub unchanged_count: usize,
    pub errors: Vec<CsvRowError>,
    pub rows: Vec<SiteContentPreviewRow>,
    // Compatibility aliases for the admin proxy's normalizer.
    pub ok: bool,
    pub dry_run: bool,
    pub changes: Vec<CsvSyncChange>,
    pub summary: CsvSyncSummary,
}

// ---------------------------------------------------------------------------
// Pure stock resolution (the concurrent-order race logic)
// ---------------------------------------------------------------------------

/// The live stock counts a row must respect. `stock_reserved` and
/// `stock_quarantined` are read-only for CSV sync (the money path owns them).
#[derive(Debug, Clone, Copy)]
struct StockCounts {
    total: i32,
    reserved: i32,
    quarantined: i32,
}

/// The resolved stock decision for one existing product row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StockDecision {
    /// The value the row should end up with.
    effective: i32,
    /// The live count moved away from the CSV snapshot (operator edit only).
    conflict: bool,
    /// Whether the `stock_total` column actually changes (same value = no
    /// write at all, so an untouched count is literally never rewritten).
    write: bool,
    /// Human warning for the preview when `conflict` is set.
    warning: Option<String>,
}

/// Resolves the effective `stock_total` for an EXISTING product.
///
/// * No `apply_stock` request or no operator stock edit (`stock_delta == 0`
///   and `stock_total == snapshot_stock_total`): the live database count is
///   kept untouched. This is the concurrent-order guarantee: a CSV exported
///   before a sale cannot rewind the sale.
/// * Otherwise the edit is resolved through `stock_conflict_mode` against the
///   locked live count, and the STOCK-01 invariant
///   `effective >= stock_reserved + stock_quarantined` is enforced.
fn resolve_stock_existing(
    apply_stock: bool,
    mode: &str,
    db: &StockCounts,
    csv_total: Option<i32>,
    snapshot: Option<i32>,
    delta: i32,
) -> Result<StockDecision, String> {
    let base_total = csv_total.unwrap_or(db.total);
    let snapshot_total = snapshot.unwrap_or(base_total);
    let operator_edited = delta != 0 || base_total != snapshot_total;
    if !apply_stock || !operator_edited {
        return Ok(StockDecision {
            effective: db.total,
            conflict: false,
            write: false,
            warning: None,
        });
    }
    let conflict = db.total != snapshot_total;
    let resolved: i64 = match mode {
        "keep_db" | "skip" => i64::from(db.total),
        "apply_delta" => i64::from(db.total) + i64::from(delta),
        "force_override" | "overwrite" => i64::from(base_total),
        // `snapshot_aware`: apply the CSV number only while the live count
        // still equals the snapshot; a delta edit is applied on top of the
        // live count (adding stock never rewinds a concurrent sale).
        "snapshot_aware" => {
            if db.total == snapshot_total {
                if delta != 0 && base_total == snapshot_total {
                    i64::from(db.total) + i64::from(delta)
                } else {
                    i64::from(base_total)
                }
            } else {
                i64::from(db.total)
            }
        }
        _ => i64::from(db.total),
    };
    let effective = i32::try_from(resolved)
        .map_err(|_| "resulting stock_total is out of range".to_string())?;
    if i64::from(effective) < i64::from(db.reserved) + i64::from(db.quarantined) {
        return Err("stock_total must be >= stock_reserved + stock_quarantined".to_string());
    }
    let warning = conflict.then(|| {
        format!(
            "stock_total changed in the live database (CSV snapshot {snapshot_total}, \
             live {}); resolved with stock_conflict_mode={mode} to {effective}",
            db.total
        )
    });
    Ok(StockDecision {
        effective,
        conflict,
        write: effective != db.total,
        warning,
    })
}

/// Resolves the initial `stock_total` for a NEW product. There is no live
/// count to race (the SKU does not exist yet), so the CSV count applies
/// directly when `apply_stock` is set; a delta-only new row uses its delta as
/// the initial count.
fn resolve_stock_new(apply_stock: bool, csv_total: Option<i32>, delta: i32) -> Result<i32, String> {
    if !apply_stock {
        return Ok(0);
    }
    let total = csv_total.unwrap_or(if delta != 0 { delta } else { 0 });
    if total < 0 {
        return Err("stock_total must be >= 0".to_string());
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Products: parse one row
// ---------------------------------------------------------------------------

/// The typed, validated-enough-to-diff view of one product CSV row.
#[derive(Debug, Clone)]
struct ProductInput {
    id: Option<i64>,
    slug: String,
    brand: String,
    model: String,
    family: String,
    active: Option<bool>,
    price_cents: i64,
    art_variant: String,
    art_hue: i32,
    image_day_url: String,
    image_lit_url: String,
    designer: String,
    design_year: String,
    dimensions: String,
    light_source: String,
    /// `(locale, field) -> raw cell` for all eight content columns (empty
    /// values included: a blank is the explicit "clear this copy" state).
    content: BTreeMap<(String, String), String>,
    /// `None` when the `pieces` column is absent (leave the drop untouched);
    /// `Some` is the exact desired set (empty = remove all pieces).
    pieces: Option<Vec<i32>>,
    stock_total: Option<i32>,
    snapshot_stock_total: Option<i32>,
    stock_delta: i32,
}

/// Parses and validates one products CSV row. Every problem is collected
/// (rather than stopping at the first) so the operator can fix a whole
/// spreadsheet in one pass; the row is skipped from planning when any error
/// remains.
fn parse_product_row(
    table: &CsvTable,
    row: &[String],
    line: usize,
) -> Result<ProductInput, Vec<CsvRowError>> {
    let mut errors: Vec<CsvRowError> = Vec::new();

    // Scientific-notation cells are rejected EVERYWHERE: Excel may have
    // converted a large integer into `1.23E+10`, and importing that would
    // silently mangle a price, a piece number or a stock count.
    for (i, cell) in row.iter().enumerate() {
        if looks_like_scientific_notation(cell) {
            let column = table.header.get(i).map(String::as_str).unwrap_or("?");
            push_error(
                &mut errors,
                line,
                column,
                format!(
                    "cell {:?} is in scientific notation; type the value in full",
                    cell.trim()
                ),
            );
        }
    }

    let id = match table.get(row, "id").trim() {
        "" => None,
        raw => match raw.parse::<i64>() {
            Ok(id) if id > 0 => Some(id),
            Ok(0) => None,
            _ => {
                push_error(&mut errors, line, "id", "id must be 0 (new row) or a product id");
                None
            }
        },
    };

    let slug = table.get(row, "slug").to_string();
    if let Err(message) = admin_crud::validate_slug_input(&slug) {
        push_error(&mut errors, line, "slug", message);
    }

    let brand = table.get(row, "brand").to_string();
    if brand.trim().is_empty() {
        push_error(&mut errors, line, "brand", "brand cannot be empty");
    }
    if brand.contains('\0') {
        push_error(&mut errors, line, "brand", "value contains a NUL byte");
    }

    let model = table.get(row, "model").to_string();
    if model.trim().is_empty() {
        push_error(&mut errors, line, "model", "model cannot be empty");
    }
    if model.contains('\0') {
        push_error(&mut errors, line, "model", "value contains a NUL byte");
    }

    let price_cents = match parse_price_hkd(table.get(row, "price_hkd")) {
        Ok(cents) => cents,
        Err(message) => {
            push_error(&mut errors, line, "price_hkd", message);
            0
        }
    };

    let (family, image_day_url, image_lit_url, designer, design_year, dimensions, light_source) = (
        table.get(row, "family").to_string(),
        table.get(row, "image_day_url").to_string(),
        table.get(row, "image_lit_url").to_string(),
        table.get(row, "designer").to_string(),
        table.get(row, "design_year").to_string(),
        table.get(row, "dimensions").to_string(),
        table.get(row, "light_source").to_string(),
    );
    for (column, value) in [
        ("family", &family),
        ("image_day_url", &image_day_url),
        ("image_lit_url", &image_lit_url),
        ("designer", &designer),
        ("design_year", &design_year),
        ("dimensions", &dimensions),
        ("light_source", &light_source),
    ] {
        if value.contains('\0') {
            push_error(&mut errors, line, column, "value contains a NUL byte");
        }
    }

    let active = match parse_optional_bool(table.get(row, "active")) {
        Ok(value) => value,
        Err(message) => {
            push_error(&mut errors, line, "active", message);
            None
        }
    };

    let art_variant = table.get(row, "art_variant").to_string();
    if let Err(message) = admin_crud::validate_art_variant(&art_variant) {
        push_error(&mut errors, line, "art_variant", message);
    }

    let art_hue = match table.get(row, "art_hue").trim().parse::<i32>() {
        Ok(hue) => {
            if let Err(message) = admin_crud::validate_art_hue(hue) {
                push_error(&mut errors, line, "art_hue", message);
            }
            hue
        }
        Err(_) => {
            push_error(
                &mut errors,
                line,
                "art_hue",
                "art_hue must be a whole number between 0 and 359",
            );
            0
        }
    };

    let mut content: BTreeMap<(String, String), String> = BTreeMap::new();
    for (column, locale, field) in CONTENT_COLUMNS {
        let value = table.get(row, column).to_string();
        if value.contains('\0') {
            push_error(&mut errors, line, column, "value contains a NUL byte");
        }
        content.insert((locale.to_string(), field.to_string()), value);
    }

    let pieces = if table.has_column("pieces") {
        match parse_pieces(table.get(row, "pieces")) {
            Ok(pieces) => Some(pieces),
            Err(message) => {
                push_error(&mut errors, line, "pieces", message);
                None
            }
        }
    } else {
        None
    };

    let stock_total = match parse_optional_i32(table.get(row, "stock_total")) {
        Ok(value) => {
            if value.is_some_and(|total| total < 0) {
                push_error(&mut errors, line, "stock_total", "stock_total must be >= 0");
                None
            } else {
                value
            }
        }
        Err(message) => {
            push_error(&mut errors, line, "stock_total", message);
            None
        }
    };
    let snapshot_stock_total = match parse_optional_i32(table.get(row, "snapshot_stock_total")) {
        Ok(value) => {
            if value.is_some_and(|total| total < 0) {
                push_error(
                    &mut errors,
                    line,
                    "snapshot_stock_total",
                    "snapshot_stock_total must be >= 0",
                );
                None
            } else {
                value
            }
        }
        Err(message) => {
            push_error(&mut errors, line, "snapshot_stock_total", message);
            None
        }
    };
    let stock_delta = match parse_optional_i32(table.get(row, "stock_delta")) {
        Ok(value) => value.unwrap_or(0),
        Err(message) => {
            push_error(&mut errors, line, "stock_delta", message);
            0
        }
    };

    if errors.is_empty() {
        Ok(ProductInput {
            id,
            slug,
            brand,
            model,
            family,
            active,
            price_cents,
            art_variant,
            art_hue,
            image_day_url,
            image_lit_url,
            designer,
            design_year,
            dimensions,
            light_source,
            content,
            pieces,
            stock_total,
            snapshot_stock_total,
            stock_delta,
        })
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Products: plan
// ---------------------------------------------------------------------------

/// The stored product state a CSV row is diffed against.
#[derive(Debug, Clone, FromRow)]
struct DbProduct {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedAction {
    Create,
    Update,
    Delete,
    Unchanged,
}

impl PlannedAction {
    fn as_str(self) -> &'static str {
        match self {
            PlannedAction::Create => "create",
            PlannedAction::Update => "update",
            PlannedAction::Delete => "delete",
            PlannedAction::Unchanged => "unchanged",
        }
    }
}

/// Everything the commit needs to apply one planned products row.
struct PlannedProduct {
    action: PlannedAction,
    product_id: Option<i64>,
    input: ProductInput,
    effective_active: bool,
    effective_stock_total: i32,
    stock_write: bool,
    slug_changed: bool,
}

/// The dry-run result: row errors, the per-row diffs, and the apply plan.
struct CsvPlan {
    total_rows: usize,
    errors: Vec<CsvRowError>,
    preview_rows: Vec<CsvPreviewRow>,
    plans: Vec<PlannedProduct>,
}

fn push_change(changes: &mut Vec<CsvFieldChange>, field: &str, before: &str, after: &str) {
    if before != after {
        changes.push(CsvFieldChange {
            field: field.to_string(),
            before: before.to_string(),
            after: after.to_string(),
        });
    }
}

async fn load_products(conn: &mut PgConnection, lock: bool) -> Result<Vec<DbProduct>, sqlx::Error> {
    let sql = if lock {
        "SELECT id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, stock_quarantined, \
             image_day_url, image_lit_url, designer, design_year, dimensions, \
             light_source \
         FROM products ORDER BY id FOR UPDATE"
    } else {
        "SELECT id, slug, brand, model, price_cents, family, art_variant, art_hue, \
             active, stock_total, stock_reserved, stock_quarantined, \
             image_day_url, image_lit_url, designer, design_year, dimensions, \
             light_source \
         FROM products ORDER BY id"
    };
    sqlx::query_as::<_, DbProduct>(sql).fetch_all(&mut *conn).await
}

/// Validates one row's piece list against the database ownership map and the
/// pieces already claimed by earlier rows of this same CSV.
fn validate_pieces_for_row(
    line: usize,
    pieces: &[i32],
    current_owner: Option<i64>,
    piece_owner: &HashMap<i32, i64>,
    claimed: &mut HashMap<i32, usize>,
) -> Vec<CsvRowError> {
    let mut errors = Vec::new();
    for &piece in pieces {
        if let Some(owner) = piece_owner.get(&piece) {
            if Some(*owner) != current_owner {
                push_error(
                    &mut errors,
                    line,
                    "pieces",
                    format!("piece #{piece:02} already belongs to another product"),
                );
            }
        }
        if let Some(previous) = claimed.insert(piece, line) {
            push_error(
                &mut errors,
                line,
                "pieces",
                format!("piece #{piece:02} is listed twice in this CSV (also line {previous})"),
            );
        }
    }
    errors
}

/// Plans (dry-runs) a products CSV against the database.
///
/// `lock` is false for the preview (a consistent REPEATABLE READ snapshot is
/// enough) and true for the commit: the commit locks every product row
/// (`FOR UPDATE`) before resolving stock, so no concurrent order/admin write
/// can slip between the conflict check and the UPDATE.
async fn plan_products_csv(
    conn: &mut PgConnection,
    table: &CsvTable,
    req: &ProductsCsvRequest,
    lock: bool,
) -> Result<CsvPlan, sqlx::Error> {
    let products = load_products(conn, lock).await?;
    let mut content_by_product: HashMap<i64, BTreeMap<String, String>> = HashMap::new();
    {
        let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT product_id, locale, field, value FROM product_content \
             ORDER BY product_id, locale, field",
        )
        .fetch_all(&mut *conn)
        .await?;
        for (product_id, locale, field, value) in rows {
            content_by_product
                .entry(product_id)
                .or_default()
                .insert(content_key(&locale, &field), value);
        }
    }
    let mut pieces_by_product: HashMap<i64, Vec<i32>> = HashMap::new();
    let mut piece_owner: HashMap<i32, i64> = HashMap::new();
    {
        let rows: Vec<(i32, i64)> =
            sqlx::query_as("SELECT piece_no, product_id FROM pieces ORDER BY piece_no")
                .fetch_all(&mut *conn)
                .await?;
        for (piece_no, product_id) in rows {
            pieces_by_product.entry(product_id).or_default().push(piece_no);
            piece_owner.insert(piece_no, product_id);
        }
    }

    let by_id: HashMap<i64, usize> = products
        .iter()
        .enumerate()
        .map(|(i, product)| (product.id, i))
        .collect();
    let by_slug: HashMap<&str, usize> = products
        .iter()
        .enumerate()
        .map(|(i, product)| (product.slug.as_str(), i))
        .collect();

    let mut plan = CsvPlan {
        total_rows: 0,
        errors: Vec::new(),
        preview_rows: Vec::new(),
        plans: Vec::new(),
    };
    let mut seen_products: HashMap<i64, usize> = HashMap::new();
    let mut claimed_slugs: HashMap<String, i64> = HashMap::new();
    let mut claimed_pieces: HashMap<i32, usize> = HashMap::new();
    let mut next_synthetic_id: i64 = -1;

    for record in &table.rows {
        plan.total_rows += 1;
        let line = record.line;
        let row = &record.fields;
        if row.len() != table.header.len() {
            push_error(
                &mut plan.errors,
                line,
                "row",
                format!(
                    "row has {} fields but the header has {}",
                    row.len(),
                    table.header.len()
                ),
            );
            continue;
        }
        let input = match parse_product_row(table, row, line) {
            Ok(input) => input,
            Err(errors) => {
                plan.errors.extend(errors);
                continue;
            }
        };

        // Match by id first, then slug. An id that does not exist is an
        // error (never a silent create: ids are allocated by the database).
        let target: Option<(usize, i64)> = match input.id {
            Some(id) => match by_id.get(&id) {
                Some(&index) => Some((index, id)),
                None => {
                    push_error(
                        &mut plan.errors,
                        line,
                        "id",
                        format!("no product has id {id}"),
                    );
                    continue;
                }
            },
            None => by_slug
                .get(input.slug.as_str())
                .map(|&index| (index, products[index].id)),
        };
        if let Some((_, id)) = target {
            if let Some(&slug_index) = by_slug.get(input.slug.as_str()) {
                if products[slug_index].id != id {
                    push_error(
                        &mut plan.errors,
                        line,
                        "slug",
                        "the slug belongs to a different product than the id",
                    );
                    continue;
                }
            }
            if let Some(previous) = seen_products.insert(id, line) {
                push_error(
                    &mut plan.errors,
                    line,
                    "row",
                    format!("duplicate row for this product (already on line {previous})"),
                );
                continue;
            }
        }

        // Final slugs must be unique across the whole import (a rename and a
        // create cannot land on the same slug).
        let owner_key = target.map(|(_, id)| id).unwrap_or_else(|| {
            let id = next_synthetic_id;
            next_synthetic_id -= 1;
            id
        });
        if let Some(previous_owner) = claimed_slugs.get(&input.slug) {
            if *previous_owner != owner_key {
                push_error(
                    &mut plan.errors,
                    line,
                    "slug",
                    "this slug is used by another product in this CSV",
                );
                continue;
            }
        }
        claimed_slugs.insert(input.slug.clone(), owner_key);

        let mut changes: Vec<CsvFieldChange> = Vec::new();
        let (
            action,
            product_id,
            effective_active,
            effective_stock_total,
            stock_write,
            slug_changed,
            stock_conflict,
            warning,
        ) = match target {
            Some((index, id)) => {
                let db = &products[index];
                let mut row_errors: Vec<CsvRowError> = Vec::new();
                if let Some(pieces) = &input.pieces {
                    row_errors.extend(validate_pieces_for_row(
                        line,
                        pieces,
                        Some(id),
                        &piece_owner,
                        &mut claimed_pieces,
                    ));
                }
                let slug_changed = input.slug != db.slug;
                if slug_changed && !req.allow_slug_rename {
                    push_error(
                        &mut row_errors,
                        line,
                        "slug",
                        "slug rename not allowed; enable allow_slug_rename to rename",
                    );
                }
                if !row_errors.is_empty() {
                    plan.errors.extend(row_errors);
                    continue;
                }

                let db_stock = StockCounts {
                    total: db.stock_total,
                    reserved: db.stock_reserved,
                    quarantined: db.stock_quarantined,
                };
                let decision = match resolve_stock_existing(
                    req.apply_stock,
                    &req.stock_conflict_mode,
                    &db_stock,
                    input.stock_total,
                    input.snapshot_stock_total,
                    input.stock_delta,
                ) {
                    Ok(decision) => decision,
                    Err(message) => {
                        push_error(&mut plan.errors, line, "stock_total", message);
                        continue;
                    }
                };
                let effective_active = input.active.unwrap_or(db.active);

                push_change(&mut changes, "slug", &db.slug, &input.slug);
                push_change(&mut changes, "brand", &db.brand, &input.brand);
                push_change(&mut changes, "model", &db.model, &input.model);
                push_change(
                    &mut changes,
                    "price_hkd",
                    &format_price_hkd(db.price_cents),
                    &format_price_hkd(input.price_cents),
                );
                push_change(&mut changes, "family", &db.family, &input.family);
                push_change(
                    &mut changes,
                    "active",
                    if db.active { "true" } else { "false" },
                    if effective_active { "true" } else { "false" },
                );
                push_change(&mut changes, "art_variant", &db.art_variant, &input.art_variant);
                push_change(
                    &mut changes,
                    "art_hue",
                    &db.art_hue.to_string(),
                    &input.art_hue.to_string(),
                );
                push_change(&mut changes, "image_day_url", &db.image_day_url, &input.image_day_url);
                push_change(&mut changes, "image_lit_url", &db.image_lit_url, &input.image_lit_url);
                push_change(&mut changes, "designer", &db.designer, &input.designer);
                push_change(&mut changes, "design_year", &db.design_year, &input.design_year);
                push_change(&mut changes, "dimensions", &db.dimensions, &input.dimensions);
                push_change(&mut changes, "light_source", &db.light_source, &input.light_source);
                if decision.write {
                    push_change(
                        &mut changes,
                        "stock_total",
                        &db.stock_total.to_string(),
                        &decision.effective.to_string(),
                    );
                }
                let empty = BTreeMap::new();
                let db_content = content_by_product.get(&id).unwrap_or(&empty);
                for (column, locale, field) in CONTENT_COLUMNS {
                    let before = db_content
                        .get(&content_key(locale, field))
                        .map(String::as_str)
                        .unwrap_or("");
                    let after = input
                        .content
                        .get(&(locale.to_string(), field.to_string()))
                        .map(String::as_str)
                        .unwrap_or("");
                    push_change(&mut changes, column, before, after);
                }
                if let Some(pieces) = &input.pieces {
                    let before =
                        format_pieces(pieces_by_product.get(&id).map(Vec::as_slice).unwrap_or(&[]));
                    let after = format_pieces(pieces);
                    push_change(&mut changes, "pieces", &before, &after);
                }

                let action = if changes.is_empty() {
                    PlannedAction::Unchanged
                } else {
                    PlannedAction::Update
                };
                (
                    action,
                    Some(id),
                    effective_active,
                    decision.effective,
                    decision.write,
                    slug_changed,
                    decision.conflict,
                    decision.warning,
                )
            }
            None => {
                let mut row_errors: Vec<CsvRowError> = Vec::new();
                if let Some(pieces) = &input.pieces {
                    row_errors.extend(validate_pieces_for_row(
                        line,
                        pieces,
                        None,
                        &piece_owner,
                        &mut claimed_pieces,
                    ));
                }
                if !row_errors.is_empty() {
                    plan.errors.extend(row_errors);
                    continue;
                }
                let effective_stock_total = match resolve_stock_new(
                    req.apply_stock,
                    input.stock_total,
                    input.stock_delta,
                ) {
                    Ok(total) => total,
                    Err(message) => {
                        push_error(&mut plan.errors, line, "stock_total", message);
                        continue;
                    }
                };
                let effective_active = input.active.unwrap_or(true);
                push_change(&mut changes, "slug", "", &input.slug);
                push_change(&mut changes, "brand", "", &input.brand);
                push_change(&mut changes, "model", "", &input.model);
                push_change(&mut changes, "price_hkd", "", &format_price_hkd(input.price_cents));
                push_change(&mut changes, "family", "", &input.family);
                push_change(
                    &mut changes,
                    "active",
                    "",
                    if effective_active { "true" } else { "false" },
                );
                push_change(&mut changes, "art_variant", "", &input.art_variant);
                push_change(&mut changes, "art_hue", "", &input.art_hue.to_string());
                push_change(&mut changes, "image_day_url", "", &input.image_day_url);
                push_change(&mut changes, "image_lit_url", "", &input.image_lit_url);
                push_change(&mut changes, "designer", "", &input.designer);
                push_change(&mut changes, "design_year", "", &input.design_year);
                push_change(&mut changes, "dimensions", "", &input.dimensions);
                push_change(&mut changes, "light_source", "", &input.light_source);
                if effective_stock_total != 0 {
                    push_change(
                        &mut changes,
                        "stock_total",
                        "",
                        &effective_stock_total.to_string(),
                    );
                }
                for (column, locale, field) in CONTENT_COLUMNS {
                    let after = input
                        .content
                        .get(&(locale.to_string(), field.to_string()))
                        .map(String::as_str)
                        .unwrap_or("");
                    push_change(&mut changes, column, "", after);
                }
                if let Some(pieces) = &input.pieces {
                    push_change(&mut changes, "pieces", "", &format_pieces(pieces));
                }
                (
                    PlannedAction::Create,
                    None,
                    effective_active,
                    effective_stock_total,
                    true,
                    false,
                    false,
                    None,
                )
            }
        };

        plan.preview_rows.push(CsvPreviewRow {
            row_num: line,
            action: action.as_str().to_string(),
            slug: input.slug.clone(),
            model: input.model.clone(),
            changes,
            stock_conflict,
            warning,
        });
        plan.plans.push(PlannedProduct {
            action,
            product_id,
            input,
            effective_active,
            effective_stock_total,
            stock_write,
            slug_changed,
        });
    }

    Ok(plan)
}

fn plan_counts(plan: &CsvPlan) -> (usize, usize, usize, usize) {
    let mut new_count = 0;
    let mut updated_count = 0;
    let mut unchanged_count = 0;
    let mut conflict_count = 0;
    for row in &plan.preview_rows {
        match row.action.as_str() {
            "create" => new_count += 1,
            "update" => updated_count += 1,
            "unchanged" => unchanged_count += 1,
            _ => {}
        }
        if row.stock_conflict {
            conflict_count += 1;
        }
    }
    (new_count, updated_count, unchanged_count, conflict_count)
}

fn build_products_preview(plan: &CsvPlan, dry_run: bool) -> CsvPreviewResponse {
    let (new_count, updated_count, unchanged_count, conflict_count) = plan_counts(plan);
    let mut changes = Vec::new();
    for row in &plan.preview_rows {
        for change in &row.changes {
            changes.push(CsvSyncChange {
                row: row.row_num.to_string(),
                slug: row.slug.clone(),
                field: change.field.clone(),
                before: change.before.clone(),
                after: change.after.clone(),
                action: row.action.clone(),
            });
        }
    }
    CsvPreviewResponse {
        valid: plan.errors.is_empty(),
        total_rows: plan.total_rows,
        new_count,
        updated_count,
        unchanged_count,
        conflict_count,
        errors: plan.errors.clone(),
        rows: plan.preview_rows.clone(),
        ok: plan.errors.is_empty(),
        dry_run,
        changes,
        summary: CsvSyncSummary {
            created: new_count,
            updated: updated_count,
            unchanged: unchanged_count,
            errors: plan.errors.len(),
        },
    }
}

// ---------------------------------------------------------------------------
// Products: apply
// ---------------------------------------------------------------------------

/// Rewrites one product's content rows to exactly the CSV state: every
/// stored (locale, field) is deleted, then every non-blank cell is upserted.
/// Blank cells are the explicit "clear this copy" state (same contract as the
/// Admin Studio full save).
async fn replace_content(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_id: i64,
    content: &BTreeMap<(String, String), String>,
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM product_content WHERE product_id = $1")
        .bind(product_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| state::database_unavailable("failed to replace product content", &e))?;
    for ((locale, field), value) in content {
        if value.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO product_content (product_id, locale, field, value) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(product_id)
        .bind(locale)
        .bind(field)
        .bind(value)
        .execute(&mut **tx)
        .await
        .map_err(|e| state::database_unavailable("failed to insert product content", &e))?;
    }
    Ok(())
}

/// Rewrites one product's piece numbers to exactly the CSV list. Collisions
/// with another SKU were rejected at plan time; a piece number can only
/// collide here if another admin adds it between plan and apply, which maps
/// to a 409 instead of a 503.
async fn replace_pieces(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_id: i64,
    pieces: &[i32],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM pieces WHERE product_id = $1")
        .bind(product_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| state::database_unavailable("failed to replace pieces", &e))?;
    for &piece in pieces {
        sqlx::query("INSERT INTO pieces (piece_no, product_id) VALUES ($1, $2)")
            .bind(piece)
            .bind(product_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| {
                mutation_error(
                    e,
                    "pieces_pkey",
                    "piece_no already exists",
                    "failed to insert piece",
                )
            })?;
    }
    Ok(())
}

/// Applies the planned rows inside the caller's open, product-locked
/// transaction, then writes the single `csv_bulk_sync` audit row. Either the
/// whole spreadsheet lands or nothing does.
async fn apply_products_csv(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &str,
    req: &ProductsCsvRequest,
    plan: &CsvPlan,
) -> Result<(), ApiError> {
    for planned in &plan.plans {
        match planned.action {
            PlannedAction::Unchanged => continue,
            PlannedAction::Delete => continue, // products are soft-deleted only
            PlannedAction::Create => {
                let input = &planned.input;
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO products \
                         (slug, brand, model, price_cents, family, active, art_variant, \
                          art_hue, stock_total, stock_reserved, stock_quarantined, \
                          image_day_url, image_lit_url, designer, design_year, \
                          dimensions, light_source) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 0, 0, $10, $11, $12, \
                             $13, $14, $15) \
                     RETURNING id",
                )
                .bind(&input.slug)
                .bind(&input.brand)
                .bind(&input.model)
                .bind(input.price_cents)
                .bind(&input.family)
                .bind(planned.effective_active)
                .bind(&input.art_variant)
                .bind(input.art_hue)
                .bind(planned.effective_stock_total)
                .bind(&input.image_day_url)
                .bind(&input.image_lit_url)
                .bind(&input.designer)
                .bind(&input.design_year)
                .bind(&input.dimensions)
                .bind(&input.light_source)
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| {
                    mutation_error(
                        e,
                        "products_slug_key",
                        "slug already in use",
                        "failed to insert product",
                    )
                })?;
                replace_content(tx, id, &input.content).await?;
                if let Some(pieces) = &input.pieces {
                    replace_pieces(tx, id, pieces).await?;
                }
            }
            PlannedAction::Update => {
                let Some(id) = planned.product_id else {
                    continue;
                };
                let input = &planned.input;
                if planned.stock_write {
                    sqlx::query(
                        "UPDATE products SET \
                             slug = $1, brand = $2, model = $3, price_cents = $4, \
                             family = $5, active = $6, art_variant = $7, art_hue = $8, \
                             stock_total = $9, image_day_url = $10, image_lit_url = $11, \
                             designer = $12, design_year = $13, dimensions = $14, \
                             light_source = $15 \
                         WHERE id = $16",
                    )
                    .bind(&input.slug)
                    .bind(&input.brand)
                    .bind(&input.model)
                    .bind(input.price_cents)
                    .bind(&input.family)
                    .bind(planned.effective_active)
                    .bind(&input.art_variant)
                    .bind(input.art_hue)
                    .bind(planned.effective_stock_total)
                    .bind(&input.image_day_url)
                    .bind(&input.image_lit_url)
                    .bind(&input.designer)
                    .bind(&input.design_year)
                    .bind(&input.dimensions)
                    .bind(&input.light_source)
                    .bind(id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                        mutation_error(
                            e,
                            "products_slug_key",
                            "slug already in use",
                            "failed to update product",
                        )
                    })?;
                } else {
                    sqlx::query(
                        "UPDATE products SET \
                             slug = $1, brand = $2, model = $3, price_cents = $4, \
                             family = $5, active = $6, art_variant = $7, art_hue = $8, \
                             image_day_url = $9, image_lit_url = $10, designer = $11, \
                             design_year = $12, dimensions = $13, light_source = $14 \
                         WHERE id = $15",
                    )
                    .bind(&input.slug)
                    .bind(&input.brand)
                    .bind(&input.model)
                    .bind(input.price_cents)
                    .bind(&input.family)
                    .bind(planned.effective_active)
                    .bind(&input.art_variant)
                    .bind(input.art_hue)
                    .bind(&input.image_day_url)
                    .bind(&input.image_lit_url)
                    .bind(&input.designer)
                    .bind(&input.design_year)
                    .bind(&input.dimensions)
                    .bind(&input.light_source)
                    .bind(id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                        mutation_error(
                            e,
                            "products_slug_key",
                            "slug already in use",
                            "failed to update product",
                        )
                    })?;
                }
                if planned.slug_changed {
                    admin_crud::cascade_product_slug(tx, id, &input.slug)
                        .await
                        .map_err(|e| {
                            state::database_unavailable("failed to cascade the slug rename", &e)
                        })?;
                }
                replace_content(tx, id, &input.content).await?;
                if let Some(pieces) = &input.pieces {
                    replace_pieces(tx, id, pieces).await?;
                }
            }
        }
    }

    let (new_count, updated_count, unchanged_count, conflict_count) = plan_counts(plan);
    admin_crud::insert_audit(
        tx,
        actor,
        "csv_bulk_sync",
        "products",
        None,
        serde_json::json!({
            "new_count": new_count,
            "updated_count": updated_count,
            "unchanged_count": unchanged_count,
            "conflict_count": conflict_count,
            "apply_stock": req.apply_stock,
            "stock_conflict_mode": req.stock_conflict_mode,
            "allow_slug_rename": req.allow_slug_rename,
        }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit the CSV sync", &e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Site content: plan / apply
// ---------------------------------------------------------------------------

/// The typed view of one site-content CSV row: key plus raw cell per locale.
#[derive(Debug, Clone)]
struct SiteContentInput {
    key: String,
    values: BTreeMap<String, String>,
}

struct PlannedSiteContent {
    action: PlannedAction,
    key: String,
    /// `(locale, before, after)`: `after == None` deletes the row.
    changes: Vec<(String, Option<String>, Option<String>)>,
}

struct SiteContentPlan {
    total_rows: usize,
    errors: Vec<CsvRowError>,
    preview_rows: Vec<SiteContentPreviewRow>,
    plans: Vec<PlannedSiteContent>,
}

/// Plans a site-content CSV against the database.
async fn plan_site_content_csv(
    conn: &mut PgConnection,
    table: &CsvTable,
    locales: &[String],
) -> Result<SiteContentPlan, sqlx::Error> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT key, locale, value FROM site_content ORDER BY key, locale")
            .fetch_all(&mut *conn)
            .await?;
    let mut db_rows: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (key, locale, value) in rows {
        db_rows.entry(key).or_default().insert(locale, value);
    }

    let mut plan = SiteContentPlan {
        total_rows: 0,
        errors: Vec::new(),
        preview_rows: Vec::new(),
        plans: Vec::new(),
    };
    let mut seen_keys: HashMap<String, usize> = HashMap::new();

    for record in &table.rows {
        plan.total_rows += 1;
        let line = record.line;
        if record.fields.len() != table.header.len() {
            push_error(
                &mut plan.errors,
                line,
                "row",
                format!(
                    "row has {} fields but the header has {}",
                    record.fields.len(),
                    table.header.len()
                ),
            );
            continue;
        }
        let input = {
            let mut errors: Vec<CsvRowError> = Vec::new();
            let key = table.get(&record.fields, "key").trim().to_string();
            if key.is_empty() {
                push_error(&mut errors, line, "key", "key cannot be empty");
            }
            if key.contains('\0') {
                push_error(&mut errors, line, "key", "value contains a NUL byte");
            }
            let mut values = BTreeMap::new();
            for locale in locales {
                let value = table.get(&record.fields, locale).to_string();
                if value.contains('\0') {
                    push_error(&mut errors, line, locale, "value contains a NUL byte");
                }
                values.insert(locale.clone(), value);
            }
            if errors.is_empty() {
                Some(SiteContentInput { key, values })
            } else {
                plan.errors.extend(errors);
                None
            }
        };
        let Some(input) = input else { continue };
        if let Some(previous) = seen_keys.insert(input.key.clone(), line) {
            push_error(
                &mut plan.errors,
                line,
                "key",
                format!("duplicate key (already on line {previous})"),
            );
            continue;
        }

        let stored = db_rows.get(&input.key);
        let mut changes: Vec<(String, Option<String>, Option<String>)> = Vec::new();
        for locale in locales {
            let before = stored.and_then(|row| row.get(locale)).cloned();
            let raw = input.values.get(locale).cloned().unwrap_or_default();
            // Blanking a cell deletes that (key, locale) row: the export
            // always carries every stored row, so a blank is the explicit
            // clear state (never an accidental wipe of unexported copy).
            let after = if raw.trim().is_empty() { None } else { Some(raw) };
            if before != after {
                changes.push((locale.clone(), before, after));
            }
        }
        let action = if changes.is_empty() {
            PlannedAction::Unchanged
        } else if stored.is_none() {
            PlannedAction::Create
        } else if changes.iter().all(|(_, _, after)| after.is_none()) {
            PlannedAction::Delete
        } else {
            PlannedAction::Update
        };
        let preview_changes = changes
            .iter()
            .map(|(locale, before, after)| CsvFieldChange {
                field: locale.clone(),
                before: before.clone().unwrap_or_default(),
                after: after.clone().unwrap_or_default(),
            })
            .collect();
        plan.preview_rows.push(SiteContentPreviewRow {
            row_num: line,
            action: action.as_str().to_string(),
            key: input.key.clone(),
            changes: preview_changes,
        });
        plan.plans.push(PlannedSiteContent {
            action,
            key: input.key,
            changes,
        });
    }

    Ok(plan)
}

fn build_site_content_preview(plan: &SiteContentPlan, dry_run: bool) -> SiteContentPreviewResponse {
    let mut new_count = 0;
    let mut updated_count = 0;
    let mut unchanged_count = 0;
    let mut changes = Vec::new();
    for row in &plan.preview_rows {
        match row.action.as_str() {
            "create" => new_count += 1,
            // A site-content delete is a state change, counted with updates.
            "update" | "delete" => updated_count += 1,
            "unchanged" => unchanged_count += 1,
            _ => {}
        }
        for change in &row.changes {
            changes.push(CsvSyncChange {
                row: row.row_num.to_string(),
                slug: row.key.clone(),
                field: change.field.clone(),
                before: change.before.clone(),
                after: change.after.clone(),
                action: row.action.clone(),
            });
        }
    }
    SiteContentPreviewResponse {
        valid: plan.errors.is_empty(),
        total_rows: plan.total_rows,
        new_count,
        updated_count,
        unchanged_count,
        errors: plan.errors.clone(),
        rows: plan.preview_rows.clone(),
        ok: plan.errors.is_empty(),
        dry_run,
        changes,
        summary: CsvSyncSummary {
            created: new_count,
            updated: updated_count,
            unchanged: unchanged_count,
            errors: plan.errors.len(),
        },
    }
}

async fn apply_site_content_csv(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &str,
    plan: &SiteContentPlan,
) -> Result<(), ApiError> {
    for planned in &plan.plans {
        for (locale, _before, after) in &planned.changes {
            match after {
                Some(value) => {
                    sqlx::query(
                        "INSERT INTO site_content (key, locale, value) VALUES ($1, $2, $3) \
                         ON CONFLICT (key, locale) DO UPDATE SET value = EXCLUDED.value",
                    )
                    .bind(&planned.key)
                    .bind(locale)
                    .bind(value)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| {
                        state::database_unavailable("failed to upsert site content", &e)
                    })?;
                }
                None => {
                    sqlx::query("DELETE FROM site_content WHERE key = $1 AND locale = $2")
                        .bind(&planned.key)
                        .bind(locale)
                        .execute(&mut **tx)
                        .await
                        .map_err(|e| {
                            state::database_unavailable("failed to delete site content", &e)
                        })?;
                }
            }
        }
    }

    let (new_count, updated_count, unchanged_count) = site_content_counts(plan);
    admin_crud::insert_audit(
        tx,
        actor,
        "csv_bulk_sync",
        "site_content",
        None,
        serde_json::json!({
            "new_count": new_count,
            "updated_count": updated_count,
            "unchanged_count": unchanged_count,
        }),
    )
    .await
    .map_err(|e| state::database_unavailable("failed to audit the site-content sync", &e))?;
    Ok(())
}

fn site_content_counts(plan: &SiteContentPlan) -> (usize, usize, usize) {
    let mut new_count = 0;
    let mut updated_count = 0;
    let mut unchanged_count = 0;
    for row in &plan.preview_rows {
        match row.action.as_str() {
            "create" => new_count += 1,
            "update" | "delete" => updated_count += 1,
            "unchanged" => unchanged_count += 1,
            _ => {}
        }
    }
    (new_count, updated_count, unchanged_count)
}

// ---------------------------------------------------------------------------
// Handlers: products
// ---------------------------------------------------------------------------

/// `GET /api/admin/csv/products` — the whole catalogue as an Excel-ready CSV.
///
/// `snapshot_stock_total` and `stock_total` both carry the CURRENT live count
/// and `stock_delta` is `0`: that is the baseline a later import diffs
/// against. `row_version_hash` matches the Admin Studio fingerprint so an
/// operator can eyeball which rows changed since export.
pub async fn export_products_csv(State(app): State<AppState>) -> Result<Response, ApiError> {
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open the CSV export", &e))?;
    // REPEATABLE READ: products, content, pieces and reference prices must
    // come from one coherent catalogue snapshot, never a half-written mix.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to set export isolation", &e))?;
    let products = load_products(&mut tx, false)
        .await
        .map_err(|e| state::database_unavailable("failed to load products for export", &e))?;
    let mut content_by_product: BTreeMap<i64, BTreeMap<String, BTreeMap<String, String>>> =
        BTreeMap::new();
    {
        let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT product_id, locale, field, value FROM product_content \
             ORDER BY product_id, locale, field",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to load product content", &e))?;
        for (product_id, locale, field, value) in rows {
            content_by_product
                .entry(product_id)
                .or_default()
                .entry(locale)
                .or_default()
                .insert(field, value);
        }
    }
    let mut pieces_by_product: HashMap<i64, Vec<i32>> = HashMap::new();
    {
        let rows: Vec<(i32, i64)> =
            sqlx::query_as("SELECT piece_no, product_id FROM pieces ORDER BY piece_no")
                .fetch_all(&mut *tx)
                .await
                .map_err(|e| state::database_unavailable("failed to load pieces", &e))?;
        for (piece_no, product_id) in rows {
            pieces_by_product.entry(product_id).or_default().push(piece_no);
        }
    }
    let references: Vec<admin_crud::AdminReferencePrice> = sqlx::query_as(
        "SELECT product_id, min_price_cents, max_price_cents, source_name, source_url, \
             checked_on FROM reference_prices ORDER BY product_id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| state::database_unavailable("failed to load reference prices", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit the export read", &e))?;
    let reference_by_product: HashMap<i64, admin_crud::AdminReferencePrice> = references
        .into_iter()
        .map(|reference| (reference.product_id, reference))
        .collect();

    let mut body = csv_line(
        &PRODUCTS_CSV_COLUMNS
            .iter()
            .map(|column| column.to_string())
            .collect::<Vec<_>>(),
    );
    for product in &products {
        let empty_content = BTreeMap::new();
        let content = content_by_product
            .get(&product.id)
            .unwrap_or(&empty_content)
            .clone();
        let version_hash = admin_crud::compute_product_version_hash(
            &admin_crud::ProductVersionState {
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
                reference_price: reference_by_product.get(&product.id).cloned(),
            },
        );
        let mut cells: Vec<String> = vec![
            product.id.to_string(),
            product.slug.clone(),
            product.brand.clone(),
            product.model.clone(),
            product.family.clone(),
            if product.active { "true" } else { "false" }.to_string(),
            format_price_hkd(product.price_cents),
            product.stock_total.to_string(),
            product.stock_total.to_string(),
            "0".to_string(),
            product.stock_reserved.to_string(),
            product.stock_quarantined.to_string(),
            product.art_variant.clone(),
            product.art_hue.to_string(),
            product.image_day_url.clone(),
            product.image_lit_url.clone(),
            product.designer.clone(),
            product.design_year.clone(),
            product.dimensions.clone(),
            product.light_source.clone(),
            format_pieces(
                pieces_by_product
                    .get(&product.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            ),
        ];
        for (_, locale, field) in CONTENT_COLUMNS {
            cells.push(
                content
                    .get(*locale)
                    .and_then(|fields| fields.get(*field))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        cells.push(version_hash);
        body.push_str(&csv_line(&cells));
    }
    csv_http_response("catalogue-products.csv", body)
}

/// `POST /api/admin/csv/products/preview` — dry run: parse, validate, diff and
/// resolve stock conflicts WITHOUT writing anything.
pub async fn preview_products_csv(
    State(app): State<AppState>,
    body: Result<Json<ProductsCsvRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let req = json_body(body)?;
    validate_conflict_mode(&req.stock_conflict_mode)?;
    let table = parse_products_csv(&req.csv).map_err(|message| bad_request(&message))?;
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open the CSV preview", &e))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to set preview isolation", &e))?;
    let plan = plan_products_csv(&mut tx, &table, &req, false)
        .await
        .map_err(|e| state::database_unavailable("failed to plan the CSV preview", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit the preview read", &e))?;
    Ok(Json(build_products_preview(&plan, true)).into_response())
}

/// `POST /api/admin/csv/products/commit` — the atomic apply.
///
/// Plans inside the transaction with every product row locked (`FOR UPDATE`),
/// so the stock conflict decision and the UPDATE see the same live counts.
/// An invalid spreadsheet writes nothing and answers 400 with the full preview
/// body (the same errors the operator saw in the dry run); a valid one writes
/// products, content, pieces and the audit row, then fires the `catalogue`
/// revalidation webhook.
pub async fn commit_products_csv(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<ProductsCsvRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let actor = actor(user)?;
    let req = json_body(body)?;
    validate_conflict_mode(&req.stock_conflict_mode)?;
    let table = parse_products_csv(&req.csv).map_err(|message| bad_request(&message))?;
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open the CSV commit", &e))?;
    let plan = plan_products_csv(&mut tx, &table, &req, true)
        .await
        .map_err(|e| state::database_unavailable("failed to plan the CSV commit", &e))?;
    if !plan.errors.is_empty() {
        // Nothing is written: drop the transaction (rollback) and answer with
        // the preview so the operator sees every row error.
        drop(tx);
        return Ok((StatusCode::BAD_REQUEST, Json(build_products_preview(&plan, false))).into_response());
    }
    let has_changes = plan
        .plans
        .iter()
        .any(|planned| planned.action != PlannedAction::Unchanged);
    if has_changes {
        apply_products_csv(&mut tx, &actor, &req, &plan).await?;
    }
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit the CSV sync", &e))?;
    if has_changes {
        admin_crud::fire_revalidation("catalogue");
    }
    Ok(Json(build_products_preview(&plan, false)).into_response())
}

// ---------------------------------------------------------------------------
// Handlers: site content
// ---------------------------------------------------------------------------

/// `GET /api/admin/csv/site-content` — every site copy row as an Excel-ready
/// CSV: `key` plus one column per configured locale (default `en,zh-HK`).
pub async fn export_site_content_csv(State(app): State<AppState>) -> Result<Response, ApiError> {
    let pool = state::require_pool(&app)?;
    let locales = config::locales();
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT key, locale, value FROM site_content ORDER BY key, locale")
            .fetch_all(pool)
            .await
            .map_err(|e| state::database_unavailable("failed to load site content", &e))?;
    let mut grouped: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (key, locale, value) in rows {
        grouped.entry(key).or_default().insert(locale, value);
    }
    let mut header = vec!["key".to_string()];
    header.extend(locales.iter().cloned());
    let mut body = csv_line(&header);
    for (key, values) in &grouped {
        let mut cells = vec![key.clone()];
        for locale in &locales {
            cells.push(values.get(locale).cloned().unwrap_or_default());
        }
        body.push_str(&csv_line(&cells));
    }
    csv_http_response("catalogue-site-content.csv", body)
}

/// `POST /api/admin/csv/site-content/preview` — dry run of the bilingual copy
/// diff: per-(key, locale) before/after, with blanks meaning delete.
pub async fn preview_site_content_csv(
    State(app): State<AppState>,
    body: Result<Json<SiteContentCsvRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let req = json_body(body)?;
    let locales = config::locales();
    let table =
        parse_site_content_csv(&req.csv, &locales).map_err(|message| bad_request(&message))?;
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open the CSV preview", &e))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(|e| state::database_unavailable("failed to set preview isolation", &e))?;
    let plan = plan_site_content_csv(&mut tx, &table, &locales)
        .await
        .map_err(|e| state::database_unavailable("failed to plan the CSV preview", &e))?;
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit the preview read", &e))?;
    Ok(Json(build_site_content_preview(&plan, true)).into_response())
}

/// `POST /api/admin/csv/site-content/commit` — applies the copy diff in one
/// transaction, writes one `csv_bulk_sync` audit row and fires the
/// `site-content` revalidation webhook.
pub async fn commit_site_content_csv(
    State(app): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    body: Result<Json<SiteContentCsvRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let actor = actor(user)?;
    let req = json_body(body)?;
    let locales = config::locales();
    let table =
        parse_site_content_csv(&req.csv, &locales).map_err(|message| bad_request(&message))?;
    let pool = state::require_pool(&app)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| state::database_unavailable("failed to open the CSV commit", &e))?;
    let plan = plan_site_content_csv(&mut tx, &table, &locales)
        .await
        .map_err(|e| state::database_unavailable("failed to plan the CSV commit", &e))?;
    if !plan.errors.is_empty() {
        drop(tx);
        return Ok(
            (StatusCode::BAD_REQUEST, Json(build_site_content_preview(&plan, false)))
                .into_response(),
        );
    }
    let has_changes = plan
        .plans
        .iter()
        .any(|planned| planned.action != PlannedAction::Unchanged);
    if has_changes {
        apply_site_content_csv(&mut tx, &actor, &plan).await?;
    }
    tx.commit()
        .await
        .map_err(|e| state::database_unavailable("failed to commit the site-content sync", &e))?;
    if has_changes {
        admin_crud::fire_revalidation("site-content");
    }
    Ok(Json(build_site_content_preview(&plan, false)).into_response())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::str::FromStr;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Once};
    use std::time::Duration;

    use axum::body::to_bytes;
    use axum::http::HeaderMap;
    use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
    use tokio::sync::Semaphore;

    // -- pure parser tests ------------------------------------------------

    #[test]
    fn csv_parser_handles_rfc4180_quoting() {
        let records = parse_csv("a,b\r\n\"x,y\",\"line1\nline2\"\r\n\"he said \"\"hi\"\"\",\r\n")
            .unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].line, 1);
        assert_eq!(records[0].fields, vec!["a", "b"]);
        assert_eq!(records[1].line, 2);
        assert_eq!(records[1].fields, vec!["x,y", "line1\nline2"]);
        // The embedded newline advanced the physical line count.
        assert_eq!(records[2].line, 4);
        assert_eq!(records[2].fields, vec!["he said \"hi\"", ""]);
    }

    #[test]
    fn csv_parser_skips_blank_lines_and_accepts_a_final_record() {
        let records = parse_csv("a\n\nb").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].fields, vec!["a"]);
        assert_eq!(records[1].line, 3);
        assert_eq!(records[1].fields, vec!["b"]);
        // A quoted empty field is a real field, not a blank line.
        let records = parse_csv("\"\"\n").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].fields, vec![""]);
    }

    #[test]
    fn csv_parser_rejects_malformed_quoting() {
        assert!(parse_csv("\"unterminated").is_err());
        assert!(parse_csv("ab\"c").is_err());
        assert!(parse_csv("\"a\"x").is_err());
    }

    #[test]
    fn csv_escape_round_trips_through_the_parser() {
        let cells = vec![
            "plain".to_string(),
            "with,comma".to_string(),
            "with \"quote\"".to_string(),
            "multi\nline".to_string(),
            "傳承".to_string(),
        ];
        let body = csv_line(&cells);
        let records = parse_csv(&body).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].fields, cells);
    }

    #[test]
    fn scientific_notation_detection_matches_excel_output() {
        for cell in ["1.23E+10", "8.52E+10", "1e5", "-2.5e-3", " +3.0E+7 "] {
            assert!(looks_like_scientific_notation(cell), "{cell}");
        }
        for cell in ["1615", "1615.50", "PH 5", "1931 / 2026", "", "e10", "1.2.3"] {
            assert!(!looks_like_scientific_notation(cell), "{cell}");
        }
    }

    #[test]
    fn price_formatting_is_excel_readable_and_round_trips() {
        assert_eq!(format_price_hkd(161_500), "1615");
        assert_eq!(format_price_hkd(161_550), "1615.50");
        assert_eq!(format_price_hkd(0), "0");
        assert_eq!(parse_price_hkd("1615").unwrap(), 161_500);
        assert_eq!(parse_price_hkd("1615.5").unwrap(), 161_550);
        assert_eq!(parse_price_hkd("1615.50").unwrap(), 161_550);
        for bad in ["", "-1", "1.234", "1,615", "abc", "$1615", "1..2"] {
            assert!(parse_price_hkd(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pieces_are_hash_prefixed_and_parse_from_all_export_forms() {
        assert_eq!(format_pieces(&[2, 1]), "#01;#02");
        assert_eq!(format_pieces(&[]), "");
        assert_eq!(format_pieces(&[100, 3]), "#03;#100");
        assert_eq!(parse_pieces("#01;#02").unwrap(), vec![1, 2]);
        assert_eq!(parse_pieces("1;2").unwrap(), vec![1, 2]);
        assert_eq!(parse_pieces("1,2").unwrap(), vec![1, 2]);
        assert!(parse_pieces(" #2 , 1 , 1 ").is_err());
        assert!(parse_pieces("0").is_err());
        assert!(parse_pieces("a").is_err());
    }

    #[test]
    fn stock_resolution_never_rewinds_a_concurrent_sale() {
        let db = StockCounts {
            total: 4,
            reserved: 1,
            quarantined: 0,
        };
        // The race-review scenario: exported at 5, a customer consumed one,
        // and the CSV carries no operator stock edit. The live count wins and
        // is NOT rewritten.
        let decision =
            resolve_stock_existing(true, "keep_db", &db, Some(5), Some(5), 0).unwrap();
        assert_eq!(decision.effective, 4);
        assert!(!decision.write);
        assert!(!decision.conflict);
        // No apply_stock request: same untouched result even for a wild edit.
        let decision =
            resolve_stock_existing(false, "force_override", &db, Some(99), Some(5), 0).unwrap();
        assert_eq!(decision.effective, 4);
        assert!(!decision.write);
    }

    #[test]
    fn stock_conflict_modes_resolve_against_the_locked_live_count() {
        let db = StockCounts {
            total: 4,
            reserved: 1,
            quarantined: 0,
        };
        // The operator edited stock (delta 2) while the live count moved.
        let keep = resolve_stock_existing(true, "keep_db", &db, Some(5), Some(5), 2).unwrap();
        assert!(keep.conflict, "a moved live count must be flagged");
        assert_eq!(keep.effective, 4);
        assert!(keep.warning.unwrap().contains("stock_conflict_mode=keep_db"));

        let delta = resolve_stock_existing(true, "apply_delta", &db, Some(5), Some(5), 2).unwrap();
        assert_eq!(delta.effective, 6, "new stock lands on top of the live count");

        let force =
            resolve_stock_existing(true, "force_override", &db, Some(7), Some(5), 0).unwrap();
        assert_eq!(force.effective, 7);

        // No conflict: a corrected number still wins via force_override.
        let corrected =
            resolve_stock_existing(true, "force_override", &StockCounts { total: 5, reserved: 0, quarantined: 0 }, Some(3), Some(5), 0)
                .unwrap();
        assert_eq!(corrected.effective, 3);
        assert!(!corrected.conflict);
    }

    #[test]
    fn stock_resolution_enforces_the_stock_01_invariant() {
        let db = StockCounts {
            total: 4,
            reserved: 3,
            quarantined: 2,
        };
        let error =
            resolve_stock_existing(true, "force_override", &db, Some(3), Some(4), 0).unwrap_err();
        assert!(error.contains("stock_reserved + stock_quarantined"), "{error}");
        let error =
            resolve_stock_existing(true, "apply_delta", &db, Some(4), Some(4), -5).unwrap_err();
        assert!(error.contains("stock_reserved + stock_quarantined"), "{error}");
        // The exact boundary is legal.
        let db = StockCounts {
            total: 5,
            reserved: 3,
            quarantined: 2,
        };
        assert_eq!(
            resolve_stock_existing(true, "force_override", &db, Some(5), Some(5), 0)
                .unwrap()
                .effective,
            5
        );
    }

    #[test]
    fn new_products_take_the_csv_stock_only_when_apply_stock_is_set() {
        assert_eq!(resolve_stock_new(true, Some(3), 0).unwrap(), 3);
        assert_eq!(resolve_stock_new(true, None, 2).unwrap(), 2);
        assert_eq!(resolve_stock_new(false, Some(3), 0).unwrap(), 0);
        assert!(resolve_stock_new(true, Some(-1), 0).is_err());
    }

    #[test]
    fn product_header_validation_names_missing_and_unknown_columns() {
        let missing = "id,slug\n1,x\n";
        let error = parse_products_csv(missing).unwrap_err();
        assert!(error.contains("missing required column(s)"), "{error}");
        let unknown = format!(
            "{},bogus\n",
            PRODUCTS_CSV_COLUMNS
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let error = parse_products_csv(&unknown).unwrap_err();
        assert!(error.contains("unknown column \"bogus\""), "{error}");
        // The full export header (with `pieces`) parses; so does the 29-column
        // header from a file written before the pieces column existed.
        let header = PRODUCTS_CSV_COLUMNS
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_products_csv(&format!("{header}\n")).is_ok());
        let legacy = PRODUCTS_CSV_COLUMNS
            .iter()
            .filter(|c| **c != "pieces")
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_products_csv(&format!("{legacy}\n")).is_ok());
    }

    #[test]
    fn site_content_header_must_match_the_configured_locales() {
        let locales = vec!["en".to_string(), "zh-HK".to_string()];
        assert!(parse_site_content_csv("key,en,zh-HK\n", &locales).is_ok());
        // Order-insensitive.
        assert!(parse_site_content_csv("key,zh-HK,en\n", &locales).is_ok());
        assert!(parse_site_content_csv("key,en\n", &locales).is_err());
        assert!(parse_site_content_csv("en,zh-HK,key\n", &locales).is_err());
        assert!(parse_site_content_csv("key,fr,zh-HK\n", &locales).is_err());
    }

    // -- DB test harness --------------------------------------------------
    //
    // `cargo test --lib` has no access to `tests/common`, so this module owns a
    // compact scratch-database harness with the SAME safety posture: the
    // STOREFRONT_TEST_DATABASE_URL gate, the loopback:55433 allowlist, the
    // ambient-env refusal and a post-run DROP. Each test gets its own database.

    const MIGRATIONS: &[&str] = &[
        "0001_init.sql",
        "0002_catalogue_content.sql",
        "0003_seed_opening_drop.sql",
        "0004_admin.sql",
        "0005_flagship_media_specs.sql",
        "0006_orders_reservations.sql",
        "0007_multi_sku_fps_media.sql",
        "0008_payment_email_session_hardening.sql",
    ];
    const MIGRATIONS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
    const SCRATCH_DB_PORT: u16 = 55433;
    const BODY_LIMIT: usize = 8 * 1024 * 1024;
    const SKIP_BANNER: &str = "\
================================================================================
 csv_sync DB TESTS SKIPPED: STOREFRONT_TEST_DATABASE_URL is not set.
 Expected value shape: postgres://<user>:<password>@localhost:55433/<database>
================================================================================";
    static SKIP_BANNER_PRINTED: Once = Once::new();

    struct TestDb {
        db_name: String,
        pool: PgPool,
        admin_pool: PgPool,
        rt: tokio::runtime::Runtime,
    }

    impl TestDb {
        /// Returns `None` (with one loud banner) when the DB gate is unset.
        fn new(slug: &str) -> Option<TestDb> {
            let url = match std::env::var("STOREFRONT_TEST_DATABASE_URL") {
                Ok(url) if !url.is_empty() => url,
                _ => {
                    SKIP_BANNER_PRINTED.call_once(|| {
                        use std::io::Write;
                        let _ = writeln!(std::io::stderr(), "{SKIP_BANNER}");
                    });
                    return None;
                }
            };
            // Ambient knobs can turn these tests into live-network calls or
            // move the locale pair; refuse them, naming the variable only.
            for name in ["PGOPTIONS", "LOCALES", "REVALIDATE_URL", "REVALIDATE_SECRET"] {
                if std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false) {
                    panic!("ambient {name} must be unset for csv_sync DB tests");
                }
            }
            let opts = PgConnectOptions::from_str(&url)
                .unwrap_or_else(|e| panic!("STOREFRONT_TEST_DATABASE_URL did not parse: {e}"));
            assert!(
                opts.get_socket().is_none(),
                "csv_sync DB tests require the TCP scratch endpoint, not a socket"
            );
            let host = opts
                .get_host()
                .trim_start_matches('[')
                .trim_end_matches(']');
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "::1";
            assert!(
                loopback && opts.get_port() == SCRATCH_DB_PORT,
                "refusing to run csv_sync DB tests against {}:{} (only \
                 localhost:{SCRATCH_DB_PORT} is sanctioned)",
                opts.get_host(),
                opts.get_port()
            );
            let opts = opts
                .ssl_mode(PgSslMode::Disable)
                .options([("statement_timeout", "10s")]);
            let db_name = format!("csv_sync_test_{slug}_{}", std::process::id());
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("failed to build the csv_sync test runtime");
            let (admin_pool, pool) = rt.block_on(async {
                let admin_pool = PgPoolOptions::new()
                    .max_connections(2)
                    .acquire_timeout(Duration::from_secs(5))
                    .connect_with(opts.clone())
                    .await
                    .expect("csv_sync harness: cannot connect the admin pool");
                sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
                    .execute(&admin_pool)
                    .await
                    .expect("csv_sync harness: DROP DATABASE IF EXISTS failed");
                sqlx::raw_sql(&format!("CREATE DATABASE {db_name}"))
                    .execute(&admin_pool)
                    .await
                    .expect("csv_sync harness: CREATE DATABASE failed");
                let pool = PgPoolOptions::new()
                    .max_connections(2)
                    .acquire_timeout(Duration::from_secs(5))
                    .connect_with(opts.database(&db_name))
                    .await
                    .expect("csv_sync harness: cannot connect the test pool");
                (admin_pool, pool)
            });
            let db = TestDb {
                db_name,
                pool,
                admin_pool,
                rt,
            };
            db.apply_migrations();
            Some(db)
        }

        fn pool(&self) -> &PgPool {
            &self.pool
        }

        fn block_on<F: Future>(&self, future: F) -> F::Output {
            self.rt.block_on(future)
        }

        fn apply_migrations(&self) {
            for file_name in MIGRATIONS {
                let path = format!("{MIGRATIONS_DIR}/{file_name}");
                let sql = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("csv_sync harness: cannot read {path}: {e}"));
                self.block_on(async {
                    sqlx::raw_sql(&sql).execute(&self.pool).await.unwrap_or_else(|e| {
                        panic!("csv_sync harness: migration {file_name} failed: {e}")
                    });
                });
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let db_name = self.db_name.clone();
            let cleanup = self.rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(30), async {
                    self.pool.close().await;
                    sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
                        .execute(&self.admin_pool)
                        .await
                })
                .await
            });
            match cleanup {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("csv_sync harness: failed to drop {db_name}: {e}"),
                Err(_) => eprintln!("csv_sync harness: drop of {db_name} timed out"),
            }
        }
    }

    fn app_state(db: &TestDb) -> AppState {
        AppState {
            pool: Some(db.pool.clone()),
            session_key: None,
            login_gate: Arc::new(Semaphore::new(1)),
            admin_ready: Arc::new(AtomicBool::new(true)),
        }
    }

    fn test_actor() -> Option<Extension<CurrentUser>> {
        Some(Extension(CurrentUser {
            username: "csv-test".to_string(),
        }))
    }

    fn clear_data(db: &TestDb) {
        db.block_on(async {
            sqlx::raw_sql(
                "DELETE FROM order_items; DELETE FROM orders; DELETE FROM pieces; \
                 DELETE FROM reference_prices; DELETE FROM product_content; \
                 DELETE FROM products; DELETE FROM site_content;",
            )
            .execute(db.pool())
            .await
            .expect("failed to clear the scratch catalogue");
        });
    }

    fn insert_product(db: &TestDb, slug: &str, price_cents: i64, stock_total: i32) -> i64 {
        db.block_on(async {
            sqlx::query_scalar(
                "INSERT INTO products (slug, brand, model, price_cents, stock_total) \
                 VALUES ($1, 'Test Brand', 'Test Model', $2, $3) RETURNING id",
            )
            .bind(slug)
            .bind(price_cents)
            .bind(stock_total)
            .fetch_one(db.pool())
            .await
            .expect("failed to insert the fixture product")
        })
    }

    async fn read_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), BODY_LIMIT)
            .await
            .expect("response body must be readable");
        serde_json::from_slice(&bytes).expect("response body must be JSON")
    }

    async fn read_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), BODY_LIMIT)
            .await
            .expect("response body must be readable");
        String::from_utf8(bytes.to_vec()).expect("response body must be UTF-8")
    }

    /// Rebuilds a CSV with one cell of the matching row replaced. The output is
    /// plain CRLF CSV (the handler strips the BOM itself, so none is added).
    fn csv_set_cell(csv: &str, match_column: &str, match_value: &str, column: &str, value: &str) -> String {
        let records = parse_csv(strip_utf8_bom(csv)).expect("export must parse");
        let mut rows: Vec<Vec<String>> = records.into_iter().map(|record| record.fields).collect();
        let header = rows.remove(0);
        let match_index = header
            .iter()
            .position(|name| name == match_column)
            .unwrap_or_else(|| panic!("no column {match_column}"));
        let column_index = header
            .iter()
            .position(|name| name == column)
            .unwrap_or_else(|| panic!("no column {column}"));
        for row in &mut rows {
            if row.get(match_index).map(String::as_str) == Some(match_value) {
                row[column_index] = value.to_string();
            }
        }
        let mut out = String::new();
        for row in std::iter::once(header).chain(rows) {
            out.push_str(&csv_line(&row));
        }
        out
    }

    fn products_request(csv: String, apply_stock: bool, mode: &str) -> ProductsCsvRequest {
        ProductsCsvRequest {
            csv,
            apply_stock,
            stock_conflict_mode: mode.to_string(),
            allow_slug_rename: false,
        }
    }

    fn product_stock(db: &TestDb, id: i64) -> (i32, i32, i32) {
        db.block_on(async {
            sqlx::query_as("SELECT stock_total, stock_reserved, stock_quarantined FROM products WHERE id = $1")
                .bind(id)
                .fetch_one(db.pool())
                .await
                .expect("product must exist")
        })
    }

    // -- DB tests ---------------------------------------------------------

    #[test]
    fn db_export_products_csv_is_excel_ready() {
        let Some(db) = TestDb::new("export") else {
            return;
        };
        clear_data(&db);
        let id = insert_product(&db, "ph-5", 161_500, 7);
        db.block_on(async {
            sqlx::query("INSERT INTO pieces (piece_no, product_id) VALUES (1, $1), (2, $1)")
                .bind(id)
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO product_content (product_id, locale, field, value) \
                 VALUES ($1, 'zh-HK', 'heritage', '傳承'), ($1, 'en', 'colourway', 'White')",
            )
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        });
        let state = app_state(&db);
        let response = db
            .block_on(export_products_csv(State(state)))
            .expect("export must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let headers: HeaderMap = response.headers().clone();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            headers.get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"catalogue-products.csv\""
        );
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        let text = db.block_on(read_text(response));
        assert!(text.starts_with(CSV_BOM), "Excel needs the UTF-8 BOM");
        let records = parse_csv(strip_utf8_bom(&text)).unwrap();
        assert_eq!(records.len(), 2, "header + one product");
        assert_eq!(records[0].fields.len(), 30);
        assert_eq!(
            records[0]
                .fields
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            PRODUCTS_CSV_COLUMNS
        );
        let row = &records[1].fields;
        let get = |column: &str| {
            let index = PRODUCTS_CSV_COLUMNS
                .iter()
                .position(|name| *name == column)
                .unwrap();
            row[index].as_str()
        };
        assert_eq!(get("id"), id.to_string());
        assert_eq!(get("slug"), "ph-5");
        assert_eq!(get("price_hkd"), "1615");
        assert_eq!(get("snapshot_stock_total"), "7");
        assert_eq!(get("stock_total"), "7");
        assert_eq!(get("stock_delta"), "0");
        assert_eq!(get("pieces"), "#01;#02");
        assert_eq!(get("zh_hk_heritage"), "傳承");
        assert_eq!(get("en_colourway"), "White");
        assert_eq!(get("row_version_hash").len(), 16);
    }

    #[test]
    fn db_concurrent_order_race_leaves_live_stock_untouched() {
        let Some(db) = TestDb::new("race") else {
            return;
        };
        clear_data(&db);
        let id = insert_product(&db, "ph-5", 745_000, 5);
        let state = app_state(&db);
        let export = db
            .block_on(export_products_csv(State(state.clone())))
            .expect("export must succeed");
        let csv_text = db.block_on(read_text(export));
        assert!(csv_text.starts_with(CSV_BOM));

        // A customer pays while the spreadsheet is open: the money path
        // consumes one unit from stock_total.
        db.block_on(async {
            sqlx::query("UPDATE products SET stock_total = 4 WHERE id = $1")
                .bind(id)
                .execute(db.pool())
                .await
                .unwrap();
        });

        // 1. No operator stock edit -> the live count must survive a commit.
        let request = products_request(csv_text.clone(), true, "keep_db");
        let response = db
            .block_on(preview_products_csv(State(state.clone()), Ok(Json(request))))
            .expect("preview must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["new_count"], 0);
        assert_eq!(preview["updated_count"], 0);
        assert_eq!(preview["unchanged_count"], 1);
        assert_eq!(preview["conflict_count"], 0);
        assert_eq!(preview["rows"][0]["action"], "unchanged");
        assert_eq!(preview["rows"][0]["stock_conflict"], false);
        let request = products_request(csv_text.clone(), true, "keep_db");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(product_stock(&db, id).0, 4, "the sale must not be rewound");

        // 2. A delta edit resolves on top of the LIVE count (apply_delta).
        let edited = csv_set_cell(&csv_text, "slug", "ph-5", "stock_delta", "2");
        let request = products_request(edited.clone(), true, "apply_delta");
        let response = db
            .block_on(preview_products_csv(State(state.clone()), Ok(Json(request))))
            .expect("preview must succeed");
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["conflict_count"], 1);
        assert!(preview["rows"][0]["stock_conflict"].as_bool().unwrap());
        assert!(preview["rows"][0]["warning"]
            .as_str()
            .unwrap()
            .contains("apply_delta"));
        let request = products_request(edited.clone(), true, "apply_delta");
        db.block_on(commit_products_csv(
            State(state.clone()),
            test_actor(),
            Ok(Json(request)),
        ))
        .expect("commit must succeed");
        assert_eq!(product_stock(&db, id).0, 6, "4 live + 2 added");

        // 3. keep_db keeps the live count even with a delta edit.
        let request = products_request(edited.clone(), true, "keep_db");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(product_stock(&db, id).0, 6);

        // 4. force_override writes the CSV number.
        let overridden = csv_set_cell(&edited, "slug", "ph-5", "stock_total", "9");
        let request = products_request(overridden, true, "force_override");
        db.block_on(commit_products_csv(
            State(state.clone()),
            test_actor(),
            Ok(Json(request)),
        ))
        .expect("commit must succeed");
        assert_eq!(product_stock(&db, id).0, 9);

        // 5. The STOCK-01 invariant rejects an override below reservations.
        db.block_on(async {
            sqlx::query(
                "UPDATE products SET stock_reserved = 8, stock_quarantined = 2 WHERE id = $1",
            )
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        });
        let too_low = csv_set_cell(&csv_text, "slug", "ph-5", "stock_total", "3");
        let request = products_request(too_low, true, "force_override");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit handler must answer");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], false);
        assert!(preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stock_reserved + stock_quarantined"));
        assert_eq!(product_stock(&db, id).0, 9, "an invalid commit writes nothing");
    }

    #[test]
    fn db_commit_updates_content_pieces_and_audit_log() {
        let Some(db) = TestDb::new("commit") else {
            return;
        };
        clear_data(&db);
        let id = insert_product(&db, "ph-5", 161_500, 2);
        db.block_on(async {
            sqlx::query("INSERT INTO pieces (piece_no, product_id) VALUES (1, $1)")
                .bind(id)
                .execute(db.pool())
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO product_content (product_id, locale, field, value) \
                 VALUES ($1, 'en', 'colourway', 'Old White')",
            )
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        });
        let state = app_state(&db);
        let export = db
            .block_on(export_products_csv(State(state.clone())))
            .expect("export must succeed");
        let csv_text = db.block_on(read_text(export));
        let edited = csv_set_cell(&csv_text, "slug", "ph-5", "price_hkd", "1999.90");
        let edited = csv_set_cell(&edited, "slug", "ph-5", "brand", "Edited Brand");
        let edited = csv_set_cell(&edited, "slug", "ph-5", "en_colourway", "Opal White");
        let edited = csv_set_cell(&edited, "slug", "ph-5", "zh_hk_heritage", "傳承故事");
        let edited = csv_set_cell(&edited, "slug", "ph-5", "pieces", "#03");
        // Stock untouched: these text edits must not move stock.
        let request = products_request(edited.clone(), false, "keep_db");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["updated_count"], 1);

        db.block_on(async {
            let (price,): (i64,) =
                sqlx::query_as("SELECT price_cents FROM products WHERE id = $1")
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(price, 199_990);
            let content: Vec<(String, String)> = sqlx::query_as(
                "SELECT field, value FROM product_content WHERE product_id = $1 ORDER BY field",
            )
            .bind(id)
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(
                content,
                vec![
                    ("colourway".to_string(), "Opal White".to_string()),
                    ("heritage".to_string(), "傳承故事".to_string()),
                ]
            );
            let pieces: Vec<(i32,)> =
                sqlx::query_as("SELECT piece_no FROM pieces ORDER BY piece_no")
                    .fetch_all(db.pool())
                    .await
                    .unwrap();
            assert_eq!(pieces, vec![(3,)]);
            let audits: Vec<(String, String)> = sqlx::query_as(
                "SELECT action, entity FROM audit_log ORDER BY id DESC LIMIT 1",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(audits, vec![("csv_bulk_sync".to_string(), "products".to_string())]);
            let (stock_total,): (i32,) =
                sqlx::query_as("SELECT stock_total FROM products WHERE id = $1")
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(stock_total, 2, "a text-only import never moves stock");
        });
    }

    #[test]
    fn db_scientific_notation_and_piece_collisions_are_rejected() {
        let Some(db) = TestDb::new("reject") else {
            return;
        };
        clear_data(&db);
        let first = insert_product(&db, "ph-5", 161_500, 2);
        let second = insert_product(&db, "ph-3-2", 900_000, 1);
        db.block_on(async {
            sqlx::query("INSERT INTO pieces (piece_no, product_id) VALUES (1, $1)")
                .bind(first)
                .execute(db.pool())
                .await
                .unwrap();
        });
        let state = app_state(&db);
        let export = db
            .block_on(export_products_csv(State(state.clone())))
            .expect("export must succeed");
        let csv_text = db.block_on(read_text(export));

        // A scientific-notation price is a row error, not a mangled import.
        let scientific = csv_set_cell(&csv_text, "slug", "ph-5", "price_hkd", "1.23E+10");
        let request = products_request(scientific, false, "keep_db");
        let response = db
            .block_on(preview_products_csv(State(state.clone()), Ok(Json(request))))
            .expect("preview must answer");
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], false);
        assert_eq!(preview["errors"][0]["column"], "price_hkd");
        assert!(preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("scientific notation"));

        // Piece collision with another SKU.
        let collision = csv_set_cell(&csv_text, "slug", "ph-3-2", "pieces", "#01");
        let request = products_request(collision, false, "keep_db");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit handler must answer");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let preview = db.block_on(read_json(response));
        assert!(preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("already belongs to another product"));
        db.block_on(async {
            let (owner,): (i64,) =
                sqlx::query_as("SELECT product_id FROM pieces WHERE piece_no = 1")
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(owner, first, "an invalid commit must not move pieces");
            let (total,): (i32,) =
                sqlx::query_as("SELECT stock_total FROM products WHERE id = $1")
                    .bind(second)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(total, 1, "an invalid commit must not write anything");
        });
    }

    #[test]
    fn db_slug_rename_is_gated_and_cascades_to_orders() {
        let Some(db) = TestDb::new("rename") else {
            return;
        };
        clear_data(&db);
        let id = insert_product(&db, "old-slug", 100_000, 1);
        let order_id: i64 = db.block_on(async {
            let order_id: i64 = sqlx::query_scalar(
                "INSERT INTO orders (order_code, product_id, product_slug, quantity, \
                     unit_price_cents, total_cents, currency, payment_method, status, \
                     customer_name, customer_email, expires_at) \
                 VALUES ('CSV-1', $1, 'old-slug', 1, 100_000, 100_000, 'HKD', 'fps', \
                     'reserved', 'Test', 'test@example.com', now() + interval '1 day') \
                 RETURNING id",
            )
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO order_items (order_id, product_id, product_slug, quantity, \
                     unit_price_cents, line_total_cents) \
                 VALUES ($1, $2, 'old-slug', 1, 100_000, 100_000)",
            )
            .bind(order_id)
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
            order_id
        });
        let state = app_state(&db);
        let export = db
            .block_on(export_products_csv(State(state.clone())))
            .expect("export must succeed");
        let csv_text = db.block_on(read_text(export));
        let renamed = csv_set_cell(&csv_text, "slug", "old-slug", "slug", "new-slug");

        // Rename without the opt-in is a row error.
        let request = products_request(renamed.clone(), false, "keep_db");
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit handler must answer");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let preview = db.block_on(read_json(response));
        assert!(preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("slug rename not allowed"));

        // With the opt-in the rename cascades to orders and order_items.
        let mut request = products_request(renamed, false, "keep_db");
        request.allow_slug_rename = true;
        let response = db
            .block_on(commit_products_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        db.block_on(async {
            let (slug,): (String,) =
                sqlx::query_as("SELECT slug FROM products WHERE id = $1")
                    .bind(id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(slug, "new-slug");
            let (order_slug,): (String,) =
                sqlx::query_as("SELECT product_slug FROM orders WHERE id = $1")
                    .bind(order_id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(order_slug, "new-slug");
            let (item_slug,): (String,) =
                sqlx::query_as("SELECT product_slug FROM order_items WHERE order_id = $1")
                    .bind(order_id)
                    .fetch_one(db.pool())
                    .await
                    .unwrap();
            assert_eq!(item_slug, "new-slug");
        });
    }

    #[test]
    fn db_site_content_round_trips_and_deletes_blanks() {
        let Some(db) = TestDb::new("site_content") else {
            return;
        };
        clear_data(&db);
        db.block_on(async {
            sqlx::raw_sql(
                "INSERT INTO site_content (key, locale, value) VALUES \
                     ('tagline', 'en', 'Curated light'), \
                     ('tagline', 'zh-HK', '精選燈飾'), \
                     ('policy.warranty', 'en', 'One year')",
            )
            .execute(db.pool())
            .await
            .unwrap();
        });
        let state = app_state(&db);
        let export = db
            .block_on(export_site_content_csv(State(state.clone())))
            .expect("export must succeed");
        let text = db.block_on(read_text(export));
        assert!(text.starts_with(CSV_BOM));
        let records = parse_csv(strip_utf8_bom(&text)).unwrap();
        assert_eq!(records[0].fields, vec!["key", "en", "zh-HK"]);
        assert_eq!(records[1].fields, vec!["policy.warranty", "One year", ""]);
        assert_eq!(records[2].fields, vec!["tagline", "Curated light", "精選燈飾"]);

        // Edit one value, clear another, add a new key.
        let edited = csv_set_cell(&text, "key", "tagline", "en", "Curated light, reimagined");
        let edited = csv_set_cell(&edited, "key", "tagline", "zh-HK", "");
        let mut edited = edited.trim_end().to_string();
        edited.push_str("\r\nnew.key,Hello,你好\r\n");
        let request = SiteContentCsvRequest {
            csv: edited.clone(),
        };
        let response = db
            .block_on(preview_site_content_csv(State(state.clone()), Ok(Json(request))))
            .expect("preview must succeed");
        let preview = db.block_on(read_json(response));
        assert_eq!(preview["valid"], true);
        assert_eq!(preview["new_count"], 1);
        assert_eq!(preview["updated_count"], 1);

        let request = SiteContentCsvRequest {
            csv: edited.clone(),
        };
        let response = db
            .block_on(commit_site_content_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit must succeed");
        assert_eq!(response.status(), StatusCode::OK);
        db.block_on(async {
            let rows: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT key, locale, value FROM site_content ORDER BY key, locale",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(
                rows,
                vec![
                    (
                        "new.key".to_string(),
                        "en".to_string(),
                        "Hello".to_string()
                    ),
                    (
                        "new.key".to_string(),
                        "zh-HK".to_string(),
                        "你好".to_string()
                    ),
                    (
                        "policy.warranty".to_string(),
                        "en".to_string(),
                        "One year".to_string()
                    ),
                    (
                        "tagline".to_string(),
                        "en".to_string(),
                        "Curated light, reimagined".to_string()
                    ),
                ],
                "the blanked zh-HK row must be deleted"
            );
            let audits: Vec<(String, String)> = sqlx::query_as(
                "SELECT action, entity FROM audit_log ORDER BY id DESC LIMIT 1",
            )
            .fetch_all(db.pool())
            .await
            .unwrap();
            assert_eq!(
                audits,
                vec![("csv_bulk_sync".to_string(), "site_content".to_string())]
            );
        });

        // A duplicate key in one CSV is a row error and commits nothing.
        let duplicate = format!("{edited}new.key,Again,再見\r\n");
        let request = SiteContentCsvRequest { csv: duplicate };
        let response = db
            .block_on(commit_site_content_csv(
                State(state.clone()),
                test_actor(),
                Ok(Json(request)),
            ))
            .expect("commit handler must answer");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let preview = db.block_on(read_json(response));
        assert!(preview["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("duplicate key"));
    }
}
