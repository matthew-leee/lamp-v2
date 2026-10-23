//! D2c step 2 — migration replay + seed idempotence against the scratch
//! Postgres harness (`common::ScratchDb`).
//!
//! What these tests pin down:
//!
//! * `scratch_target_validation`: the scratch-harness endpoint allowlist is
//!   pure (parse-only, no network), so it is pinned here with NO env gate and
//!   runs on EVERY `cargo test` — accepts the loopback spellings of :55433,
//!   refuses the verify DB (:55432), the native :5432, a remote host, a
//!   socket-path URL (incl. a socket override riding a sanctioned host:port)
//!   and an unparseable string. The ambient policy tests pin
//!   `assert_ambient_pg_env_clean` the same ungated way (mock env).
//! * `migrations_apply_in_order_on_fresh_db`: migrations 0001..0006 applied in
//!   order produce the exact verified opening-drop state (10 tables, seeded
//!   counts, the TRUST-02 missing reference price, single-language 404 titles
//!   per LANG-01).
//! * `seed_replay_twice_is_idempotent`: re-running 0003 (the seed) is a no-op
//!   on an already seeded database — the ON CONFLICT ... DO NOTHING contract.
//! * `seed_never_overwrites_admin_edits`: the same DO NOTHING contract means an
//!   existing row is never overwritten, so admin edits always win over the
//!   seed.
//!
//! The three DB-backed tests skip (loudly, via the shared helper) when
//! `STOREFRONT_TEST_DATABASE_URL` is unset, so plain `cargo test` stays green
//! without a scratch server; `scratch_target_validation` never skips. Tests
//! are plain `#[test]` fns driving the harness runtime through
//! `ScratchDb::block_on` — see `common` for why (`Drop` cannot block a
//! `#[tokio::test]` executor).

mod common;

use common::{assert_ambient_pg_env_clean, validate_scratch_target, ScratchDb};

/// The verified public schema after 0001..0008 (sorted, `information_schema`
/// order). A new table is a deliberate migration-contract change: update this
/// list consciously, never by making the test fuzzy.
const EXPECTED_TABLES: &[&str] = &[
    "admin_users",
    "audit_log",
    "order_items",
    "orders",
    "payment_events",
    "pieces",
    "product_content",
    "products",
    "reference_prices",
    "revoked_admin_sessions",
    "settings",
    "site_content",
];

/// The product deliberately left without a verified reference price
/// (TRUST-02): the storefront shows "verification pending" instead of an
/// invented comparison price.
const NO_REFERENCE_PRICE_SLUG: &str = "ph-1-1-piano-table-limited";

/// Marker proving an admin edit survived a seed replay in
/// `seed_never_overwrites_admin_edits`.
const EDIT_MARKER: &str = "ADMIN-EDIT-MARKER-9c2f";

/// Row counts of the opening drop (the 0003 + 0005 seed contract), covering
/// ALL 12 tables so a fresh apply AND every seed replay check the complete
/// state. The six always-empty tables are counted alongside the seeded ones
/// — a non-zero count there is a regression even though they are "not part of
/// the seed": `settings` is CONFIG-01 (overrides table, never seeded),
/// `audit_log` is written only by the D2 admin console, `admin_users` holds
/// no seeded secrets (the server upserts the first admin from env at boot),
/// `orders`/`payment_events` are runtime money-path rows (0006: created only
/// by real reservations and verified gateway events), and
/// `revoked_admin_sessions` is written only by admin logout (0008).
/// `site_content` is 36 rows after 0003 plus the 6 atmosphere rows
/// (dayLabel / nightLabel / caption x 2 locales) that 0005 adds = 42.
struct SeedCounts {
    products: i64,
    pieces: i64,
    product_content: i64,
    reference_prices: i64,
    site_content: i64,
    settings: i64,
    audit_log: i64,
    admin_users: i64,
    orders: i64,
    order_items: i64,
    payment_events: i64,
    revoked_admin_sessions: i64,
}

impl SeedCounts {
    /// Reads the counts from a scratch database.
    fn read(db: &ScratchDb) -> SeedCounts {
        SeedCounts {
            products: count(db, "products"),
            pieces: count(db, "pieces"),
            product_content: count(db, "product_content"),
            reference_prices: count(db, "reference_prices"),
            site_content: count(db, "site_content"),
            settings: count(db, "settings"),
            audit_log: count(db, "audit_log"),
            admin_users: count(db, "admin_users"),
            orders: count(db, "orders"),
            order_items: count(db, "order_items"),
            payment_events: count(db, "payment_events"),
            revoked_admin_sessions: count(db, "revoked_admin_sessions"),
        }
    }

