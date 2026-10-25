# Curated Architectural Lighting Flagship & Operator Studio (v2 Clean-Room)

Production-ready bilingual (`en` / `zh-HK`) ecommerce flagship and operator studio for curated designer lighting & homeware in Hong Kong.

## Key Architecture & Features

1. **Zero-Hardcode White-Label Engine (`NAME-01`, `ENV-01`, `DATA-02`, `CONFIG-01`, `FPS-01`)**:
   - Every company scalar (Brand Name, Native Chinese Wordmark, Canonical URL, Currency, WhatsApp Concierge, HK FPS Merchant ID / Account Name / QR Code, Analytics) is resolved dynamically from live Admin Settings overrides (`settings` table) -> Environment defaults (`.env`) -> safe code defaults.
   - All product SKUs, prices (integer minor units / cents), bilingual heritage & electrical copy, serialized edition pieces and public site copy live in PostgreSQL and are managed through the Admin Studio with **0-second storefront propagation** (`revalidatePath("/", "layout")`); the admin-editable `brand.logo_url` slot renders a configured logo with the locale wordmark as fallback (`ENV-01`).
2. **Awwwards / Webby Exhibition Storefront (`web/`)**:
   - **Synchronized Room Atmosphere (`Daylight 5500K` ↔ `Nocturne 2700K`)**: Toggle from the sticky header, hero, or `L` keyboard shortcut; cross-fades dual-photo lamp stages with physically inspired 2700K radial light bloom, persisted across navigation.
   - **Self-Sufficient Product Photo & Lighting Modes (`MEDIA-01`)**:
     - Dual-photo SKUs (`image_day_url` + `image_lit_url`) cross-fade between Daylight 5500K and Nocturne 2700K with radial bloom.
     - Single-photo SKUs (`image_day_url` only, `image_lit_url` blank) render as clean plain photographs without artificial tinting or redundant 2700K tabs.
     - Setting `art_variant = "none"` disables the 1:10 CAD schematic tab for plain-photo or non-lamp items.
     - All 8 opening-drop demo images are cached locally under `web/public/demo-assets/` so zero external CDN timeouts occur.
   - **Asymmetric Hero & Rotating Piece Carousel (`DROP-01`, 2026-10-23)**: A framed `brand.logo_url` slot (locale wordmark fallback) with a concise six-word-max slogan sits beside an auto-rotating (2.2s, dissolve) product carousel offering prev/next, an explicit touch/keyboard **Pause/Play** toggle (`⏸`/`▶`, `aria-pressed`; a manual pause survives slide changes and hover/focus pauses), a numbered `.reel-x` indicator strip (`36×36px`, `aria-current` + `data-active`, each indicator naming its piece in both locales) and a `fit="contain"` stage. The entire Hero (left column and carousel card) fits 100% of the first fold on both 16:9 (`1440×740`) and 16:10 (`1440×900`) laptops; autoplay never starts under `prefers-reduced-motion: reduce`.
   - **Two Categorized Lineup Galleries (`DROP-01`)**: **Gallery I — Flagship PH Table Series** (3-up architectural plinth cards) and **Gallery II — Panthella 160 V3 Portable 5-Colourway Series** (five colourways side-by-side on one shelf), with **Gallery III** automatically rendering any additional operator-created family. Cards align their internal rows across the grid with CSS `subgrid` (`row-span-5` / `row-span-4` + `grid-rows-subgrid`), while `@container` lives on the inner CTA/count footer — never on the card root, because inline-size containment disqualifies subgrid participation. The standalone `01–16` Collector's Edition Matrix and every competitor/showroom price comparison are permanently removed from public storefront surfaces (`DROP-01`, `TRUST-02`).
   - **Crash-Proof New Products**: Newly created SKUs with `0` assigned pieces or empty copy render cleanly as **Open Edition / 常設單品**. The seeded `drop.framing` default is also sanitized at render time: any showroom/retail/discount phrasing (`HK Showroom`, `Save %`, `15% under retail`, `陳列室`) is replaced by neutral copy, so CMS text cannot smuggle a competitor price comparison back onto the storefront (`TRUST-02`).
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
   - **1-Click Excel/CSV Bulk Sync (`DATA-01`, `ADMIN-01`)**: Export products / site copy as an Excel-safe CSV, edit it offline, review a dry-run diff, then commit atomically — see the dedicated section below.
   - **Concurrency, stale-tab & snapshot guards (`ADMIN-01`)**: Saves send `expected_version_hash` (a stale tab gets `409` instead of overwriting a newer save), the URL slug is locked behind `⛓ Unlock URL Slug` (renaming cascades onto historical order lines only as a deliberate act), `sessionStorage` drafts remember the version they were based on, and checkout/order pages render deactivated SKUs from the order's stored line-item snapshot (`model`, brand, photos, dimensions) instead of failing on a row that left the public catalogue.

