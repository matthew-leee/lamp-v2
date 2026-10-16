//! `GET /api/catalogue` — the active drop for the storefront and admin console.
//!
//! Assembly rules (D1.2):
//! - Active products only (`WHERE active`, re-checked in assembly as defence in
//!   depth and to keep the rule unit-testable).
//! - NO stock fields anywhere in this response: live stock has its own
//!   fail-closed channel (`GET /api/products`) which backs the availability
//!   badge; a cached catalogue read can never be mistaken for stock truth.
//! - Derived counts are COUNTs computed at query/assembly time
//!   (`total_pieces`, `units_in_drop`), never constants (DROP-01).
//! - Assembly happens in Rust from simple per-table queries — no SQL JSON
//!   aggregation — so the response shape is unit-testable without a database.

use std::collections::{BTreeMap, HashMap};

use axum::{extract::State, Json};
use chrono::NaiveDate;
use serde::Serialize;
use sqlx::{FromRow, PgPool};

use crate::state::{self, ApiError, AppState};

/// Placeholder-art configuration for a SKU. The `art_variant` column is
/// CHECK-constrained to `'dome' | 'shades' | 'shades-tall' | 'piano'`
/// (migration 0005): one variant per verified silhouette — the Panthella
/// dome, the PH two/three-shade stack at two heights, and the PH 1/1 Piano.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtVariant {
    Dome,
    Shades,
    /// Hyphenated DB spelling (serde's `lowercase` alone would yield
    /// `"shadestall"`), pinned by the serialization test.
    #[serde(rename = "shades-tall")]
    ShadesTall,
    Piano,
    /// Plain-photo / non-schematic items (MEDIA-01c).
    None,
}

impl ArtVariant {
    /// Maps a DB value to its variant. Anything that is not one of the five
    /// CHECK-constrained values falls back to `Dome`, the schema default: an
    /// unexpected value (only reachable if the CHECK constraint is dropped)
    /// must not take down the whole catalogue read.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "shades" => Self::Shades,
            "shades-tall" => Self::ShadesTall,
            "piano" => Self::Piano,
            "none" => Self::None,
            _ => Self::Dome,
        }
    }
}

/// Per-locale product copy. Each field serializes only when its
/// `product_content` row exists — the API never invents copy.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ProductContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colourway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heritage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub electrical: Option<String>,
}

impl ProductContent {
    /// Applies one `(field, value)` row. Unknown fields (impossible via the
    /// table CHECK) are ignored rather than failing the read.
    fn apply(&mut self, field: &str, value: String) {
        match field {
            "model_alt" => self.model_alt = Some(value),
            "colourway" => self.colourway = Some(value),
            "heritage" => self.heritage = Some(value),
            "electrical" => self.electrical = Some(value),
            _ => {}
        }
    }
}

/// Labelled reference price (TRUST-02). `null` in the response when no
/// verified reference row exists — the UI then says verification pending
/// instead of an invented number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferencePrice {
    pub min_price_cents: i64,
    pub max_price_cents: i64,
    pub source_name: String,
    pub source_url: String,
    /// DB DATE, serialized as a `YYYY-MM-DD` string (chrono's NaiveDate serde
    /// format; asserted in tests).
    pub checked_on: NaiveDate,
}

/// One active SKU in the catalogue response. Key set is fixed and deliberately
/// excludes stock: see the module docs.
#[derive(Debug, Serialize)]
pub struct CatalogueProduct {
    pub slug: String,
    pub brand: String,
    pub model: String,
    pub price_cents: i64,
    pub family: String,
    pub active: bool,
    pub art_variant: ArtVariant,
    pub art_hue: i32,
    /// Verified demo imagery (TRUST-02): daylight / lit views of the actual
    /// piece. `''` until an admin or a migration fills them: the UI shows its
    /// placeholder art instead of an invented photo.
    pub image_day_url: String,
    pub image_lit_url: String,
    /// Architectural specs (per-SKU facts, not locale copy — 0005).
    pub designer: String,
    pub design_year: String,
    pub dimensions: String,
    pub light_source: String,
    pub content: BTreeMap<String, ProductContent>,
    pub reference_price: Option<ReferencePrice>,
    pub pieces: Vec<i32>,
    pub units_in_drop: i64,
}

/// `GET /api/catalogue` response.
#[derive(Debug, Serialize)]
pub struct CatalogueResponse {
    /// `COUNT(*)` over `pieces` for the whole drop.
    pub total_pieces: i64,
    /// Active products, ordered by id.
    pub products: Vec<CatalogueProduct>,
}