    /// Fail-loud against the verified fresh-apply counts; `phase` names the
    /// point in the test so a failure says WHERE the drift happened.
    fn assert_opening_drop(&self, phase: &str) {
        assert_eq!(self.products, 8, "products after {phase}");
        assert_eq!(self.pieces, 16, "pieces after {phase}");
        assert_eq!(self.product_content, 56, "product_content after {phase}");
        assert_eq!(self.reference_prices, 7, "reference_prices after {phase}");
        assert_eq!(self.site_content, 42, "site_content after {phase}");
        assert_eq!(
            self.settings, 0,
            "settings must stay empty after {phase} (CONFIG-01: overrides \
             table, never seeded)"
        );
        assert_eq!(
            self.audit_log, 0,
            "audit_log must stay empty after {phase} (written only by the D2 \
             admin console)"
        );
        assert_eq!(
            self.admin_users, 0,
            "admin_users must stay empty after {phase} (no seeded secrets; \
             the server upserts the first admin from env at boot)"
        );
        assert_eq!(
            self.orders, 0,
            "orders must stay empty after {phase} (runtime money-path rows: \
             created only by real reservations, never by the seed)"
        );
        assert_eq!(
            self.order_items, 0,
            "order_items must stay empty after {phase} (runtime money-path rows: \
             created only by real reservations, never by the seed)"
        );
        assert_eq!(
            self.payment_events, 0,
            "payment_events must stay empty after {phase} (PAY-01 ledger: \
             written only by verified gateway events)"
        );
        assert_eq!(
            self.revoked_admin_sessions, 0,
            "revoked_admin_sessions must stay empty after {phase} (session \
             revocation ledger: written only by admin logout)"
        );
    }
}

/// `SELECT COUNT(*)` against a known table name (constants only — identifiers
/// cannot be bound as parameters).
fn count(db: &ScratchDb, table: &str) -> i64 {
    db.block_on(async {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("COUNT(*) failed for table {table}: {e}"))
    })
}

/// Base-table names in the `public` schema, sorted (information_schema order).
fn public_tables(db: &ScratchDb) -> Vec<String> {
    db.block_on(async {
        sqlx::query_scalar(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
        )
        .fetch_all(db.pool())
        .await
        .expect("information_schema.tables query failed")
    })
}

/// One `site_content` value by (key, locale), or `None` when the row is absent.
fn site_content_value(db: &ScratchDb, key: &str, locale: &str) -> Option<String> {
    db.block_on(async {
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM site_content WHERE key = $1 AND locale = $2",
        )
        .bind(key)
        .bind(locale)
        .fetch_optional(db.pool())
        .await
        .unwrap_or_else(|e| panic!("site_content lookup failed for ({key}, {locale}): {e}"))
    })
}

/// NETWORK-FREE endpoint-allowlist test: `validate_scratch_target` is pure
/// (it parses and inspects the URL, never connects), so this runs on every
/// `cargo test` with no env gate and pins both the accepted spellings and the
/// refused shapes without a scratch server.
#[test]
fn scratch_target_validation() {
    // The sanctioned endpoint: loopback TCP on the pinned scratch port, in
    // every spelling `get_host()` can yield (name, IPv4 literal, bracketed
    // IPv6 literal).
    for url in [
        "postgres://u:p@localhost:55433/x",
        "postgres://u:p@127.0.0.1:55433/x",
        "postgres://u:p@[::1]:55433/x",
    ] {
        validate_scratch_target(url)
            .unwrap_or_else(|e| panic!("scratch endpoint {url:?} must be accepted: {e}"));
    }

    // Refusals. Each entry names WHY the shape is refused.
    for (url, why) in [
        (
            "postgres://u:p@localhost:55432/x",
            "the shared verify DB port",
        ),
        (
            "postgres://u:p@localhost:5432/x",
            "the cluster's native port",
        ),
        (
            "postgres://u:p@db.example.com:55433/x",
            "a remote host on the scratch port",
        ),
        (
            "postgres://%2Fvar%2Frun%2Fpostgresql/x",
            "a Unix socket-path URL",
        ),
        (
            "postgres://u:p@localhost:55433/x?host=/var/run/postgresql",
            "a socket override riding a sanctioned TCP host:port — isolates \
             the get_socket() guard (without it this shape passes the \
             loopback host and port checks)",
        ),
        ("not a url", "an unparseable string"),
    ] {
        assert!(
            validate_scratch_target(url).is_err(),
            "validate_scratch_target must refuse {url:?} ({why})"
        );
    }

    // The verify-DB refusal must carry the full story in its message: WHICH
    // database is being protected and WHAT the sanctioned recipe is instead.
    let verify_err = validate_scratch_target("postgres://u:p@localhost:55432/x")
        .expect_err("55432 must be refused");
    assert!(
        verify_err.contains("verify"),
        "55432 refusal must name the shared verify DB: {verify_err}"
    );
    assert!(
        verify_err.contains("storefront_scratch") && verify_err.contains("55433"),
        "55432 refusal must name the scratch recipe: {verify_err}"
    );
}