## Mobile-First & Intrinsic Responsive Architecture (`320px → 1440px+`, 2026-10-24)

The storefront and Admin Studio are authored narrow-first and enhanced upward; layout geometry comes from intrinsic content sizing, never fixed pixel widths or faked heights.

- **2-tier mobile exhibition header + 3-zone desktop header**: Below `md` (`< 768px`) the single sticky header stacks **Row 1** — a protected, non-shrinking brand slot (`brand.logo_url` when configured, else the locale wordmark) plus compact utilities (atmosphere toggle, `EN | 中文`, cart with count badge) — and **Row 2** — `Collection | Provenance | Order Lookup` as a three-column, ≥36px touch bar with hairline dividers and locale-prefixed links. From `md` (`>= 768px`) the same markup becomes one row in a `md:grid-cols-[1fr_auto_1fr]` structural grid (brand · centre nav · utilities), so `EN ↔ 中文` label changes never shove the utility cluster. Primary navigation is never hidden on phones, and the slide-over cart respects `env(safe-area-inset-bottom)`.
- **`.intrinsic-stack` zero-shift bilingual / state labels**: The active and inactive labels share one CSS Grid cell (`[grid-area: 1/1]`); ghost labels are `visibility: hidden; block-size: 0; overflow: clip; pointer-events: none; user-select: none`. The browser sizes each control to the widest label intrinsically at any zoom or font scale, so language and atmosphere swaps produce zero horizontal jump — no `w-[...]` / `min-w-[...]` magic numbers.
- **Truthful `--header-height`**: Measured from the header's **inner content wrapper** (`getBoundingClientRect().height` + the header's own border chrome), coalesced to one measurement per animation frame, kept current by a `ResizeObserver` plus resize/orientation fallbacks and re-measured after `document.fonts.ready`. The outer `<header>` — which consumes `min-block-size: var(--header-height)` — is never observed, so there is no resize → measure → resize feedback loop. The property drives `scroll-padding-block-start`, the PDP sticky offset and the laptop first fold.
- **Viewport-bounded first fold**: On `lg:` the telemetry bar + Hero sit in `min-h`/`max-h-[calc(100dvh-var(--header-height))]` with fixed chrome `shrink-0` and a `flex-1 min-h-0` carousel stage, so the Hero copy, carousel card, price and CTAs are visible without scrolling on 16:9 (`1440×740`) and 16:10 (`1440×900`) laptops. Below `lg:` the Hero scrolls naturally — it is never squashed or clipped to fake a fit.
- **`subgrid` + `@container` components**: Lineup `<article>`s keep `row-span-N grid grid-rows-subgrid` for cross-card row alignment, while `container-type: inline-size` lives on the inner CTA/footer wrapper (and on the PDP purchase panel, cart/checkout line items and cart drawer) so stacked↔inline switching responds to the component's own inline width — 1-up phone → 2-up tablet → 3-up `lg:`. The two never share an element.
- **Touch & keyboard ergonomics**: Primary actions are `min-h-11` (44px), compact chrome (`.reel-*` indicators, pause/play and the mobile nav row) is at least 36px, every interactive element sets `touch-action: manipulation`, drawers/modals close on `Escape`, wide operator tables scroll inside their own track (`overscroll-behavior: contain`), and long `ORD-…` / FPS codes wrap instead of widening the page.
- **iOS Safari `16px` inputs**: Every storefront and Admin Studio `<input>` / `<select>` / `<textarea>` is `text-base sm:text-sm` — 16px on phones so Safari never auto-zooms a focused field, 14px from `sm` up.
- **Zero horizontal overflow, no masking**: No page-level `overflow-x: hidden/clip`; `scrollWidth === clientWidth` and no visible element crosses the viewport edge at 320, 375, 390, 430, 768, 1024, `1440×740` and `1440×900`.
- **React 19 hydration stability**: Exactly one owner per non-React root value (`StorefrontProvider` → `data-atmosphere`, `ExhibitionHeader` → `--header-height`), deterministic server/client `<html>` attributes (`lang`, `data-scroll-behavior`, scoped `suppressHydrationWarning`), and the dev annotation toolbar mounts only after `load` + an idle window so its `document.body` portal cannot interleave with hydration.

