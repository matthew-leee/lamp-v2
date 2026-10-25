//! lamp API — Axum server.
//!
//! Config from the environment (ENV-01; see `lamp_api::config`):
//! - `DATABASE_URL` — optional at boot. Without it the server still serves
//!   `/health`; the `/api/*` data endpoints return 503 with a clear JSON error.
//! - `PORT` — listen port, default 8080.
//! - `CORS_ORIGIN` — allowed browser origin, default `http://localhost:3000`.
//! - `MAX_POOL_CONNECTIONS` — Postgres pool size, default 5 (clamped to 1..=64).
//! - `SESSION_SECRET` — HMAC key for admin session cookies (>= 32 bytes);
//!   missing/weak disables admin login (503 on `/api/admin/login`).
//! - `ADMIN_USERNAME` / `ADMIN_PASSWORD` — when both are set, the admin user
//!   is upserted into `admin_users` at boot (argon2id hashed).
//! - `SESSION_COOKIE_SECURE` — `Secure` flag on the admin session cookie,
//!   default true; `false` only for plain-HTTP local dev.
//! - `LOCALES` — comma-separated locale set for admin content-write
//!   validation (default `en,zh-HK`); the web tier keeps its own
//!   `NEXT_PUBLIC_LOCALES`.
//! - `REVALIDATE_URL` / `REVALIDATE_SECRET` — commitment-5b on-demand
//!   revalidation webhook: after every committed admin mutation the API
//!   POSTs `{"secret":..., "tags":[...]}` to the web tier. The URL must be an
//!   absolute `http://` URL (the webhook client has no TLS); unset = webhook
//!   off (the 60s ISR window is the fallback).
//! - `CARD_RESERVATION_TTL_MINS` — how long a card reservation is held before
//!   it lapses (default 30, clamped to 1..=1440 minutes).
//! - `FPS_RESERVATION_TTL_HOURS` — how long an FPS reservation is held before
//!   it lapses (default 24, clamped to 1..=168 hours).
//! - `STRIPE_WEBHOOK_SECRET` — HMAC key for `Stripe-Signature` verification
//!   on `POST /api/webhooks/stripe` (>= 16 bytes); missing/weak disables the
//!   webhook (503), never half-authenticates it (PAY-01).
//! - `CURRENCY` / `NEXT_PUBLIC_CURRENCY` — fallback currency for new orders
//!   (3-letter ISO 4217 code, default `HKD`); the `settings` table's
//!   `currency` override wins when present.

use std::net::SocketAddr;

