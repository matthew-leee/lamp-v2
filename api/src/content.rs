//! `GET /api/site-content` and `GET /api/settings` — site copy and the PUBLIC
//! subset of runtime settings.
//!
//! Settings rule (CONFIG-01): secrets (`DATABASE_URL`, gateway/session keys)
//! are env-only and never enter the `settings` table, but this endpoint is
//! still fail-safe: only keys on the public whitelist are readable, everything
//! else is skipped and warned about. A secret must never be readable through
//! this endpoint even if one is mis-seeded.

use std::collections::BTreeMap;

use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::FromRow;
use tracing::warn;

use crate::state::{self, ApiError, AppState};

/// `GET /api/site-content` response: `{ "content": { key: { locale: value } } }`.
#[derive(Debug, Serialize)]
pub struct SiteContentResponse {
    pub content: BTreeMap<String, BTreeMap<String, String>>,
}

/// `GET /api/settings` response: a FLAT `{ "<key>": "<value>" }` object of the
/// whitelisted settings rows only. Transparent so the JSON shape is exactly
/// the flat map the contract specifies.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct SettingsResponse(pub BTreeMap<String, String>);

/// The only settings keys readable through the public API (CONFIG-01).
/// Anything not on this list is skipped and warned about.
pub const PUBLIC_SETTING_KEYS: &[&str] = &[
    "brand.name",
    "brand.name_native",
    "site.url",
    "contact.whatsapp",
    "analytics.ga_measurement_id",
    "analytics.gsc_verification",
    "currency",
    "payment.fps_id",
    "payment.fps_account_name",
    "payment.fps_qr_url",
];

/// Result of [`filter_public_settings`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilteredSettings {
    /// Whitelisted rows, ready for the flat response.
    pub visible: BTreeMap<String, String>,
    /// Keys that were skipped (in input order) — the caller logs these; the
    /// filter itself stays pure and unit-testable.
    pub skipped: Vec<String>,
}

/// Pure whitelist filter for `settings` rows. Non-whitelisted keys never reach
/// the response; they are returned in `skipped` so the request handler can log
/// them with `warn!`.
pub fn filter_public_settings(
    rows: impl IntoIterator<Item = (String, String)>,
) -> FilteredSettings {
    let mut out = FilteredSettings::default();
    for (key, value) in rows {
        if PUBLIC_SETTING_KEYS.contains(&key.as_str()) {
            out.visible.insert(key, value);
        } else {
            out.skipped.push(key);
        }
    }
    out
}

// Row shapes for the runtime queries (runtime `query_as`, never the `query!`
// macro: the build must not need DATABASE_URL or a .sqlx cache).

#[derive(Debug, FromRow)]
struct SiteContentRow {
    key: String,
    locale: String,
    value: String,
}

#[derive(Debug, FromRow)]
struct SettingRow {
    key: String,
    value: String,
}

/// `GET /api/site-content` — all site copy rows, nested key -> locale -> value.
pub async fn get_site_content(
    State(app): State<AppState>,
) -> Result<Json<SiteContentResponse>, ApiError> {
    let pool = state::require_pool(&app)?;
    let rows = sqlx::query_as::<_, SiteContentRow>(
        "SELECT key, locale, value FROM site_content ORDER BY key, locale",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| state::database_unavailable("failed to load site content", &e))?;

    let mut content: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for row in rows {
        content
            .entry(row.key)
            .or_default()
            .insert(row.locale, row.value);
    }
    Ok(Json(SiteContentResponse { content }))
}

/// `GET /api/settings` — whitelisted runtime settings as a flat `{ key: value }`.
/// Non-whitelisted rows are skipped and warned about (never returned).
pub async fn get_settings(State(app): State<AppState>) -> Result<Json<SettingsResponse>, ApiError> {
    let pool = state::require_pool(&app)?;
    let rows = sqlx::query_as::<_, SettingRow>("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to load settings", &e))?;

    let filtered = filter_public_settings(rows.into_iter().map(|r| (r.key, r.value)));
    for key in &filtered.skipped {
        // Usually a secret was mis-seeded into `settings` (0002 forbids it):
        // surface the key name in ops logs, never the value.
        warn!(
            key = %key,
            "settings key not in the public whitelist; skipped in GET /api/settings"
        );
    }
    Ok(Json(SettingsResponse(filtered.visible)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn row(key: &str, value: &str) -> (String, String) {
        (key.to_string(), value.to_string())
    }

    #[test]
    fn whitelist_matches_the_public_contract() {
        // Guard against accidental additions: exposing one extra key here would
        // make an admin-set secret readable by anyone.
        assert_eq!(
            PUBLIC_SETTING_KEYS.iter().copied().collect::<BTreeSet<_>>(),
            [
                "brand.name",
                "brand.name_native",
                "site.url",
                "contact.whatsapp",
                "analytics.ga_measurement_id",
                "analytics.gsc_verification",
                "currency",
                "payment.fps_id",
                "payment.fps_account_name",
                "payment.fps_qr_url",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn filter_passes_whitelisted_keys() {
        let filtered = filter_public_settings([
            row("brand.name", "ATELIER"),
            row("site.url", "https://shop.example.com"),
            row("currency", "HKD"),
            row("payment.fps_id", "10849201"),
        ]);
        assert_eq!(
            filtered
                .visible
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["brand.name", "currency", "payment.fps_id", "site.url"]
        );
        assert_eq!(filtered.visible["brand.name"], "ATELIER");
        assert_eq!(filtered.visible["payment.fps_id"], "10849201");
        assert!(filtered.skipped.is_empty());
    }

    #[test]
    fn filter_skips_secret_looking_keys() {
        let filtered = filter_public_settings([
            row("database.url", "postgres://user:pass@db/none"),
            row("stripe.secret_key", "sk_live_xxx"),
            row("session.secret", "s3cr3t"),
            row("smtp.password", "hunter2"),
            row("analytics.internal_api_key", "internal-only"),
        ]);
        assert!(filtered.visible.is_empty());
        assert_eq!(
            filtered.skipped,
            vec![
                "database.url",
                "stripe.secret_key",
                "session.secret",
                "smtp.password",
                "analytics.internal_api_key"
            ]
        );
    }

    #[test]
    fn settings_response_is_flat_key_value() {
        let filtered =
            filter_public_settings([row("brand.name", "ATELIER"), row("currency", "HKD")]);
        let value = serde_json::to_value(SettingsResponse(filtered.visible)).unwrap();
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["brand.name", "currency"].into_iter().collect()
        );
        assert_eq!(value["brand.name"], "ATELIER");
        assert_eq!(value["currency"], "HKD");
    }

    #[test]
    fn site_content_response_nests_key_locale_value() {
        let mut content: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        content
            .entry("tagline".to_string())
            .or_default()
            .insert("en".to_string(), "Curated designer homeware".to_string());
        content
            .entry("tagline".to_string())
            .or_default()
            .insert("zh-HK".to_string(), "精選設計家居".to_string());
        content
            .entry("policy.warranty".to_string())
            .or_default()
            .insert("en".to_string(), "Pending".to_string());

        let value = serde_json::to_value(SiteContentResponse { content }).unwrap();
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["content"]
        );
        let inner = value["content"].as_object().unwrap();
        assert_eq!(
            inner.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            ["tagline", "policy.warranty"].into_iter().collect()
        );
        assert_eq!(
            value["content"]["tagline"]["en"],
            "Curated designer homeware"
        );
        assert_eq!(value["content"]["tagline"]["zh-HK"], "精選設計家居");
        assert_eq!(value["content"]["policy.warranty"]["en"], "Pending");
    }
}
