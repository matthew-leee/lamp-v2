# Curated Architectural Lighting Flagship & Operator Studio (v2 Clean-Room)

Production-ready bilingual (`en` / `zh-HK`) ecommerce flagship and operator studio for curated designer lighting & homeware in Hong Kong.

## Key Architecture & Features

1. **Zero-Hardcode White-Label Engine (`NAME-01`, `ENV-01`, `DATA-02`, `CONFIG-01`, `FPS-01`)**:
   - Every company scalar (Brand Name, Native Chinese Wordmark, Canonical URL, Currency, WhatsApp Concierge, HK FPS Merchant ID / Account Name / QR Code, Analytics) is resolved dynamically from live Admin Settings overrides (`settings` table) -> Environment defaults (`.env`) -> safe code defaults.
   - All product SKUs, prices (integer minor units / cents), bilingual heritage & electrical copy, verified showroom reference prices, and numbered edition pieces (`01–16`) live in PostgreSQL and are managed through the Admin Studio with **0-second storefront propagation** (`revalidatePath("/", "layout")`).
2. **Awwwards / Webby Exhibition Storefront (`web/`)**:
   - **Synchronized Room Atmosphere (`Daylight 5500K` ↔ `Nocturne 2700K`)**: Toggle from the sticky header, hero, or `L` keyboard shortcut; cross-fades dual-photo lamp stages with physically inspired 2700K radial light bloom, persisted across navigation.
   - **Self-Sufficient Product Photo & Lighting Modes (`MEDIA-01`)**:
     - Dual-photo SKUs (`image_day_url` + `image_lit_url`) cross-fade between Daylight 5500K and Nocturne 2700K with radial bloom.
     - Single-photo SKUs (`image_day_url` only, `image_lit_url` blank) render as clean plain photographs without artificial tinting or redundant 2700K tabs.
     - Setting `art_variant = "none"` disables the 1:10 CAD schematic tab for plain-photo or non-lamp items.
     - All 8 opening-drop demo images are cached locally under `web/public/demo-assets/` so zero external CDN timeouts occur.
   - **Interactive Collector's Edition `01–16` Matrix**: Live serialized edition strip with piece preview and real-time vault stock indicators.
   - **Crash-Proof New Products**: Newly created SKUs with `0` assigned pieces or empty copy render cleanly as **Open Edition / 常設單品**.
3. **Multi-SKU Shopping Cart, Real Stripe & HK FPS Settlement, and Live Transactional Emails (`CART-01`, `STRIPE-01`, `FPS-01`, `EMAIL-01`)**:
   - Slide-over **Shopping Cart (`購物車` / `加入購物車`)** supporting atomic multi-SKU reservation and checkout for **1 SKU up to all SKUs in the store** (`order_items` table with deadlock-free sorted row locking).
   - **Dedicated Checkout & Payment Settlement Page (`/[lang]/checkout/[code]`)** displaying all cart line items, live hold countdown timer, **Real Stripe Sandbox/Live Hosted Checkout Session + Return `session_id` & HMAC Webhook Verification (`POST /api/webhooks/stripe`)** alongside local test card fallback (`4242` success / `0002` decline recovery), **Live Admin-Configurable HK FPS Faster Payment Instructions & QR Code**, self-service hold cancellation, **Printable Official Collector's Pass**, and **Live Transactional HTML Email Delivery (`RESEND_API_KEY` or `SMTP_HOST`/`SMTP_USER`/`SMTP_PASS`)** + on-screen Email Preview modal.
   - **Payment / Email / Session Robustness (backend hardening, findings `2026-10-21`)**: reservation idempotency (`idempotency_key` body field / `Idempotency-Key` header — one order per pay tap, a lost-response retry never holds stock twice), order-stored `locale` (`en` / `zh-HK`) so paid & FPS-confirmation emails speak the customer's language, a durable paid-email delivery ledger (`POST /api/orders/{order_code}/email-delivery`, `paid_email_sent_at` dedupe + `email_last_error` for admin re-sends), webhook responses carrying recipient/email metadata, and server-side admin logout revocation (`POST /api/admin/logout` + `revoked_admin_sessions` deny-list — a stolen cookie dies at logout, not at expiry).
   - **Web / Email / Session / Upload Hardening (web + ops, same findings round)**: webhook-only Stripe settlements email the customer in their stored locale; `verify_session` cross-checks Stripe `amount_total`/`currency` against the DB order; Resend failures fall through to SMTP (`G15`); the admin Orders console shows per-order `locale` + delivery status with **Re-send Receipt**; checkout clients send per-attempt idempotency keys and `/[lang]/orders` offers a **Resume recent order** chip; all `/admin/api` mutations carry same-origin CSRF checks (`403`) and studio edits survive session expiry via `sessionStorage` drafts; photo uploads are media-only (MIME allowlist + magic-byte proof + `crypto.randomUUID()` names + 16 MB pre-check) and persist on the `uploads_data` volume; storefront data loading degrades gracefully, GA4 renders only with a validated Measurement ID, `prefers-reduced-motion` is honoured, and hold cancellation requires two-step confirmation.
   - **Exactly-once Paid Email, Durable Pay-Attempt Identity & Upload Serving (reviewer `gpt-6.1-sol` round)**: paid receipts go through an ATOMIC claim (`claim: true` on the email-delivery ledger — exactly one dispatcher wins, a failed send releases the claim for retry/admin re-send) and Caddy routes `/api/webhooks/stripe` through the Next.js webhook route so webhook-only settlements settle AND email AND revalidate; pay-attempt `idempotency_key`s survive refresh/second-tab retries via `localStorage` (`lamp_pending_attempt_v2`, 30-minute TTL) and the API resolves a keyed retry racing for the last unit in stock to the original order; admin session tokens carry a unique `jti` (same-second logins can no longer share one revocation hash) with fail-closed revocation checks and a fail-closed logout; uploads are enforced by a streaming 16 MB counter + strict magic bytes and served read-only from `/api/uploads/{filename}` (`nosniff`, immutable cache) so runtime uploads work under `next start`; settings validation enforces 7–15 WhatsApp digits and real http(s) `site.url` values, and the studio draft only clears after a save when no newer edits raced it.
   - **Round-2 reviewer hardening (claim recovery, `kind`, strict LANG-01)**: the paid-email ledger separates the RECOVERABLE in-flight claim (`paid_email_claimed_at`, a 2-minute lease) from CONFIRMED delivery (`paid_email_sent_at`, stamped only when the dispatch actually succeeds) — a won claim answers `paid_email_sent: false`, an expired lease is re-claimable, and a later failed re-send never wipes an already-confirmed delivery; `EmailDeliveryRequest.kind` (`"reservation"` | `"paid"`) keeps hold-confirmation re-sends off the PAID-email ledger (admin `Re-send Receipt` reports `kind: order.status === "reserved" ? "reservation" : "paid"`); EVERY automatic paid-email path (`webhooks/stripe`, `pay-card`, admin `confirm_fps`) claims before dispatch and releases the claim via the ledger on ANY post-claim failure (try/catch claim release); admin `POST /api/admin/logout` fails closed (`503`) when the DB pool is absent instead of silently skipping revocation; the deploy hook waits for `pg_isready` before migrating, FAILS (no `|| true`) when preserved uploads cannot be copied (re-owned `1000:1000` so the web user can still write new uploads) and restarts `caddy` so the bind-mounted `ops/Caddyfile` reloads; a lost response on the LAST unit stays recoverable (`hasPendingAttemptForSlug` + `canRecoverPending` re-open the express checkout to replay the same `idempotency_key`); and `zh-HK` routes never fall back to English product copy (incl. localized `Not Found` metadata title and enquiry-message fallback).
