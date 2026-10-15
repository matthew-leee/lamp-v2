-- 0006_orders_reservations.sql — M1 money path (R5a): orders, reservations
-- and the payment-event idempotency ledger.
--
-- Contract this schema carries:
-- * STOCK-01: reserving an order MOVES units into `products.stock_reserved`
--   (0001/0005 columns). A lapsed reservation is RELEASED (the reserved count
--   drops) — a release is never a restock; only a returned unit enters
--   `stock_quarantined` (STOCK-01 / R2-11), so `available = stock_total -
--   stock_reserved - stock_quarantined` can never overcount saleable units.
--   `stock_reserved + stock_quarantined <= stock_total` stays application-side
--   (same contract as 0001/0005); the column CHECKs keep the counts non-negative.
-- * PAY-01: a VERIFIED gateway confirmation (order, amount, currency) is the
--   only thing that marks an order paid. `payment_events` is the idempotency
--   ledger behind that: `event_id` is the PRIMARY KEY and the webhook inserts
--   `ON CONFLICT (event_id) DO NOTHING`, so a replayed event can never deduct
--   stock twice.
-- * R2-5: an order whose reservation lapsed keeps its history; a late payment
--   moves it to 'late_payment_review' (human refund decision) instead of ever
--   being silently fulfilled against released stock.
--
-- Money is integer cents (0001 rule, never floats); timestamps are TIMESTAMPTZ.

CREATE TABLE IF NOT EXISTS orders (
    id BIGSERIAL PRIMARY KEY,
    order_code TEXT UNIQUE NOT NULL,
    product_id BIGINT NOT NULL REFERENCES products(id),
    product_slug TEXT NOT NULL,
    quantity INT NOT NULL CHECK (quantity >= 1),
    unit_price_cents BIGINT NOT NULL CHECK (unit_price_cents >= 0),
    total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
    currency TEXT NOT NULL,
    payment_method TEXT NOT NULL CHECK (payment_method IN ('card', 'fps')),
    status TEXT NOT NULL CHECK (status IN (
        'reserved',
        'paid',
        'fulfilled',
        'expired',
        'cancelled',
        'late_payment_review',
        'returned_quarantined'
    )),
    customer_name TEXT NOT NULL,
    customer_email TEXT NOT NULL,
    customer_phone TEXT NOT NULL DEFAULT '',
    delivery_address TEXT NOT NULL DEFAULT '',
    fps_reference TEXT NOT NULL DEFAULT '',
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The expiry sweep (`status = 'reserved' AND expires_at <= now()`) runs on
-- EVERY order touch (reserve, read, webhook, admin op); this index turns it
-- into a small range scan instead of a sequential scan of the whole table.
CREATE INDEX IF NOT EXISTS idx_orders_status_expires_at
    ON orders (status, expires_at);

-- PAY-01 idempotency ledger: one row per processed gateway event, keyed by
-- the gateway's own event id. A replay inserts nothing (`ON CONFLICT DO
-- NOTHING`) and the handler answers "already processed" without touching
-- stock or order state again.
CREATE TABLE IF NOT EXISTS payment_events (
    event_id TEXT PRIMARY KEY,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    event_type TEXT NOT NULL,
    amount_cents BIGINT NOT NULL,
    currency TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- FK lookups ("all gateway events for this order") without a scan.
CREATE INDEX IF NOT EXISTS idx_payment_events_order_id
    ON payment_events (order_id);