/// `GET /api/catalogue` — the active drop assembled from Postgres.
pub async fn get_catalogue(
    State(app): State<AppState>,
) -> Result<Json<CatalogueResponse>, ApiError> {
    let pool = state::require_pool(&app)?;
    let response = load(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to load catalogue", &e))?;
    Ok(Json(response))
}

/// Loads the four catalogue tables with simple runtime queries and assembles
/// the response in Rust.
/**
 * Loads the four catalogue tables and the COUNT inside ONE REPEATABLE READ
 * transaction, so the response is a coherent snapshot: the D2 admin console
 * mutates these tables concurrently, and a read split across implicit
 * transactions could mix pre- and post-mutation state (a piece moved between
 * the pieces read and the COUNT read would make units_in_drop and
 * total_pieces describe different instants).
 */
async fn load(pool: &PgPool) -> Result<CatalogueResponse, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;

    let products = sqlx::query_as::<_, ProductRow>(
        "SELECT id, slug, brand, model, price_cents, family, active, art_variant, art_hue, \
             image_day_url, image_lit_url, designer, design_year, dimensions, light_source \
         FROM products WHERE active ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await?;

    let content = sqlx::query_as::<_, ContentRow>(
        "SELECT product_id, locale, field, value \
         FROM product_content ORDER BY product_id, locale, field",
    )
    .fetch_all(&mut *tx)
    .await?;

    let reference_prices = sqlx::query_as::<_, ReferencePriceRow>(
        "SELECT product_id, min_price_cents, max_price_cents, source_name, source_url, checked_on \
         FROM reference_prices ORDER BY product_id",
    )
    .fetch_all(&mut *tx)
    .await?;

    let pieces =
        sqlx::query_as::<_, PieceRow>("SELECT piece_no, product_id FROM pieces ORDER BY piece_no")
            .fetch_all(&mut *tx)
            .await?;

    // Derived count (DROP-01): the drop total is a COUNT, never a constant,
    // consistent with the piece rows above because it reads the same
    // snapshot.
    let (total_pieces,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pieces")
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(assemble(
        products,
        content,
        reference_prices,
        pieces,
        total_pieces,
    ))
}

// Row shapes for the four runtime queries (runtime `query_as`, never the
// `query!` macro: the build must not need DATABASE_URL or a .sqlx cache).

#[derive(Debug, FromRow)]
struct ProductRow {
    id: i64,
    slug: String,
    brand: String,
    model: String,
    price_cents: i64,
    family: String,
    active: bool,
    art_variant: String,
    art_hue: i32,
    image_day_url: String,
    image_lit_url: String,
    designer: String,
    design_year: String,
    dimensions: String,
    light_source: String,
}

#[derive(Debug, FromRow)]
struct ContentRow {
    product_id: i64,
    locale: String,
    field: String,
    value: String,
}

#[derive(Debug, FromRow)]
struct ReferencePriceRow {
    product_id: i64,
    min_price_cents: i64,
    max_price_cents: i64,
    source_name: String,
    source_url: String,
    checked_on: NaiveDate,
}

#[derive(Debug, FromRow)]
struct PieceRow {
    piece_no: i32,
    product_id: i64,
}

/// Pure assembly: merges the four row sets into the catalogue response.
/// Inactive products are dropped here too (belt and braces over the SQL
/// `WHERE active`), which keeps the visibility rule testable without a DB.
fn assemble(
    products: Vec<ProductRow>,
    content: Vec<ContentRow>,
    reference_prices: Vec<ReferencePriceRow>,
    pieces: Vec<PieceRow>,
    total_pieces: i64,
) -> CatalogueResponse {
    let mut content_by_product: HashMap<i64, BTreeMap<String, ProductContent>> = HashMap::new();
    for row in content {
        content_by_product
            .entry(row.product_id)
            .or_default()
            .entry(row.locale)
            .or_default()
            .apply(&row.field, row.value);
    }

    let mut reference_by_product: HashMap<i64, ReferencePrice> = reference_prices
        .into_iter()
        .map(|row| {
            (
                row.product_id,
                ReferencePrice {
                    min_price_cents: row.min_price_cents,
                    max_price_cents: row.max_price_cents,
                    source_name: row.source_name,
                    source_url: row.source_url,
                    checked_on: row.checked_on,
                },
            )
        })
        .collect();

    let mut pieces_by_product: HashMap<i64, Vec<i32>> = HashMap::new();
    for row in pieces {
        pieces_by_product
            .entry(row.product_id)
            .or_default()
            .push(row.piece_no);
    }
    for list in pieces_by_product.values_mut() {
        list.sort_unstable();
    }

    let products = products
        .into_iter()
        .filter(|row| row.active)
        .map(|row| {
            let mut piece_list = pieces_by_product.remove(&row.id).unwrap_or_default();
            piece_list.sort_unstable();
            let units_in_drop = piece_list.len() as i64;
            CatalogueProduct {
                slug: row.slug,
                brand: row.brand,
                model: row.model,
                price_cents: row.price_cents,
                family: row.family,
                active: row.active,
                art_variant: ArtVariant::parse(&row.art_variant),
                art_hue: row.art_hue,
                image_day_url: row.image_day_url,
                image_lit_url: row.image_lit_url,
                designer: row.designer,
                design_year: row.design_year,
                dimensions: row.dimensions,
                light_source: row.light_source,
                content: content_by_product.remove(&row.id).unwrap_or_default(),
                reference_price: reference_by_product.remove(&row.id),
                pieces: piece_list,
                units_in_drop,
            }
        })
        .collect();

    CatalogueResponse {
        total_pieces,
        products,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn product_row(id: i64, slug: &str, active: bool) -> ProductRow {
        ProductRow {
            id,
            slug: slug.to_string(),
            brand: "Louis Poulsen".to_string(),
            model: "Panthella 160 V3 Portable".to_string(),
            price_cents: 161_500,
            family: "panthella-160-portable".to_string(),
            active,
            art_variant: "dome".to_string(),
            art_hue: 35,
            image_day_url: "https://cdn.example.com/day.png".to_string(),
            image_lit_url: "https://cdn.example.com/lit.webp".to_string(),
            designer: "Verner Panton".to_string(),
            design_year: "1971".to_string(),
            dimensions: "Ø 160 × H 238 mm".to_string(),
            light_source: "2700K Integrated LED · 2.5W".to_string(),
        }
    }

    fn content_row(product_id: i64, locale: &str, field: &str, value: &str) -> ContentRow {
        ContentRow {
            product_id,
            locale: locale.to_string(),
            field: field.to_string(),
            value: value.to_string(),
        }
    }

    fn reference_row(product_id: i64) -> ReferencePriceRow {
        ReferencePriceRow {
            product_id,
            min_price_cents: 150_000,
            max_price_cents: 168_000,
            source_name: "MyConcept Hong Kong".to_string(),
            source_url: "https://example.com/reference".to_string(),
            checked_on: NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
        }
    }

    fn keys_of(value: &Value) -> BTreeSet<&str> {
        value
            .as_object()
            .expect("expected a JSON object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn catalogue_product_json_has_exact_keys_and_no_stock() {
        let response = assemble(
            vec![product_row(1, "panthella-160-v3-beige", true)],
            vec![content_row(1, "en", "colourway", "Beige")],
            vec![reference_row(1)],
            vec![PieceRow {
                piece_no: 1,
                product_id: 1,
            }],
            1,
        );
        let value = serde_json::to_value(&response).unwrap();
        let product = &value["products"][0];
        let expected: BTreeSet<&str> = [
            "slug",
            "brand",
            "model",
            "price_cents",
            "family",
            "active",
            "art_variant",
            "art_hue",
            "image_day_url",
            "image_lit_url",
            "designer",
            "design_year",
            "dimensions",
            "light_source",
            "content",
            "reference_price",
            "pieces",
            "units_in_drop",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys_of(product), expected);

        // The R1 flagship fields travel at their exact values (0005 writes
        // them; the response must not reformat or invent any of them).
        assert_eq!(product["image_day_url"], "https://cdn.example.com/day.png");
        assert_eq!(product["image_lit_url"], "https://cdn.example.com/lit.webp");
        assert_eq!(product["designer"], "Verner Panton");
        assert_eq!(product["design_year"], "1971");
        assert_eq!(product["dimensions"], "Ø 160 × H 238 mm");
        assert_eq!(product["light_source"], "2700K Integrated LED · 2.5W");

        // Stock lives only on /api/products (fail-closed availability channel):
        // no stock key may appear anywhere in the catalogue payload.
        let text = serde_json::to_string(&response).unwrap();
        assert!(!text.contains("stock"));
        assert!(!text.contains("reserved"));
    }

    #[test]
    fn content_fields_appear_only_when_rows_exist() {
        let response = assemble(
            vec![product_row(1, "ph-2-1-table-chrome", true)],
            vec![
                content_row(1, "zh-HK", "model_alt", "PH 2/1 檯燈"),
                content_row(1, "zh-HK", "colourway", "鍍鉻"),
                content_row(1, "en", "colourway", "Chrome"),
            ],
            vec![],
            vec![],
            0,
        );
        let value = serde_json::to_value(&response).unwrap();
        let zh = &value["products"][0]["content"]["zh-HK"];
        assert_eq!(
            keys_of(zh),
            ["model_alt", "colourway"].into_iter().collect()
        );
        assert_eq!(zh["model_alt"], "PH 2/1 檯燈");
        // The en locale has only a colourway row: no other keys are invented.
        assert_eq!(
            keys_of(&value["products"][0]["content"]["en"]),
            ["colourway"].into_iter().collect()
        );
    }

    #[test]
    fn reference_price_null_or_object_and_checked_on_format() {
        let with_ref = assemble(
            vec![product_row(1, "with-ref", true)],
            vec![],
            vec![reference_row(1)],
            vec![],
            0,
        );
        let value = serde_json::to_value(&with_ref).unwrap();
        let reference = &value["products"][0]["reference_price"];
        assert_eq!(
            keys_of(reference),
            [
                "min_price_cents",
                "max_price_cents",
                "source_name",
                "source_url",
                "checked_on"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(reference["min_price_cents"], 150_000);
        assert_eq!(reference["max_price_cents"], 168_000);
        // checked_on serializes as a YYYY-MM-DD string, not an object or array.
        assert_eq!(
            reference["checked_on"],
            Value::String("2026-10-12".to_string())
        );

        let without_ref = assemble(
            vec![product_row(2, "no-ref", true)],
            vec![],
            vec![],
            vec![],
            0,
        );
        let value = serde_json::to_value(&without_ref).unwrap();
        assert!(value["products"][0]["reference_price"].is_null());
    }

    #[test]
    fn assemble_sorts_pieces_counts_units_and_keeps_count_total() {
        let response = assemble(
            vec![product_row(1, "panthella-160-v3-beige", true)],
            vec![],
            vec![],
            vec![
                PieceRow {
                    piece_no: 3,
                    product_id: 1,
                },
                PieceRow {
                    piece_no: 1,
                    product_id: 1,
                },
                PieceRow {
                    piece_no: 2,
                    product_id: 1,
                },
            ],
            16,
        );
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["products"][0]["pieces"], serde_json::json!([1, 2, 3]));
        assert_eq!(value["products"][0]["units_in_drop"], 3);
        // total_pieces is the COUNT(*) result, not a constant and not derived
        // from the rows in hand (a product page never "knows" the drop size).
        assert_eq!(value["total_pieces"], 16);
    }

    #[test]
    fn assemble_excludes_inactive_products_and_their_data() {
        let response = assemble(
            vec![
                product_row(1, "active-sku", true),
                product_row(2, "inactive-sku", false),
            ],
            vec![
                content_row(1, "en", "colourway", "Beige"),
                content_row(2, "en", "colourway", "Secret Colourway"),
            ],
            vec![reference_row(2)],
            vec![
                PieceRow {
                    piece_no: 1,
                    product_id: 1,
                },
                PieceRow {
                    piece_no: 2,
                    product_id: 2,
                },
            ],
            2,
        );
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["products"].as_array().unwrap().len(), 1);
        assert_eq!(value["products"][0]["slug"], "active-sku");
        assert_eq!(value["products"][0]["units_in_drop"], 1);
        // Nothing from the inactive SKU leaks into the payload.
        let text = serde_json::to_string(&response).unwrap();
        assert!(!text.contains("inactive-sku"));
        assert!(!text.contains("Secret Colourway"));
    }

    #[test]
    fn art_variant_serializes_as_lowercase_and_parses() {
        // All four CHECK-constrained values round-trip: the hyphenated
        // `shades-tall` spelling is pinned explicitly (a plain `lowercase`
        // rename would emit `shadestall` and break the web tier).
        for (raw, variant) in [
            ("dome", ArtVariant::Dome),
            ("shades", ArtVariant::Shades),
            ("shades-tall", ArtVariant::ShadesTall),
            ("piano", ArtVariant::Piano),
            ("none", ArtVariant::None),
        ] {
            assert_eq!(ArtVariant::parse(raw), variant, "parse({raw})");
            assert_eq!(
                serde_json::to_value(variant).unwrap(),
                Value::String(raw.to_string()),
                "serialize({raw})"
            );
        }
        // An unknown value falls back to the schema default (Dome): one bad
        // row must not take down the whole catalogue read.
        assert_eq!(ArtVariant::parse("orb"), ArtVariant::Dome);
        assert_eq!(ArtVariant::parse("shadestall"), ArtVariant::Dome);
        assert_eq!(ArtVariant::parse(""), ArtVariant::Dome);
    }
}
