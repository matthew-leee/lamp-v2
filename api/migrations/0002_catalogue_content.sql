-- 0002_catalogue_content.sql — catalogue content, drop numbering, site copy,
-- runtime settings and audit trail.
--
-- DATA-02: product content is database data, seeded, served through the API and
-- CRUD-managed via the admin console; never hardcoded inline in a TS file.
-- LOCALE-01: the locale set is env-configurable, so content is locale-KEYED
-- (product_content.locale TEXT) rather than fixed _en/_zh_hk columns. Locale
-- values are validated against the configured set in the application layer,
-- not by a DB CHECK, so another company may run any locale pair.
--
-- Money stays integer cents everywhere (never floats).
--
-- Tradeoff, chosen deliberately: family copy (heritage/electrical) is stored per
-- SKU (product_content rows) rather than normalised into per-family blocks. The
-- CRUD unit in the admin console is the SKU, and the seed repeats the family
-- copy across a family's colourways; the alternative (a family join) complicates
-- every read for a deduplication the admin can do with a copy-paste.

ALTER TABLE products
    -- colourway becomes bilingual content in product_content (locale-keyed).
    DROP COLUMN colourway,
    -- Copy family (e.g. 'panthella-160-portable'); informational, groups SKUs.
    ADD COLUMN family TEXT NOT NULL DEFAULT '',
    -- Placeholder art configuration is product data (kills the slug-prefix
    -- art rule in the web tier: no more product.slug.startsWith('ph-')).
    ADD COLUMN art_variant TEXT NOT NULL DEFAULT 'dome'
        CHECK (art_variant IN ('dome', 'shades')),
    ADD COLUMN art_hue INT NOT NULL DEFAULT 35
        CHECK (art_hue BETWEEN 0 AND 359),
    -- Soft visibility: inactive SKUs stay as data but leave the storefront.
    ADD COLUMN active BOOLEAN NOT NULL DEFAULT TRUE;

-- Bilingual product content, one row per (product, locale, field).
-- No locale CHECK: locale values are validated against the env-configured set
-- in the application layer (LOCALE-01).
CREATE TABLE product_content (
    product_id BIGINT NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    locale TEXT NOT NULL,
    field TEXT NOT NULL
        CHECK (field IN ('model_alt', 'colourway', 'heritage', 'electrical')),
    value TEXT NOT NULL,
    PRIMARY KEY (product_id, locale, field)
);

-- Labelled reference price (TRUST-02): one verified reference per product.
-- `min`/`max` because the comparison is a checked range, not a single number.
-- No row for a product means no verified reference exists; the UI says
-- verification pending instead of inventing one.
CREATE TABLE reference_prices (
    product_id BIGINT PRIMARY KEY REFERENCES products(id) ON DELETE CASCADE,
    min_price_cents BIGINT NOT NULL CHECK (min_price_cents >= 0),
    max_price_cents BIGINT NOT NULL CHECK (max_price_cents >= 0),
    CHECK (min_price_cents <= max_price_cents),
    source_name TEXT NOT NULL,
    source_url TEXT NOT NULL,
    checked_on DATE NOT NULL
);

-- DROP-01: the numbered drop. One row per physical piece, numbered across the
-- whole drop (01-16 for the opening drop). Derived values are COUNTs, never
-- constants: total pieces = COUNT(*), units per SKU = COUNT(*) per product.
-- piece_no is unique across the drop (a number belongs to exactly one piece).
-- Stock quantity stays on products (stock_total/stock_reserved, 0001); pieces
-- are the drop's identity record, not the inventory count.
CREATE TABLE pieces (
    piece_no INT PRIMARY KEY CHECK (piece_no >= 1),
    product_id BIGINT NOT NULL REFERENCES products(id)
);

-- Site copy: identity, positioning, policy and meta copy, locale-keyed.
-- Chrome labels (navigation, availability states, section labels) stay in code;
-- everything carrying brand, sold-brand, counts, policy or legal claims is data.
CREATE TABLE site_content (
    key TEXT NOT NULL,
    locale TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (key, locale)
);

-- Runtime settings: admin-editable OVERRIDES for non-secret keys only
-- (CONFIG-01). Resolution is override -> env default -> code default.
-- This table stays empty unless an admin sets an override; seeding does not
-- duplicate env defaults here. Secrets (DATABASE_URL, gateway/session keys)
-- are env-only and never enter this table.
--
-- Build-baked exception, documented: the locale set is env-only, NOT
-- settings-overridable. Routes and generateStaticParams are baked at build
-- (dynamicParams = false blocks unknown locales with a 404), so a locale change
-- requires a rebuild/redeploy, which the admin console cannot trigger on demand.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Audit trail for admin mutations (commitment 2: every state-changing request
-- is authorised server-side and recorded against the acting user). Written by
-- the D2 admin console; schema lands here so D2 needs no further migration.
CREATE TABLE audit_log (
    id BIGSERIAL PRIMARY KEY,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    entity TEXT NOT NULL,
    entity_id TEXT,
    detail JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);