4. **Operator-First Admin Studio (`/admin`)**:
   - **Unified Single-Save Product Studio (`/admin/products/[id]`)** with real-time as-you-type stage preview, **direct photo file upload** (stored on the `uploads_data` volume and served read-only via `/api/uploads/{filename}`), **1-click Duplicate/Clone SKU**, human **Dollar** price inputs, `art_variant` (`dome | shades | shades-tall | piano | none`), side-by-side `EN | 繁體中文` copy editors, and **Quarantine Inspection** (*Pass Inspection -> Restock* or *Write Off Damaged*).
   - **Multi-SKU Orders & Quarantine Console (`/admin/orders`)** and **Live Company & FPS Settings (`/admin/settings`)**.

## Deployment (Finland Hetzner CX33 + Cloudflare Tunnel & Auto-HTTPS)

The full Docker Compose stack (`db`, `api`, `web`, `caddy`, `cloudflared`) runs in isolation on the Hetzner CX33 (`89.167.15.159` under `/opt/storefront-v2`) without touching any existing host services or Tailscale listeners (`BIND_IP=89.167.15.159`):
- **Automatic Let's Encrypt HTTPS (`sslip.io`)**: `https://89-167-15-159.sslip.io`
- **Cloudflare Tunnel (`cloudflared`)**: forwards `http://caddy:8080` to a public `*.trycloudflare.com` HTTPS endpoint (compatible with NordVPN without needing Tailscale).
- **Stripe webhooks via the web tier**: Caddy routes `/api/webhooks/stripe` to `web:3002` (the Next.js webhook route), so a webhook-only settlement settles in the API AND sends the paid email + revalidates the cache; `/health` still probes `api:8080` directly.

### One-Line VPS & Git Auto-Deploy CLI (`scripts/vps.mjs`)

`scripts/vps.mjs` reads `HETZNER_VPS_PASSPHRASE` from `.env` automatically and provides a single-command workflow for Git-backed auto-deploy, remote log inspection, and database resets:

```bash
# Push current Git HEAD to /opt/storefront-v2.git on VPS & auto-rebuild via post-receive hook
# (hook order: start db -> wait for pg_isready -> apply
#  api/migrations/0008_payment_email_session_hardening.sql idempotently -> migrate any
#  existing container /app/public/uploads into the uploads_data volume (copy failure FAILS
#  the deploy; files re-owned 1000:1000) -> docker compose up -d --build -> restart caddy
#  so the bind-mounted ops/Caddyfile reloads)
node scripts/vps.mjs deploy

# Inspect container status, health, and active Cloudflare Tunnel URL
node scripts/vps.mjs status

# Tail live container logs (e.g. web, api, [email-receipt]) without manual SSH
node scripts/vps.mjs logs web

# Reset VPS database to the pristine 8 SKUs / 16 units / 0 orders baseline
node scripts/vps.mjs reset-db

# Execute a one-line shell command on the VPS
node scripts/vps.mjs exec "docker compose ps"
```

- **GitHub Repository (`origin`)**: `https://github.com/matthew-leee/lamp-v2.git` (branches `v2-clean-room` and `main`).

## Development & Verification

```bash
# Rust API (port 18080)
cd api && STOREFRONT_TEST_DATABASE_URL="postgres://storefront:scratch-only@localhost:55433/storefront_scratch" cargo test
# Next.js Web (port 3002)
cd web && npm run lint && npm run typecheck && npm run build && npm test
```