/// NETWORK-FREE ambient-env policy regression: `assert_ambient_pg_env_clean`
/// is pure over its lookup, so the tests drive a MOCK environment — mutating
/// the process env from a test would race the parallel test threads, and
/// libtest captures each test's panic, so `should_panic` on a mock is the
/// honest way to pin the refusal. These two cover the POLICY; the live
/// wiring (one std::env closure in `ScratchDb::new`) is exercised by the
/// architect's live gate, not by these.
#[test]
#[should_panic(expected = "PGOPTIONS")]
fn ambient_pgoptions_set_refuses_the_run() {
    assert_ambient_pg_env_clean(|name| {
        (name == "PGOPTIONS").then(|| "-c search_path=pg_catalog".to_string())
    });
}

#[test]
fn ambient_pg_env_clean_passes() {
    assert_ambient_pg_env_clean(|_| None);
}

#[test]
fn migrations_apply_in_order_on_fresh_db() {
    let Some(url) = common::scratch_db_url() else {
        // Skip: the helper has already printed the loud banner.
        return;
    };
    let db = ScratchDb::new(&url, "migration_order");
    db.apply_migrations();

    let tables = public_tables(&db);
    let expected: Vec<String> = EXPECTED_TABLES.iter().map(|t| t.to_string()).collect();
    assert_eq!(tables, expected, "public base tables after 0001..0006");

    // SeedCounts::assert_opening_drop covers ALL 12 tables: the seeded five at
    // their verified counts plus settings/audit_log/admin_users/orders/
    // payment_events/revoked_admin_sessions at 0.
    SeedCounts::read(&db).assert_opening_drop("0001..0006");

    // DROP-01 numbering: pieces 1-16, spread over exactly the 8 seeded SKUs.
    let (min_piece, max_piece, distinct_products) = db.block_on(async {
        sqlx::query_as::<_, (i32, i32, i64)>(
            "SELECT MIN(piece_no), MAX(piece_no), COUNT(DISTINCT product_id) \
             FROM pieces",
        )
        .fetch_one(db.pool())
        .await
        .expect("pieces numbering query failed")
    });
    assert_eq!(min_piece, 1, "lowest piece number after 0001..0006");
    assert_eq!(max_piece, 16, "highest piece number after 0001..0006");
    assert_eq!(
        distinct_products, 8,
        "piece numbers must cover all 8 seeded products"
    );

    // TRUST-02: the PH 1/1 Piano Table intentionally gets no reference price,
    // so the UI can say "verification pending" instead of inventing one. The
    // check must not pass vacuously: first prove the product row EXISTS (a
    // joined count that silently matches nothing when the product is missing
    // would fake this green), THEN count reference_prices by ITS id.
    let product_id: i64 = db
        .block_on(async {
            sqlx::query_scalar::<_, i64>("SELECT id FROM products WHERE slug = $1")
                .bind(NO_REFERENCE_PRICE_SLUG)
                .fetch_optional(db.pool())
                .await
                .expect("products lookup failed")
        })
        .unwrap_or_else(|| {
            panic!(
                "seeded product {NO_REFERENCE_PRICE_SLUG} must exist after \
                 0001..0006 (TRUST-02)"
            )
        });
    let unpriced_references: i64 = db.block_on(async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reference_prices WHERE product_id = $1")
            .bind(product_id)
            .fetch_one(db.pool())
            .await
            .expect("reference_prices lookup failed")
    });
    assert_eq!(
        unpriced_references, 0,
        "{NO_REFERENCE_PRICE_SLUG} (id {product_id}) must have no reference \
         price row (TRUST-02)"
    );

    // LANG-01 (single language per page): the 404 title AND description are
    // one string per locale, no bilingual pair — pinned verbatim as seeded in
    // 0003_seed_opening_drop.sql.
    for (key, locale, want) in [
        ("notFound.title", "en", "{brand} | Page not found"),
        ("notFound.title", "zh-HK", "{brand} | 找不到頁面"),
        (
            "notFound.description",
            "en",
            "Page not found. {brand}, curated designer homeware, Hong Kong.",
        ),
        (
            "notFound.description",
            "zh-HK",
            "找不到頁面。{brand}，香港精選設計家居。",
        ),
    ] {
        assert_eq!(
            site_content_value(&db, key, locale).as_deref(),
            Some(want),
            "site_content ({key}, {locale}) after 0001..0006"
        );
    }
}