### Verification evidence (2026-10-24)

- `63/63` Node unit & contract tests in `web/test/storefront-and-admin.test.mjs`, `web/test/mobile-ergonomics.test.mjs` and `web/test/hydration-stability.test.mjs`.
- `285` Rust tests green against a scratch PostgreSQL instance (CI provisions the same suite).
- Real-browser Stage 5 archive `artifacts/ui-audit/20261024__session-01a16e95-e867-714a-b8f3-0295d7169adf/`: **25/25** `browse.exe` capture groups PASS, **234/234** DOM geometry assertions, 345 `.jpg` screenshots across 6 runs, zero console/page errors.

## 1-Click Excel/CSV Bulk Sync (`DATA-01`, `ADMIN-01`)

Operators can round-trip the whole catalogue and the site copy through Excel without a developer:

- **Endpoints**: `GET /api/admin/csv/products`, `POST /api/admin/csv/products/preview`, `POST /api/admin/csv/products/commit` (plus the `site-content` equivalents) live in `api/src/csv_sync.rs`, all behind `require_admin` + CSRF. The web tier proxies them at `/admin/api/csv/[...slug]` (`web/app/admin/api/csv/[...slug]/route.ts`), attaching the `admin_session` cookie, enforcing its own same-origin guard, forcing `X-Requested-With: XMLHttpRequest`, capping uploads at 8 MB and revalidating the storefront **only on `/commit`** — a preview never touches live pages.
- **Excel-safe export**: Every download starts with a UTF-8 BOM so Excel detects UTF-8 and Traditional Chinese copy stops mojibaking; line endings are CRLF (RFC 4180); piece numbers are written `#01;#02` because Excel would silently turn `01-02` into a date.
- **Dry-run → atomic commit**: `AdminCsvSyncClient` (mounted on `/admin/products` and `/admin/site-content`) uploads the edited file for a before→after diff with per-row validation errors and stock warnings, then commits in one click. Commit plans inside its transaction and writes products, content, pieces and one `csv_bulk_sync` audit row — all or nothing. Changing the file or any option invalidates the preview, so an unpreviewed spreadsheet can never be committed.
- **Concurrent-order stock protection**: The export carries `snapshot_stock_total` next to `stock_total`/`stock_delta`. A CSV that does not edit stock can never clobber a live count (the locked database value wins); when stock *was* edited, the locked count is compared with the snapshot and resolved by `stock_conflict_mode` — `apply_delta` (default: a sale made while you were in Excel is never rewound), `keep_db`, or `force_override`. `stock_total >= stock_reserved + stock_quarantined` is enforced (`STOCK-01`), scientific-notation cells are rejected, and slug renames require an explicit `allow_slug_rename`.

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
# Rust API (port 18080) — DB-backed suite; point it at a scratch Postgres
# (CI provisions postgres:17 on 55433 with these neutral credentials)
cd api && STOREFRONT_TEST_DATABASE_URL="postgres://ci_user:scratch-only@localhost:55433/storefront_scratch" cargo test  # 285 passed
# Next.js Web (port 3002) — storefront/admin contracts, mobile ergonomics, hydration stability
cd web && npm run lint && npm run typecheck && npm run build && npm test  # 63 passed
```

The web suite is `web/test/storefront-and-admin.test.mjs`, `web/test/mobile-ergonomics.test.mjs` and `web/test/hydration-stability.test.mjs`. Real-browser evidence (Stage 3 + Stage 5 `.jpg` archives and `INDEX.md` ledgers) lives under `artifacts/ui-audit/<YYYYMMDD>__session-<PI_SESSION_ID>/`.
