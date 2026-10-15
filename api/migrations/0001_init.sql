-- 0001_init.sql — lamp initial schema.
--
-- Money is integer cents (HKD), never floats: price_cents is BIGINT cents.

CREATE TABLE products (
    id BIGSERIAL PRIMARY KEY,
    slug TEXT UNIQUE NOT NULL,
    brand TEXT NOT NULL,
    model TEXT NOT NULL,
    colourway TEXT NOT NULL,
    price_cents BIGINT NOT NULL CHECK (price_cents >= 0),
    stock_total INT NOT NULL CHECK (stock_total >= 0),
    stock_reserved INT NOT NULL DEFAULT 0 CHECK (stock_reserved >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- INVARIANT (application layer for now, trigger-free): stock_reserved <= stock_total.
-- Each column carries its own CHECK, but the cross-column invariant is enforced
-- by the application (all stock mutations will run as single atomic UPDATEs).
-- Promote this to a table-level CHECK or a deferred constraint trigger when the
-- reservation flow lands.