#[test]
fn seed_replay_twice_is_idempotent() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "seed_replay");
    db.apply_migrations();
    SeedCounts::read(&db).assert_opening_drop("0001..0006");

    // Replay 0003 twice more: each replay must apply without error (the whole
    // file runs through sqlx; a conflict would surface as a panic) and leave
    // every seeded count untouched.
    for replay in 1..=2 {
        db.apply_migration_file("0003_seed_opening_drop.sql");
        SeedCounts::read(&db).assert_opening_drop(&format!("seed replay #{replay}"));
    }
}

#[test]
fn seed_never_overwrites_admin_edits() {
    let Some(url) = common::scratch_db_url() else {
        return;
    };
    let db = ScratchDb::new(&url, "admin_edits");
    db.apply_migrations();

    // Three admin edits, one per conflict target of the seed: a product row
    // (ON CONFLICT (slug)), a site_content row (ON CONFLICT (key, locale)) and
    // a piece row (ON CONFLICT (piece_no)). Piece 1 is seeded as
    // panthella-160-v3-beige; we move it to a DIFFERENT product.
    db.block_on(async {
        let edited = sqlx::query(
            "UPDATE products SET price_cents = 999900 \
             WHERE slug = 'panthella-160-v3-beige'",
        )
        .execute(db.pool())
        .await
        .expect("price edit failed");
        assert_eq!(
            edited.rows_affected(),
            1,
            "price edit must touch exactly one product row"
        );

        let edited = sqlx::query(
            "UPDATE site_content SET value = $1 \
             WHERE key = 'hero.eyebrow' AND locale = 'en'",
        )
        .bind(EDIT_MARKER)
        .execute(db.pool())
        .await
        .expect("site copy edit failed");
        assert_eq!(
            edited.rows_affected(),
            1,
            "site copy edit must touch exactly one site_content row"
        );

        let edited = sqlx::query(
            "UPDATE pieces SET product_id = \
             (SELECT id FROM products WHERE slug = 'ph-3-2-table-chrome') \
             WHERE piece_no = 1",
        )
        .execute(db.pool())
        .await
        .expect("piece re-assignment edit failed");
        assert_eq!(
            edited.rows_affected(),
            1,
            "piece edit must touch exactly one pieces row"
        );
    });

    db.apply_migration_file("0003_seed_opening_drop.sql");

    // The seed's documented contract (0003 header): an existing row is never
    // overwritten, so all three admin edits must survive the replay untouched.
    db.block_on(async {
        let price_cents: i64 = sqlx::query_scalar(
            "SELECT price_cents FROM products WHERE slug = 'panthella-160-v3-beige'",
        )
        .fetch_one(db.pool())
        .await
        .expect("product lookup failed");
        assert_eq!(
            price_cents, 999900,
            "seed replay must not overwrite the admin price edit"
        );

        let eyebrow = sqlx::query_scalar::<_, String>(
            "SELECT value FROM site_content WHERE key = 'hero.eyebrow' AND locale = 'en'",
        )
        .fetch_one(db.pool())
        .await
        .expect("site_content lookup failed");
        assert_eq!(
            eyebrow, EDIT_MARKER,
            "seed replay must not overwrite the admin site copy edit"
        );

        let piece_owner = sqlx::query_scalar::<_, String>(
            "SELECT p.slug FROM pieces JOIN products p ON p.id = pieces.product_id \
             WHERE pieces.piece_no = 1",
        )
        .fetch_one(db.pool())
        .await
        .expect("piece lookup failed");
        assert_eq!(
            piece_owner, "ph-3-2-table-chrome",
            "seed replay must not overwrite the admin piece re-assignment"
        );
    });
}
