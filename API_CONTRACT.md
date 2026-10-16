# Rust Backend API Contract (`lamp-api`)

This document specifies the HTTP/JSON interface exposed by the Rust (`axum` + `sqlx` + PostgreSQL) backend in `api/src/`. Any frontend or client must integrate strictly through these endpoints and environment variables.

## 1. Public Storefront & Availability Endpoints

### `GET /health`
- **Response `200 OK`**: `{"status": "ok"}`

### `GET /api/catalogue`
Returns all active products (`WHERE active = TRUE`) with their locale-keyed copy, verified reference prices, numbered drop pieces, and total drop piece count (`COUNT(*) FROM pieces`). Contains **zero** stock fields.
- **Response `200 OK`**:
  ```json
  {
    "total_pieces": 16,
    "products": [
      {
        "slug": "panthella-160-v3-beige",
        "brand": "Louis Poulsen",
        "model": "Panthella 160 V3 Portable",
        "price_cents": 161500,
        "family": "panthella-160-portable",
        "active": true,
        "art_variant": "dome",
        "art_hue": 35,
        "image_day_url": "/demo-assets/panthella-160-v3-beige.png",
        "image_lit_url": "/demo-assets/panthella-160-v3-beige.png",
        "designer": "Verner Panton",
        "design_year": "1971",
        "dimensions": "Ø 160 × H 238 mm",
        "light_source": "2700K Integrated LED · 2.5W · Stepless / 3-Step Dimmer",
        "content": {
          "en": {
            "colourway": "Beige",
            "heritage": "...",
            "electrical": "..."
          },
          "zh-HK": {
            "model_alt": "Panthella 160 V3 便攜檯燈",
            "colourway": "米色",
            "heritage": "...",
            "electrical": "..."
          }
        },
        "reference_price": {
          "min_price_cents": 190000,
          "max_price_cents": 280000,
          "source_name": "MyConcept Hong Kong",
          "source_url": "https://...",
          "checked_on": "2026-10-12"
        },
        "pieces": [1, 2],
        "units_in_drop": 2
      }
    ]
  }
  ```
  - Note: Newly created products may have `pieces: []` (`units_in_drop: 0`), `reference_price: null`, and empty/partial `content: {}`. The frontend must render those gracefully without throwing.
  - `art_variant` is one of `"dome" | "shades" | "shades-tall" | "piano" | "none"`.

### `GET /api/products`
Live stock channel (`Cache-Control: no-store`). Automatically sweeps expired reservations before reading.
- **Response `200 OK`**:
  ```json
  [
    {
      "id": 1,
      "slug": "panthella-160-v3-beige",
      "brand": "Louis Poulsen",
      "model": "Panthella 160 V3 Portable",
      "price_cents": 161500,
      "stock_total": 2,
      "stock_reserved": 0,
      "stock_quarantined": 0,
      "created_at": "2026-10-15T00:40:36.044132Z"
    }
  ]
  ```
  - Saleable availability for a SKU is `stock_total - stock_reserved - stock_quarantined`.

### `GET /api/site-content`
Locale-keyed company and editorial copy (`site_content` table).
- **Response `200 OK`**:
  ```json
  {
    "content": {
      "hero.eyebrow": { "en": "...", "zh-HK": "..." },
      "hero.lead": { "en": "...", "zh-HK": "..." }
    }
  }
  ```
  - Template placeholders `{brand}`, `{pieces}`, and `{subject}` are interpolated by the caller.

### `GET /api/settings`
Public runtime overrides (`settings` table), restricted to the 10 whitelisted company keys:
`brand.name`, `brand.name_native`, `site.url`, `contact.whatsapp`, `currency`, `analytics.ga_measurement_id`, `analytics.gsc_verification`, `payment.fps_id`, `payment.fps_account_name`, `payment.fps_qr_url`.
- **Response `200 OK`**: Flat JSON map `{"brand.name": "...", "currency": "HKD", "payment.fps_id": "..."}`.
- Resolution order (`CONFIG-01`): `settings` override (where empty string `""` on an optional key means explicitly OFF) -> `NEXT_PUBLIC_*` environment variable -> code default (`currency` defaults to `"HKD"`).

