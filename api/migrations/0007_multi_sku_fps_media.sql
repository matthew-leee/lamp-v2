-- 0007_multi_sku_fps_media.sql — Multi-SKU Shopping Cart line items (CART-01),
-- self-sufficient product media & 'none' art_variant (MEDIA-01), and locally
-- cached opening-drop demo assets.
--
-- Contracts this migration enforces:
-- * CART-01: An order may hold 1 to N distinct SKUs in one atomic reservation
--   and payment settlement transaction. `order_items` stores the per-SKU line
--   items (`product_id`, `product_slug`, `quantity`, `unit_price_cents`,
--   `line_total_cents`). Any existing single-SKU orders are idempotently
--   backfilled into `order_items`.
-- * MEDIA-01(c): `products.art_variant` gains `'none'` alongside the four
--   lamp silhouettes so non-lamp or plain-photo items can disable the 1:10 CAD
--   blueprint tab without code changes.
-- * MEDIA-01(d): The 8 opening-drop SKUs point to locally cached assets under
--   `/demo-assets/*` so zero external CDN timeouts or mixed-network failures
--   occur.

-- ---------------------------------------------------------------------------
-- 1. Multi-SKU order_items table (CART-01)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS order_items (
    id BIGSERIAL PRIMARY KEY,
    order_id BIGINT NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id BIGINT NOT NULL REFERENCES products(id),
    product_slug TEXT NOT NULL,
    quantity INT NOT NULL CHECK (quantity >= 1),
    unit_price_cents BIGINT NOT NULL CHECK (unit_price_cents >= 0),
    line_total_cents BIGINT NOT NULL CHECK (line_total_cents >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (order_id, product_id)
);

CREATE INDEX IF NOT EXISTS idx_order_items_order_id
    ON order_items (order_id);

-- Backfill any pre-existing single-SKU orders into order_items idempotently.
INSERT INTO order_items
    (order_id, product_id, product_slug, quantity, unit_price_cents, line_total_cents, created_at)
SELECT id, product_id, product_slug, quantity, unit_price_cents, total_cents, created_at
FROM orders
ON CONFLICT (order_id, product_id) DO NOTHING;

-- ---------------------------------------------------------------------------
-- 2. art_variant: add 'none' for plain-photo / non-schematic items (MEDIA-01c)
-- ---------------------------------------------------------------------------

ALTER TABLE products DROP CONSTRAINT IF EXISTS products_art_variant_check;
ALTER TABLE products ADD CONSTRAINT products_art_variant_check
    CHECK (art_variant IN ('dome', 'shades', 'shades-tall', 'piano', 'none'));

-- ---------------------------------------------------------------------------
-- 3. Local cached demo assets for the 8 opening-drop SKUs (MEDIA-01d)
-- ---------------------------------------------------------------------------

UPDATE products SET
    image_day_url = '/demo-assets/panthella-160-v3-beige.png',
    image_lit_url = '/demo-assets/panthella-160-v3-beige.png'
WHERE slug = 'panthella-160-v3-beige';

UPDATE products SET
    image_day_url = '/demo-assets/panthella-160-v3-white-opal.webp',
    image_lit_url = '/demo-assets/panthella-160-v3-white-opal.webp'
WHERE slug = 'panthella-160-v3-white-opal';

UPDATE products SET
    image_day_url = '/demo-assets/panthella-160-v3-opal-beige-chrome.png',
    image_lit_url = '/demo-assets/panthella-160-v3-opal-beige-chrome.png'
WHERE slug = 'panthella-160-v3-opal-beige-chrome';

UPDATE products SET
    image_day_url = '/demo-assets/panthella-160-v3-opal-pale-blue-day.webp',
    image_lit_url = '/demo-assets/panthella-160-v3-opal-pale-blue-lit.webp'
WHERE slug = 'panthella-160-v3-opal-pale-blue';

UPDATE products SET
    image_day_url = '/demo-assets/panthella-160-v3-opal-pale-rose-day.webp',
    image_lit_url = '/demo-assets/panthella-160-v3-opal-pale-rose-lit.webp'
WHERE slug = 'panthella-160-v3-opal-pale-rose';

UPDATE products SET
    image_day_url = '/demo-assets/ph-2-1-table-chrome.png',
    image_lit_url = '/demo-assets/ph-2-1-table-chrome.png'
WHERE slug = 'ph-2-1-table-chrome';

UPDATE products SET
    image_day_url = '/demo-assets/ph-3-2-table-chrome-day.webp',
    image_lit_url = '/demo-assets/ph-3-2-table-chrome-lit.webp'
WHERE slug = 'ph-3-2-table-chrome';

UPDATE products SET
    image_day_url = '/demo-assets/ph-1-1-piano-table-limited-day.webp',
    image_lit_url = '/demo-assets/ph-1-1-piano-table-limited-lit.png'
WHERE slug = 'ph-1-1-piano-table-limited';
