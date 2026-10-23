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
- **Idempotency (one order per pay tap)**: an optional `idempotency_key` (body field, <= 128 chars after trimming, NUL-free) or `Idempotency-Key` HTTP header (fallback when the body carries none) identifies ONE pay attempt. Replaying a known key returns the FIRST order unchanged — same `order_code`, no second stock hold — even across concurrent retries (the `orders.idempotency_key` UNIQUE constraint picks one winner), including the race where the retry arrives while the first attempt is still taking the LAST unit in stock: the insufficient-stock refusal is re-checked against the key before it is returned. Blank/absent keys mean no idempotency (legacy clients).
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
    "delivery_address": "Central, Hong Kong",
    "locale": "en",
    "idempotency_key": "pay-attempt-0001"
  }
  ```
  *(Single-SKU `slug` + `quantity` fields are also accepted for backward compatibility. `locale` is `"en"` | `"zh-HK"` (LANG-01) and defaults to `"en"`; `idempotency_key` is optional.)*
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
    "locale": "en",
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
    ],
    "idempotent_replay": false
  }
  ```
  *(`idempotent_replay` is `false` on a fresh insert and `true` when this answer REPLAYS an earlier attempt loaded by `idempotency_key` — the web tier must not dispatch a second reservation email for a replay.)*
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
    "locale": "en",
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

### `POST /api/orders/{order_code}/email-delivery`
Paid-email delivery ledger: the web tier reports every paid-confirmation email outcome here so sends are deduplicated and failures stay visible. The ledger separates RECOVERABLE claim state (`paid_email_claimed_at`, "a send is in flight") from CONFIRMED delivery (`paid_email_sent_at`, "the customer WAS told") — a claim is a 2-minute lease on the one send, never proof of delivery. Optional body field `kind` names which email the outcome belongs to: `"reservation"` (the hold confirmation) vs `"paid"` (the paid receipt, the default). Four call shapes share this endpoint (response is always `{"ok": true, "paid_email_sent": <post-update paid_email_sent_at IS NOT NULL>, "claimed": <did THIS call win the claim>}`):
- **Atomic claim** — Body `{"claim": true, "sent": false}`: atomically wins THE one paid-email send, only for a `paid`/`fulfilled` order whose `paid_email_sent_at IS NULL` and whose claim is free (never taken, or the 2-minute lease expired). Exactly one caller ever gets `{"claimed": true, "paid_email_sent": false}` — a won claim reports `paid_email_sent: false` because claiming is not delivery; every other caller (lease held, already sent, or an unpaid order) gets `{"claimed": false, "paid_email_sent": <current state>}`. Automatic send paths claim BEFORE dispatching (and release the claim on ANY post-claim failure) so concurrent settlements can never double-send and a crashed dispatcher cannot block the receipt past the lease.
- **Reservation outcome** — Body `{"sent": true, "kind": "reservation"}`: the hold-confirmation email was delivered — clears `email_last_error` ONLY. It never touches `paid_email_sent_at` or the paid claim, even on a later-settled order.
- **Paid success stamp** — Body `{"sent": true}` (or `"kind": "paid"`): stamps `paid_email_sent_at` (COALESCE'd — a repeat can never move the FIRST delivery moment), releases the claim and clears `email_last_error`. Stamped ONLY when the order is `paid`/`fulfilled`: an outcome reported for a `reserved` order never marks the PAID email as sent.
- **Failure release** — Body `{"sent": false, "error": "..."}` (no claim): an attempt failed after claiming (or a manual re-send failed later) — RELEASES the claim (`paid_email_claimed_at = NULL`) and stores `email_last_error`, so a retry or the admin "re-send receipt" can still reach the customer. An already-confirmed `paid_email_sent_at` is NEVER wiped by a later failed re-send.

`404` for an unknown order code; `400` for a NUL byte or > 2000-char error note.

`404` for an unknown order code; `400` for a NUL byte or > 2000-char error note.

### `POST /api/webhooks/stripe`
Payment settlement authority (`PAY-01`). Requires header `Stripe-Signature: t=<unix_ts>,v1=<hex_hmac_sha256>` signed with `STRIPE_WEBHOOK_SECRET` over `<unix_ts>.<raw_body>`.
- Accepts either a standard Stripe Event envelope (`id`, `type` in `payment_intent.succeeded | checkout.session.completed | charge.succeeded`, `data.object.metadata.order_code`, `data.object.amount_total | amount_received | amount`, `data.object.currency`) or a flat JSON body (`event_id`, `event_type`, `order_code`, `amount_cents`, `currency`).
- **Response `200 OK`**: `{"ok": true, "idempotent": false, "status": "paid", "order_code": "ORD-…", "customer_name": "…", "customer_email": "…", "locale": "en", "paid_email_sent": false}` — the metadata tells the web tier whom to email, in which language, and whether the paid-confirmation email is already recorded as delivered (`paid_email_sent`).

---

## 3. Admin Authentication & CRUD Endpoints (`/api/admin/*`)

All non-`GET` requests under `/api/admin/*` require header `X-Requested-With: XMLHttpRequest` (or any non-empty value). All routes except `/api/admin/login` require cookie `admin_session=<signed_token>`.

- `POST /api/admin/login` — Body: `{"username": "...", "password": "..."}` -> `200 OK` `{"ok": true}` + `Set-Cookie: admin_session=...; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800`
- `POST /api/admin/logout` -> `200 OK` `{"ok": true}` + `Set-Cookie: admin_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0` (clears the cookie). SERVER-SIDE revocation: the token's SHA-256 is recorded in `revoked_admin_sessions` (never the token itself) and `require_admin` refuses the token from that moment on — a stolen cookie dies at logout, not at `exp`. The revocation check FAILS CLOSED: any database error (including a missing `revoked_admin_sessions` table) answers `503` instead of letting a possibly-revoked session through.
- Session token claims: `{"sub": <username>, "exp": <unix seconds>, "jti": <timestamp-micros hex + counter>}` — the `jti` makes every login token unique even within the same second (two logins never share one token or one revocation-ledger row); tokens minted before `jti` remain valid until their own `exp`.
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