---

## 2. Public Checkout, Order & Payment Webhook Endpoints

### `POST /api/orders/reserve`
Atomically reserves stock (`30 minutes` for `"card"`, `24 hours` for `"fps"`) using the authoritative database price (`products.price_cents`) across 1 to N distinct SKUs (`CART-01`, `order_items` table).
- **Request JSON**:
  ```json
  {
    "items": [
      { "slug": "panthella-160-v3-beige", "quantity": 1 },
      { "slug": "ph-2-1-table-chrome", "quantity": 1 }
    ],
    "payment_method": "card",
    "customer_name": "Chan Tai Man",
    "customer_email": "chan@example.com",
    "customer_phone": "85291234567",
    "delivery_address": "Central, Hong Kong"
  }
  ```
  *(Single-SKU `slug` + `quantity` fields are also accepted for backward compatibility.)*
- **Response `200 OK`**:
  ```json
  {
    "id": 1,
    "order_code": "ORD-1760500000000-1",
    "product_id": 1,
    "product_slug": "panthella-160-v3-beige",
    "quantity": 2,
    "unit_price_cents": 161500,
    "total_cents": 875500,
    "currency": "HKD",
    "payment_method": "card",
    "status": "reserved",
    "customer_name": "Chan Tai Man",
    "customer_email": "chan@example.com",
    "expires_at": "2026-10-15T06:30:00Z",
    "created_at": "2026-10-15T06:00:00Z",
    "items": [
      {
        "product_id": 1,
        "product_slug": "panthella-160-v3-beige",
        "quantity": 1,
        "unit_price_cents": 161500,
        "line_total_cents": 161500
      },
      {
        "product_id": 6,
        "product_slug": "ph-2-1-table-chrome",
        "quantity": 1,
        "unit_price_cents": 714000,
        "line_total_cents": 714000
      }
    ]
  }
  ```
- **Error Responses**: `400` validation error, `404` product not found/inactive, `409` `{"error": "insufficient stock available"}`.

### `GET /api/orders/{order_code}`
Sweeps expired holds and returns public order status (`Cache-Control: no-store`, no PII) including all line items (`items`).
- **Response `200 OK`**:
  ```json
  {
    "order_code": "ORD-1760500000000-1",
    "product_slug": "panthella-160-v3-beige",
    "quantity": 2,
    "unit_price_cents": 161500,
    "total_cents": 875500,
    "currency": "HKD",
    "payment_method": "card",
    "status": "reserved",
    "fps_reference": "",
    "expires_at": "2026-10-15T06:30:00Z",
    "created_at": "2026-10-15T06:00:00Z",
    "items": [
      {
        "product_id": 1,
        "product_slug": "panthella-160-v3-beige",
        "quantity": 1,
        "unit_price_cents": 161500,
        "line_total_cents": 161500
      },
      {
        "product_id": 6,
        "product_slug": "ph-2-1-table-chrome",
        "quantity": 1,
        "unit_price_cents": 714000,
        "line_total_cents": 714000
      }
    ]
  }
  ```

### `POST /api/orders/{order_code}/submit-fps`
Allows a customer with an FPS reservation to submit their bank transfer reference (`{"fps_reference": "..."}`). Returns `200 OK` `PublicOrderStatus`.

### `POST /api/orders/{order_code}/cancel`
Allows a customer with an unpaid `reserved` order to self-cancel the hold (freeing `stock_reserved` immediately so they can switch payment methods or release the piece). Returns `200 OK` `PublicOrderStatus`.

