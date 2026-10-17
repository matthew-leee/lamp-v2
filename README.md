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
4. **Operator-First Admin Studio (`/admin`)**:
   - **Unified Single-Save Product Studio (`/admin/products/[id]`)** with real-time as-you-type stage preview, **direct photo file upload**, **1-click Duplicate/Clone SKU**, human **Dollar** price inputs, `art_variant` (`dome | shades | shades-tall | piano | none`), side-by-side `EN | 繁體中文` copy editors, and **Quarantine Inspection** (*Pass Inspection -> Restock* or *Write Off Damaged*).
   - **Multi-SKU Orders & Quarantine Console (`/admin/orders`)** and **Live Company & FPS Settings (`/admin/settings`)**.

## Deployment (Finland Hetzner CX33 + Cloudflare Tunnel & Auto-HTTPS)

The full Docker Compose stack (`db`, `api`, `web`, `caddy`, `cloudflared`) runs in isolation on the Hetzner CX33 (`89.167.15.159` under `/opt/storefront-v2`) without touching any existing host services or Tailscale listeners (`BIND_IP=89.167.15.159`):
- **Automatic Let's Encrypt HTTPS (`sslip.io`)**: `https://89-167-15-159.sslip.io`
- **Cloudflare Tunnel (`cloudflared`)**: forwards `http://caddy:8080` to a public `*.trycloudflare.com` HTTPS endpoint (compatible with NordVPN without needing Tailscale).

### One-Line VPS & Git Auto-Deploy CLI (`scripts/vps.mjs`)

`scripts/vps.mjs` reads `HETZNER_VPS_PASSPHRASE` from `.env` automatically and provides a single-command workflow for Git-backed auto-deploy, remote log inspection, and database resets:

```bash
# Push current Git HEAD to /opt/storefront-v2.git on VPS & auto-rebuild via post-receive hook
node scripts/vps.mjs deploy

# Inspect container status, health, and active Cloudflare Tunnel URL
node scripts/vps.mjs status

# Tail live container logs (e.g. web, api, [email-receipt]) without manual SSH
node scripts/vps.mjs logs web

# Reset VPS database to the pristine 8 SKUs / 16 units / 0 orders baseline
node scripts/vps.mjs reset-db
```

## Development & Verification

```bash
# Rust API (port 18080)
cd api && STOREFRONT_TEST_DATABASE_URL="postgres://storefront:scratch-only@localhost:55433/storefront_scratch" cargo test
# Next.js Web (port 3002)
cd web && npm run lint && npm run typecheck && npm run build && npm test
```