use anyhow::Context;
use axum::{
    extract::State,
    http::header,
    middleware,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use lamp_api::{
    admin, admin_crud, catalogue, config, content, csv_sync, orders,
    state::{self, AppState},
};
use serde::Serialize;
use sqlx::{postgres::PgPoolOptions, FromRow};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// A product row for `/api/products` — the fail-closed availability channel.
/// `price_cents` is integer HKD cents — money is never a float.
///
/// No `colourway`: migration 0002 moved colour copy into locale-keyed
/// `product_content` rows served by `/api/catalogue`. This response carries
/// stock (`stock_total`/`stock_reserved`); the catalogue deliberately does not.
#[derive(Debug, Serialize, FromRow)]
struct Product {
    id: i64,
    slug: String,
    brand: String,
    model: String,
    price_cents: i64,
    stock_total: i32,
    stock_reserved: i32,
    stock_quarantined: i32,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct HealthBody {
    status: &'static str,
}

async fn health() -> Json<HealthBody> {
    Json(HealthBody { status: "ok" })
}

async fn list_products(
    State(app): State<AppState>,
) -> Result<
    (
        [(axum::http::HeaderName, &'static str); 1],
        Json<Vec<Product>>,
    ),
    state::ApiError,
> {
    let pool = state::require_pool(&app)?;
    orders::sweep_stale_reservations(pool)
        .await
        .map_err(|e| state::database_unavailable("failed to expire stale reservations", &e))?;

    let products = sqlx::query_as::<_, Product>(
        "SELECT id, slug, brand, model, price_cents, \
             stock_total, stock_reserved, stock_quarantined, created_at \
         FROM products ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| state::database_unavailable("failed to load products", &e))?;

    // Stock is live data: no layer (browser, CDN, proxy) may cache this
    // response (commitment 5b; the web tier already fetches no-store, the
    // header keeps any direct consumer honest).
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(products)))
}

fn app(state: AppState) -> Router {
    // Permissive methods/headers, but the origin is locked to CORS_ORIGIN
    // (default: the local web app).
    let cors = CorsLayer::new()
        .allow_origin(config::cors_origin())
        .allow_methods(Any)
        .allow_headers(Any);

    // Admin auth (ADMIN-01): login is public; every other /api/admin route
    // runs the session-cookie + CSRF middleware. The D2a CRUD surface lives
    // HERE (this router), so every route below carries require_admin + CSRF
    // exactly like whoami.
    let admin_protected: Router<AppState> = Router::new()
        .route("/whoami", get(admin::whoami))
        .route("/logout", post(admin::logout))
        .route(
            "/products",
            get(admin_crud::list_products).post(admin_crud::create_product),
        )
        .route(
            "/products/{id}",
            get(admin_crud::get_product_full).put(admin_crud::update_product),
        )
        .route("/products/{id}/full", put(admin_crud::update_product_full))
        .route("/products/{id}/clone", post(admin_crud::clone_product))
        .route(
            "/products/{id}/inspect-quarantine",
            post(admin_crud::inspect_quarantine),
        )
        .route("/product-content", put(admin_crud::upsert_product_content))
        .route(
            "/reference-prices/{product_id}",
            put(admin_crud::upsert_reference_price).delete(admin_crud::delete_reference_price),
        )
        .route("/pieces", post(admin_crud::create_piece))
        .route(
            "/pieces/{piece_no}",
            patch(admin_crud::reassign_piece).delete(admin_crud::delete_piece),
        )
        .route("/site-content", put(admin_crud::upsert_site_content))
        .route(
            "/site-content/{key}/{locale}",
            delete(admin_crud::delete_site_content),
        )
        .route(
            "/settings/{key}",
            put(admin_crud::upsert_setting).delete(admin_crud::delete_setting),
        )
        // Money-path admin surface (R5a): order inspection + FPS confirmation,
        // release and quarantine-return — all behind the same require_admin +
        // CSRF middleware as the rest of the console.
        .route("/orders", get(orders::list_orders))
        .route("/orders/{id}/confirm-fps", post(orders::confirm_fps_order))
        .route("/orders/{id}/fulfil", post(orders::fulfil_order))
        .route("/orders/{id}/release", post(orders::release_order))
        .route(
            "/orders/{id}/quarantine-return",
            post(orders::quarantine_return_order),
        )
        .route("/audit-log", get(admin_crud::list_audit_log))
        // 1-Click Excel/CSV Bulk Sync (DATA-01): UTF-8 BOM export, dry-run
        // preview and atomic commit for the catalogue and the bilingual site
        // copy. All six routes carry the same require_admin + CSRF middleware
        // as the rest of the console.
        .route("/csv/products", get(csv_sync::export_products_csv))
        .route(
            "/csv/products/preview",
            post(csv_sync::preview_products_csv),
        )
        .route("/csv/products/commit", post(csv_sync::commit_products_csv))
        .route("/csv/site-content", get(csv_sync::export_site_content_csv))
        .route(
            "/csv/site-content/preview",
            post(csv_sync::preview_site_content_csv),
        )
        .route(
            "/csv/site-content/commit",
            post(csv_sync::commit_site_content_csv),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin::require_admin,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/api/products", get(list_products))
        .route("/api/catalogue", get(catalogue::get_catalogue))
        .route("/api/site-content", get(content::get_site_content))
        .route("/api/settings", get(content::get_settings))
        // Money path (R5a): the reservation/public-status endpoints and the
        // gateway webhook are PUBLIC (no session): the buyer has none, and the
        // webhook authenticates by `Stripe-Signature`, not by cookie.
        .route("/api/orders/reserve", post(orders::create_reservation))
        .route("/api/orders/{order_code}", get(orders::get_order_by_code))
        .route(
            "/api/orders/{order_code}/submit-fps",
            post(orders::submit_customer_fps),
        )
        .route(
            "/api/orders/{order_code}/cancel",
            post(orders::cancel_customer_order),
        )
        // Paid-email delivery ledger (findings G02): the web tier reports the
        // outcome of every paid-confirmation email here (no session — the
        // caller is our own server route, like submit-fps/cancel), so sends
        // are deduplicated against `paid_email_sent_at` and failures stay
        // visible instead of vanishing.
        .route(
            "/api/orders/{order_code}/email-delivery",
            post(orders::record_email_delivery),
        )
        .route("/api/webhooks/stripe", post(orders::stripe_webhook))
        .route("/api/admin/login", post(admin::login))
        .nest("/api/admin", admin_protected)
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);

    // Lazy pool: an unreachable database never prevents boot (see AppState).
    let pool = match std::env::var("DATABASE_URL") {
        Ok(url) => {
            let pool = PgPoolOptions::new()
                .max_connections(config::max_pool_connections())
                .connect_lazy(&url)
                .context("DATABASE_URL is not a valid Postgres URL")?;
            info!("DATABASE_URL set; /api/* will query Postgres");
            Some(pool)
        }
        Err(_) => {
            warn!("DATABASE_URL not set; /health works, /api/* returns 503");
            None
        }
    };

    // Session key + login work-admission gate (ADMIN-01): misconfiguration
    // warns and continues — the storefront never fails to boot over admin
    // console config.
    let session_key = config::session_secret().map(admin::SessionKey::new);
    // Warn-once validation of the request-path knobs at boot (each fn falls
    // back safely and only ever warns this first time): LOCALES and the
    // revalidation-webhook pair are read per admin write, so their
    // misconfiguration must surface here, not on every mutation.
    let _ = config::locales();
    let _ = config::revalidate_url();
    let _ = config::revalidate_secret();
    // Money-path knobs are also read per request (every reservation resolves
    // its TTL/currency, every webhook its secret): surface their
    // misconfiguration here once, not on every order.
    let _ = config::card_reservation_ttl_mins();
    let _ = config::fps_reservation_ttl_hours();
    let _ = config::stripe_webhook_secret();
    let _ = config::default_currency();
    let login_gate = std::sync::Arc::new(tokio::sync::Semaphore::new(admin::LOGIN_MAX_CONCURRENT));
    // Login refuses (503 "not ready") until bootstrap resolves; bootstrap_admin
    // sets it ONLY when no rotation was requested or the requested rotation
    // was applied — a requested-but-FAILED rotation keeps login disabled
    // until a good boot (fail closed against the stale credential).
    let admin_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    // The admin bootstrap (SESSION_SECRET warning + ADMIN_USERNAME/
    // ADMIN_PASSWORD upsert) is spawned OFF the serving path: a slow or
    // stuck DB (e.g. an admin_users row-lock wait) must never delay the
    // storefront from accepting connections. Until bootstrap resolves, login
    // answers 503 "not ready" (see `admin_ready`); nothing else is affected.
    let bootstrap_pool = pool.clone();
    let bootstrap_key = session_key.clone();
    let bootstrap_ready = admin_ready.clone();
    tokio::spawn(async move {
        admin::bootstrap_admin(
            bootstrap_pool.as_ref(),
            bootstrap_key.as_ref(),
            bootstrap_ready,
        )
        .await;
    });

    let app = app(AppState {
        pool,
        session_key,
        login_gate,
        admin_ready,
    });
    info!("listening on {addr}");
    axum::serve(listener, app).await.context("server error")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    /// Builds test [`AppState`]s: real-shape pool/session-key options plus a
    /// fresh, isolated login gate (never a shared one — tests run in
    /// parallel, so a saturated-gate test must not race the others).
    /// Builds test [`AppState`]s: real-shape pool/session-key options plus a
    /// fresh, isolated login gate (never a shared one — tests run in
    /// parallel, so a saturated-gate test must not race the others) and
    /// admin auth marked ready (the boot-race flag is exercised explicitly
    /// by the test that needs it false).
    fn test_state(pool: Option<sqlx::PgPool>, session_key: Option<admin::SessionKey>) -> AppState {
        AppState {
            pool,
            session_key,
            login_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(
                admin::LOGIN_MAX_CONCURRENT,
            )),
            admin_ready: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let response = app(test_state(None, None))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn products_503_without_database() {
        let response = app(test_state(None, None))
            .oneshot(
                Request::builder()
                    .uri("/api/products")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("database not configured"));
    }

    /// Shared contract for the D1.2 read endpoints: without `DATABASE_URL`
    /// each returns the exact 503 body the web tier already handles.
    async fn assert_database_not_configured(uri: &str) {
        let response = app(test_state(None, None))
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            text,
            "{\"error\":\"database not configured: set DATABASE_URL\"}"
        );
    }

    #[tokio::test]
    async fn catalogue_503_without_database() {
        assert_database_not_configured("/api/catalogue").await;
    }

    #[tokio::test]
    async fn site_content_503_without_database() {
        assert_database_not_configured("/api/site-content").await;
    }

    #[tokio::test]
    async fn settings_503_without_database() {
        assert_database_not_configured("/api/settings").await;
    }

    #[tokio::test]
    async fn products_error_does_not_leak_driver_details() {
        // Lazy pool aimed at a dead address: the query fails at use, and the client
        // must see only the generic message, never the raw sqlx error (which embeds
        // the connection target and can embed credentials).
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/none")
            .unwrap();
        let response = app(test_state(Some(pool), None))
            .oneshot(
                Request::builder()
                    .uri("/api/products")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(text, "{\"error\":\"database unavailable\"}");
        assert!(!text.contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn data_endpoints_do_not_leak_driver_details() {
        // Same leak-proofing contract for the D1.2 endpoints: a failing query
        // yields the generic body, never the driver error. The short acquire
        // timeout keeps this test fast; the contract under test is the error
        // mapping, not the timing of a dead TCP connect.
        for uri in ["/api/catalogue", "/api/site-content", "/api/settings"] {
            let pool = PgPoolOptions::new()
                .acquire_timeout(std::time::Duration::from_secs(2))
                .connect_lazy("postgres://127.0.0.1:1/none")
                .unwrap();
            let response = app(test_state(Some(pool), None))
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert_eq!(text, "{\"error\":\"database unavailable\"}", "{uri}");
            assert!(!text.contains("127.0.0.1"), "{uri}");
        }
    }

    // -- admin auth (ADMIN-01) contract ----------------------------------

    fn test_session_key() -> admin::SessionKey {
        admin::SessionKey::new(b"contract-test-session-secret-0123456789ab".to_vec())
    }

    /// Mints a valid cookie the same way the login handler does (pure helper).
    fn test_session_cookie() -> String {
        let token = admin::session_cookie_value(
            "admin",
            test_session_key().as_bytes(),
            Utc::now().timestamp(),
        );
        format!("{}={token}", admin::SESSION_COOKIE_NAME)
    }

    fn login_request(body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/admin/login")
            .header("content-type", "application/json")
            .header("x-requested-with", "test-client")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn login_503_without_database() {
        // Session key configured but no DATABASE_URL: the standard generic
        // database-not-configured body, exactly like the other /api endpoints.
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(login_request(r#"{"username":"admin","password":"secret"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"database not configured: set DATABASE_URL\"}"
        );
    }

    #[tokio::test]
    async fn login_503_without_session_secret() {
        // SESSION_SECRET missing/weak: admin login is disabled, never minting
        // cookies under a guessable key.
        let app = app(test_state(None, None));
        let response = app
            .oneshot(login_request(r#"{"username":"admin","password":"secret"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"admin sessions not configured: set SESSION_SECRET (>= 32 bytes)\"}"
        );
    }

    #[tokio::test]
    async fn login_400_on_blank_or_missing_credentials() {
        // Blank values, a missing field and malformed JSON are all form
        // problems (400), never auth verdicts — and never reach the DB.
        let app = app(test_state(None, Some(test_session_key())));
        for body in [
            r#"{"username":"","password":""}"#,
            r#"{"username":"admin"}"#,
            r#"{"password":"secret"}"#,
            "not json",
        ] {
            let response = app.clone().oneshot(login_request(body)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                String::from_utf8(bytes.to_vec()).unwrap(),
                "{\"error\":\"username and password required\"}",
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn login_403_without_x_requested_with() {
        // Login mints a bearer credential: the same CSRF custom-header rule as
        // the mutations applies (a cross-site form cannot set it).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"secret"}"#.to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"missing X-Requested-With header\"}"
        );
    }

    #[tokio::test]
    async fn whoami_401_without_cookie() {
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"not authenticated\"}"
        );
    }

    #[tokio::test]
    async fn whoami_401_with_tampered_cookie() {
        // A cookie the signature does not cover is not a session.
        let cookie = test_session_cookie();
        let tampered = format!("{}x", &cookie[..cookie.len() - 1]);
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", tampered)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn whoami_403_without_x_requested_with() {
        // Valid cookie but an unsafe method without the CSRF header: rejected
        // before the handler runs.
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"missing X-Requested-With header\"}"
        );
    }

    #[tokio::test]
    async fn whoami_200_with_valid_cookie_and_header() {
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .header("x-requested-with", "XMLHttpRequest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["username"], "admin");
    }

    // -- middleware contracts the reviewer asked to pin (no DB needed) ----

    #[tokio::test]
    async fn whoami_get_is_exempt_from_the_csrf_header_rule() {
        // GET is a safe method: a valid cookie alone passes (200).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn whoami_unsafe_method_with_header_passes_middleware() {
        // POST is unrouted for whoami: passing middleware lands on the method
        // router's 405 — proving auth + CSRF accepted the request.
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .header("x-requested-with", "test-client")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn whoami_empty_csrf_header_is_rejected() {
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .header("x-requested-with", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn whoami_csrf_header_name_is_case_insensitive() {
        // Header names are case-insensitive per HTTP; mixed case passes.
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/whoami")
                    .header("cookie", test_session_cookie())
                    .header("X-Requested-With", "test-client")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn whoami_expired_cookie_is_rejected() {
        let now = Utc::now().timestamp();
        let token = admin::session_cookie_value(
            "admin",
            test_session_key().as_bytes(),
            now - admin::SESSION_TTL_SECS - 1,
        );
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", format!("{}={token}", admin::SESSION_COOKIE_NAME))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn whoami_rejects_every_cookie_without_a_session_key() {
        // SESSION_SECRET unset: no cookie, however well-formed, can verify.
        let now = Utc::now().timestamp();
        let token = admin::session_cookie_value("admin", test_session_key().as_bytes(), now);
        let app = app(test_state(None, None));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", format!("{}={token}", admin::SESSION_COOKIE_NAME))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn whoami_finds_the_session_across_multiple_cookie_headers() {
        let now = Utc::now().timestamp();
        let token = admin::session_cookie_value("admin", test_session_key().as_bytes(), now);
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/whoami")
                    .header("cookie", "theme=dark")
                    .header("cookie", format!("{}={token}", admin::SESSION_COOKIE_NAME))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn login_429_when_the_verify_gate_is_saturated() {
        // Zero permits = a saturated gate: rejected before the DB is touched,
        // with a body the UI can distinguish from a wrong password (401).
        let state = AppState {
            login_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(0)),
            ..test_state(None, Some(test_session_key()))
        };
        let app = app(state);
        let response = app
            .oneshot(login_request(r#"{"username":"admin","password":"secret"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"too many concurrent login attempts; retry shortly\"}"
        );
    }

    #[tokio::test]
    async fn login_503_until_bootstrap_marks_admin_auth_ready() {
        // Restart race (reviewer finding): with a stale credential row in the
        // DB, login must refuse until the boot upsert resolves — otherwise
        // the OLD password would mint sessions while the upsert is in flight.
        let state = AppState {
            admin_ready: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ..test_state(None, Some(test_session_key()))
        };
        let booting_app = app(state);
        let response = booting_app
            .oneshot(login_request(r#"{"username":"admin","password":"secret"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"admin auth not ready; retry later\"}"
        );

        // Once bootstrap marks ready (only a clean no-rotation or an APPLIED
        // rotation does), the same request proceeds to the next check: no
        // DATABASE_URL -> the standard 503.
        let state = test_state(None, Some(test_session_key()));
        state
            .admin_ready
            .store(true, std::sync::atomic::Ordering::Release);
        let ready_app = app(state);
        let response = ready_app
            .oneshot(login_request(r#"{"username":"admin","password":"secret"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"database not configured: set DATABASE_URL\"}"
        );
    }

    // -- D2a admin CRUD contracts (validation BEFORE the DB) -------------
    //
    // Every request below runs against a pool-less app (pool: None): a 400
    // proves validation rejected the request without touching the database,
    // a 503 "database not configured" proves validation PASSED and the
    // request reached the DB gate. No test needs a database.

    fn crud_request(method: &str, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", test_session_cookie())
            .header("x-requested-with", "test-client")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn crud_expect(req: Request<Body>, status: StatusCode, body: &str) {
        let app = app(test_state(None, Some(test_session_key())));
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), status, "{body}");
        let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), body);
    }

    const DB_GATE: &str = "{\"error\":\"database not configured: set DATABASE_URL\"}";

    #[tokio::test]
    async fn admin_settings_writes_reject_keys_outside_the_whitelist() {
        // D1 enforced PUBLIC_SETTING_KEYS only on reads; the D2 write side is
        // enforced here — and it rejects BEFORE any DB work.
        crud_expect(
            crud_request("PUT", "/api/admin/settings/api.url", r#"{"value":"x"}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"settings key not admin-editable\"}",
        )
        .await;
        crud_expect(
            crud_request("DELETE", "/api/admin/settings/database.url", ""),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"settings key not admin-editable\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_settings_writes_reject_secret_looking_keys() {
        // Defense in depth: secret-looking keys are outside the whitelist AND
        // rejected on their own (same generic 400 — never confirm a key).
        for key in [
            "session.secret",
            "smtp.password",
            "api.token",
            "stripe.credential",
            "vault.private",
            "SESSION.SECRET",
        ] {
            crud_expect(
                crud_request(
                    "PUT",
                    &format!("/api/admin/settings/{key}"),
                    r#"{"value":"x"}"#,
                ),
                StatusCode::BAD_REQUEST,
                "{\"error\":\"settings key not admin-editable\"}",
            )
            .await;
            crud_expect(
                crud_request("DELETE", &format!("/api/admin/settings/{key}"), ""),
                StatusCode::BAD_REQUEST,
                "{\"error\":\"settings key not admin-editable\"}",
            )
            .await;
        }
    }

    #[tokio::test]
    async fn admin_settings_put_rejects_empty_required_fields() {
        // brand.name / site.url are no-toggle required fields.
        crud_expect(
            crud_request("PUT", "/api/admin/settings/brand.name", r#"{"value":""}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"brand.name cannot be empty\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/settings/brand.name",
                r#"{"value":"   "}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"brand.name cannot be empty\"}",
        )
        .await;
        crud_expect(
            crud_request("PUT", "/api/admin/settings/site.url", r#"{"value":""}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"site.url cannot be empty\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_settings_put_rejects_bad_currency() {
        // currency has NO OFF state and must match ^[A-Z]{3}$.
        for value in ["usd", "HK", "HKDD", "HK D", ""] {
            crud_expect(
                crud_request(
                    "PUT",
                    "/api/admin/settings/currency",
                    &format!(r#"{{"value":"{value}"}}"#),
                ),
                StatusCode::BAD_REQUEST,
                "{\"error\":\"currency must be three uppercase letters (ISO 4217)\"}",
            )
            .await;
        }
    }

    #[tokio::test]
    async fn admin_settings_valid_writes_reach_the_db_gate() {
        // Valid writes pass validation and stop at the DB gate (pool: None).
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/settings/brand.name",
                r#"{"value":"ATELIER"}"#,
            ),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
        crud_expect(
            crud_request("PUT", "/api/admin/settings/currency", r#"{"value":"HKD"}"#),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
        // Empty value on an OPTIONAL key is the explicit OFF state (CONFIG-01
        // presence-wins) — legal.
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/settings/contact.whatsapp",
                r#"{"value":""}"#,
            ),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
        // Delete = revert to env default; whitelisted keys pass to the DB.
        crud_expect(
            crud_request("DELETE", "/api/admin/settings/currency", ""),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
    }

    #[tokio::test]
    async fn admin_crud_400_on_malformed_json_body() {
        crud_expect(
            crud_request("PUT", "/api/admin/settings/currency", "not json"),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"invalid JSON body\"}",
        )
        .await;
        crud_expect(
            crud_request("POST", "/api/admin/products", "not json"),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"invalid JSON body\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_products_post_rejects_invalid_input() {
        let cases: &[(&str, &str)] = &[
            (
                r#"{"slug":"Ph-5","brand":"B","model":"M","price_cents":1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#,
                "{\"error\":\"slug must be 1-64 lowercase letters, digits or hyphens\"}",
            ),
            (
                r#"{"slug":"ph-5","brand":"","model":"M","price_cents":1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#,
                "{\"error\":\"brand cannot be empty\"}",
            ),
            (
                r#"{"slug":"ph-5","brand":"B","model":" ","price_cents":1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#,
                "{\"error\":\"model cannot be empty\"}",
            ),
            (
                r#"{"slug":"ph-5","brand":"B","model":"M","price_cents":-1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#,
                "{\"error\":\"price_cents must be >= 0\"}",
            ),
            (
                r#"{"slug":"ph-5","brand":"B","model":"M","price_cents":1,"family":"","art_variant":"orb","art_hue":1,"active":true}"#,
                r#"{"error":"art_variant must be one of \"dome\", \"shades\", \"shades-tall\", \"piano\", \"none\""}"#,
            ),
            (
                r#"{"slug":"ph-5","brand":"B","model":"M","price_cents":1,"family":"","art_variant":"dome","art_hue":360,"active":true}"#,
                "{\"error\":\"art_hue must be between 0 and 359\"}",
            ),
        ];
        for (body, expected) in cases {
            crud_expect(
                crud_request("POST", "/api/admin/products", body),
                StatusCode::BAD_REQUEST,
                expected,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn admin_products_valid_mutations_reach_the_db_gate() {
        crud_expect(
            crud_request(
                "POST",
                "/api/admin/products",
                r#"{"slug":"ph-5","brand":"Louis Poulsen","model":"PH 5","price_cents":745000,"family":"ph-5","art_variant":"dome","art_hue":35,"active":true}"#,
            ),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
        crud_expect(
            crud_request("PUT", "/api/admin/products/1", r#"{"active":false}"#),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
    }

    #[tokio::test]
    async fn admin_products_put_rejects_invalid_patch() {
        crud_expect(
            crud_request("PUT", "/api/admin/products/1", r#"{"art_hue":360}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"art_hue must be between 0 and 359\"}",
        )
        .await;
        crud_expect(
            crud_request("PUT", "/api/admin/products/1", r#"{"brand":""}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"brand cannot be empty\"}",
        )
        .await;
        // An empty patch would fabricate an audit + webhook event for a
        // mutation that changed nothing.
        crud_expect(
            crud_request("PUT", "/api/admin/products/1", "{}"),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"no fields to update\"}",
        )
        .await;
        crud_expect(
            crud_request("PUT", "/api/admin/products/abc", r#"{"active":false}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"invalid product id\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_product_content_put_rejects_invalid_input() {
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/product-content",
                r#"{"product_id":0,"locale":"en","field":"colourway","value":"x"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"product_id must be >= 1\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/product-content",
                r#"{"product_id":1,"locale":"fr","field":"colourway","value":"x"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"locale not in the configured locale set\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/product-content",
                r#"{"product_id":1,"locale":"en","field":"price","value":"x"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"field must be one of model_alt, colourway, heritage, electrical\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/product-content",
                r#"{"product_id":1,"locale":"en","field":"colourway","value":" "}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"value cannot be empty\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_reference_prices_put_rejects_invalid_input() {
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/0",
                r#"{"min_price_cents":1,"max_price_cents":2,"source_name":"S","source_url":"http://x.example","checked_on":"2026-01-01"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"product_id must be >= 1\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":201,"max_price_cents":200,"source_name":"S","source_url":"http://x.example","checked_on":"2026-01-01"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"min_price_cents must be <= max_price_cents\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":-1,"max_price_cents":200,"source_name":"S","source_url":"http://x.example","checked_on":"2026-01-01"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"min_price_cents must be >= 0\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":1,"max_price_cents":200,"source_name":" ","source_url":"http://x.example","checked_on":"2026-01-01"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"source_name cannot be empty\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":1,"max_price_cents":200,"source_name":"S","source_url":"x.example","checked_on":"2026-01-01"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"source_url must be an absolute http:// or https:// URL\"}",
        )
        .await;
        // Reject 2026-02-30: not a real calendar date.
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":1,"max_price_cents":200,"source_name":"S","source_url":"https://x.example","checked_on":"2026-02-30"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"checked_on must be a real date in YYYY-MM-DD form\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_reference_prices_accepts_https_source_and_reaches_the_db_gate() {
        // https:// IS valid for reference sources (unlike REVALIDATE_URL).
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":600000,"max_price_cents":800000,"source_name":"Nordic Nest","source_url":"https://www.nordicnest.com/x","checked_on":"2026-02-28"}"#,
            ),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
    }

    #[tokio::test]
    async fn admin_pieces_reject_invalid_numbers() {
        crud_expect(
            crud_request(
                "POST",
                "/api/admin/pieces",
                r#"{"piece_no":0,"product_id":1}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"piece_no must be >= 1\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "POST",
                "/api/admin/pieces",
                r#"{"piece_no":1,"product_id":0}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"product_id must be >= 1\"}",
        )
        .await;
        crud_expect(
            crud_request("PATCH", "/api/admin/pieces/x", r#"{"product_id":1}"#),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"invalid piece_no\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_site_content_rejects_invalid_input() {
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/site-content",
                r#"{"key":" ","locale":"en","value":"x"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"key cannot be empty\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/site-content",
                r#"{"key":"tagline","locale":"fr","value":"x"}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"locale not in the configured locale set\"}",
        )
        .await;
        crud_expect(
            crud_request(
                "PUT",
                "/api/admin/site-content",
                r#"{"key":"tagline","locale":"en","value":""}"#,
            ),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"value cannot be empty\"}",
        )
        .await;
    }

    #[tokio::test]
    async fn admin_site_content_delete_validates_key_and_locale() {
        crud_expect(
            crud_request("DELETE", "/api/admin/site-content/tagline/fr", ""),
            StatusCode::BAD_REQUEST,
            "{\"error\":\"locale not in the configured locale set\"}",
        )
        .await;
        crud_expect(
            crud_request("DELETE", "/api/admin/site-content/tagline/en", ""),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
    }

    #[tokio::test]
    async fn admin_crud_routes_require_authentication() {
        // Every CRUD route lives on the admin_protected router: no session,
        // no handler — the same generic 401 as whoami.
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/products")
                    .header("content-type", "application/json")
                    .header("x-requested-with", "test-client")
                    .body(Body::from(
                        r#"{"slug":"ph-5","brand":"B","model":"M","price_cents":1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"not authenticated\"}"
        );
    }

    #[tokio::test]
    async fn admin_crud_mutations_require_the_csrf_header() {
        // Valid session cookie but no X-Requested-With: rejected before the
        // handler runs (cross-site forms cannot set the header).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/settings/brand.name")
                    .header("content-type", "application/json")
                    .header("cookie", test_session_cookie())
                    .body(Body::from(r#"{"value":"ATELIER"}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"missing X-Requested-With header\"}"
        );
    }

    #[tokio::test]
    async fn admin_list_endpoints_hit_the_db_gate() {
        // The admin reads carry no input to validate: straight to the DB gate.
        for uri in ["/api/admin/products", "/api/admin/audit-log"] {
            crud_expect(
                crud_request("GET", uri, ""),
                StatusCode::SERVICE_UNAVAILABLE,
                DB_GATE,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn admin_crud_validates_before_any_db_work() {
        // No pool configured: every one of these invalid inputs must be the
        // specific 400, never a 503 from the DB path (validation-first).
        let app = app(test_state(None, Some(test_session_key())));
        let cookie = test_session_cookie();
        let cases: [(&str, &str, &str, &str); 7] = [
            (
                "nonpositive product id",
                "PUT",
                "/api/admin/products/0",
                r#"{"active":false}"#,
            ),
            (
                "negative product id",
                "PUT",
                "/api/admin/products/-1",
                r#"{"active":false}"#,
            ),
            (
                "nul byte in brand on create",
                "POST",
                "/api/admin/products",
                r#"{"slug":"ph-5","brand":"B\u0000","model":"M","price_cents":1,"family":"","art_variant":"dome","art_hue":1,"active":true}"#,
            ),
            (
                "malformed source url",
                "PUT",
                "/api/admin/reference-prices/1",
                r#"{"min_price_cents":0,"max_price_cents":1,"source_name":"S","source_url":"https://:443/x","checked_on":"2026-01-01"}"#,
            ),
            (
                "nul byte in a settings value",
                "PUT",
                "/api/admin/settings/brand.name",
                "{\"value\":\"a\\u0000b\"}",
            ),
            ("empty product patch", "PUT", "/api/admin/products/1", "{}"),
            (
                "unknown settings key",
                "PUT",
                "/api/admin/settings/api.url",
                r#"{"value":"http://x"}"#,
            ),
        ];
        for (label, method, uri, body) in cases {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .header("cookie", &cookie)
                        .header("x-requested-with", "test-client")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{label}");
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(text.starts_with("{\"error\":\""), "{label}: {text}");
        }
    }

    #[tokio::test]
    async fn admin_crud_delete_rejects_nul_in_site_content_keys() {
        // Percent-encoded %00 decodes to a NUL in the path param: the shared
        // key validator must reject it BEFORE the DB (Postgres TEXT no-NUL,
        // reviewer finding: delete previously bypassed the check).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/admin/site-content/tag%00line/en")
                    .header("cookie", test_session_cookie())
                    .header("x-requested-with", "test-client")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"value contains a NUL byte\"}"
        );
    }

    #[tokio::test]
    async fn admin_crud_deletes_require_authentication_and_csrf() {
        // DELETE routes sit behind the same middleware as the writes (the
        // earlier tests covered POST/PUT only).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/admin/settings/brand.name")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/admin/pieces/1")
                    .header("cookie", test_session_cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn unmatched_admin_methods_still_require_authentication() {
        // PATCH is unrouted on /products/{id}: Router::layer wraps the method
        // fallback too, so the 405 must come AFTER auth (never a bypass).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/admin/products/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // With the session + CSRF header, the unrouted method answers 405.
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/admin/products/1")
                    .header("cookie", test_session_cookie())
                    .header("x-requested-with", "test-client")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn admin_trailing_slashes_fall_to_the_outer_404() {
        // /products/1/ does not match the nested route: the outer 404 answers
        // (no admin handler runs — documented behavior, not a middleware gap).
        let app = app(test_state(None, Some(test_session_key())));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/products/1/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- R5a money-path route wiring ------------------------------------
    //
    // Same pool-less style as the D2a section: the exact pre-DB answer pins
    // the route AND its ordering (validation before any database work).

    #[tokio::test]
    async fn orders_reserve_validates_before_the_db() {
        let app = app(test_state(None, None));
        // A rejected field is the specific 400 — the handler never reached the
        // (missing) pool.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/orders/reserve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"slug":"ph-2-1-table-chrome","quantity":17,"payment_method":"card","customer_name":"A","customer_email":"a@b.c"}"#
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"quantity must be between 1 and 16\"}"
        );

        // A well-formed body passes validation and hits the DB gate (503).
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/orders/reserve")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"slug":"ph-2-1-table-chrome","payment_method":"card","customer_name":"A","customer_email":"a@b.c"}"#
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), DB_GATE);
    }

    #[tokio::test]
    async fn order_public_read_hits_the_db_gate() {
        // GET /api/orders/{order_code} has no input to validate.
        crud_expect(
            crud_request("GET", "/api/orders/ORD-1-1", ""),
            StatusCode::SERVICE_UNAVAILABLE,
            DB_GATE,
        )
        .await;
    }

    #[tokio::test]
    async fn stripe_webhook_route_rejects_before_the_db() {
        // Route-wiring pin for POST /api/webhooks/stripe, without any ambient
        // dependency: whichever way STRIPE_WEBHOOK_SECRET is configured on
        // this machine, the answer is the documented pre-DB rejection (503
        // unconfigured / 401 unsigned) — never a 503 from the DB gate. The
        // signature semantics themselves are unit-pinned in `orders`.
        let app = app(test_state(None, None));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/webhooks/stripe")
                    .header("content-type", "application/json")
                    .body(Body::from("{}".to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (expected_status, expected_body) = if config::stripe_webhook_secret().is_some() {
            (
                StatusCode::UNAUTHORIZED,
                "{\"error\":\"invalid stripe signature\"}",
            )
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "{\"error\":\"stripe webhook not configured\"}",
            )
        };
        assert_eq!(response.status(), expected_status);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), expected_body);
    }

    #[tokio::test]
    async fn admin_order_routes_require_authentication() {
        // The money-path admin surface lives on the admin_protected router:
        // no session, no handler.
        for (method, uri) in [
            ("GET", "/api/admin/orders"),
            ("POST", "/api/admin/orders/1/confirm-fps"),
            ("POST", "/api/admin/orders/1/release"),
            ("POST", "/api/admin/orders/1/quarantine-return"),
        ] {
            let app = app(test_state(None, Some(test_session_key())));
            let response = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .header("x-requested-with", "test-client")
                        .body(Body::from("{}".to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri}"
            );
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                String::from_utf8(bytes.to_vec()).unwrap(),
                "{\"error\":\"not authenticated\"}",
                "{method} {uri}"
            );
        }
    }

    fn logout_request(cookie: Option<&str>, with_csrf: bool) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/admin/logout")
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        if with_csrf {
            builder = builder.header("x-requested-with", "test-client");
        }
        builder.body(Body::from("{}".to_string())).unwrap()
    }

    #[tokio::test]
    async fn logout_is_behind_the_admin_middleware() {
        // POST /api/admin/logout revokes a session SERVER-SIDE (findings G17):
        // it must never run for an unauthenticated caller — the exact
        // contract the admin_protected router mounts it behind.
        let cookie = test_session_cookie();
        // No cookie: 401 even though the session key could verify one.
        let response = app(test_state(None, Some(test_session_key())))
            .oneshot(logout_request(None, true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "no cookie");
        // Cookie present but no configured session key: nothing can verify.
        let response = app(test_state(None, None))
            .oneshot(logout_request(Some(&cookie), true))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "no session key"
        );
        // Cookie + session key but no CSRF header: the guard answers 403 —
        // logout is a state change like every other admin mutation.
        let response = app(test_state(None, Some(test_session_key())))
            .oneshot(logout_request(Some(&cookie), false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "no CSRF header");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"missing X-Requested-With header\"}"
        );
        // Valid cookie + CSRF header, but no DB pool: logout fails closed
        // (503) without clearing the cookie.
        let response = app(test_state(None, Some(test_session_key())))
            .oneshot(logout_request(Some(&cookie), true))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "logout must fail closed when DB pool is absent"
        );
        assert!(
            response.headers().get("set-cookie").is_none(),
            "failed logout must not clear the cookie"
        );
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), DB_GATE);
    }

    #[tokio::test]
    async fn email_delivery_validates_before_the_db_gate() {
        // POST /api/orders/{order_code}/email-delivery wiring pin (findings
        // G02): a malformed body is the standard 400 before any DB work, a
        // valid one stops at the DB gate (pool: None).
        let app = app(test_state(None, None));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/orders/ORD-1-1/email-delivery")
                    .header("content-type", "application/json")
                    .body(Body::from("not json".to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            "{\"error\":\"invalid JSON body\"}"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/orders/ORD-1-1/email-delivery")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"sent":true}"#.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), DB_GATE);
    }
}