### `POST /api/webhooks/stripe`
Payment settlement authority (`PAY-01`). Requires header `Stripe-Signature: t=<unix_ts>,v1=<hex_hmac_sha256>` signed with `STRIPE_WEBHOOK_SECRET` over `<unix_ts>.<raw_body>`.
- Accepts either a standard Stripe Event envelope (`id`, `type` in `payment_intent.succeeded | checkout.session.completed | charge.succeeded`, `data.object.metadata.order_code`, `data.object.amount_total | amount_received | amount`, `data.object.currency`) or a flat JSON body (`event_id`, `event_type`, `order_code`, `amount_cents`, `currency`).
- **Response `200 OK`**: `{"ok": true, "idempotent": false, "status": "paid"}`

---

## 3. Admin Authentication & CRUD Endpoints (`/api/admin/*`)

All non-`GET` requests under `/api/admin/*` require header `X-Requested-With: XMLHttpRequest` (or any non-empty value). All routes except `/api/admin/login` require cookie `admin_session=<signed_token>`.

- `POST /api/admin/login` — Body: `{"username": "...", "password": "..."}` -> `200 OK` `{"ok": true}` + `Set-Cookie: admin_session=...; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800`
- `GET /api/admin/whoami` -> `200 OK` `{"username": "..."}`
- `GET /api/admin/products` -> `200 OK` `AdminProduct[]` (all products, active & inactive)
- `POST /api/admin/products` -> Create product (`slug`, `brand`, `model`, `price_cents`, `family`, `art_variant`, `art_hue`, `active`)
- `GET /api/admin/products/{id}` -> `200 OK` `AdminProductFull` (complete product aggregate including `stock_total`, `stock_reserved`, `stock_quarantined`, `image_day_url`, `image_lit_url`, `designer`, `design_year`, `dimensions`, `light_source`, `content`, `reference_price`, `pieces` — works for both active and inactive products)
- `PUT /api/admin/products/{id}` -> Partial product update
- `PUT /api/admin/products/{id}/full` -> Atomic full-product save (product fields + optional `stock_total`/`stock_quarantined` + `content` map + `reference_price` upsert/null-delete in one transaction)
- `POST /api/admin/products/{id}/clone` -> 1-click duplicate of product `#id` (copies specs, media URLs, reference price, and bilingual copy into a new inactive SKU) -> `200 OK` `AdminProductFull`
- `POST /api/admin/products/{id}/inspect-quarantine` -> Body `{"action": "restock" | "write_off", "quantity": 1}` -> resolves quarantined units after physical inspection -> `200 OK` `AdminProductFull`
- `PUT /api/admin/product-content` -> Upsert `(product_id, locale, field, value)`
- `PUT / DELETE /api/admin/reference-prices/{product_id}` -> Upsert or delete reference price
- `POST /api/admin/pieces`, `PATCH / DELETE /api/admin/pieces/{piece_no}` -> Create, reassign (`{"product_id": N}`), or delete a numbered piece
- `PUT /api/admin/site-content`, `DELETE /api/admin/site-content/{key}/{locale}` -> Upsert or delete site copy
- `PUT / DELETE /api/admin/settings/{key}` -> Upsert (`{"value": "..."}`) or delete runtime setting override
- `GET /api/admin/orders` -> Last 200 orders newest-first
- `POST /api/admin/orders/{id}/confirm-fps` -> Body `{"fps_reference": "..."}` (marks `reserved` FPS order `paid` or `expired` FPS order `late_payment_review`)
- `POST /api/admin/orders/{id}/fulfil` -> Marks a `paid` order `fulfilled`
- `POST /api/admin/orders/{id}/release` -> Releases an unpaid `reserved` order (`cancelled`, returns `stock_reserved`)
- `POST /api/admin/orders/{id}/quarantine-return` -> Marks a `paid`/`fulfilled` order `returned_quarantined` and increments both `stock_total` and `stock_quarantined`
- `GET /api/admin/audit-log` -> Last 200 audit log entries newest-first
