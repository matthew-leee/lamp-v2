# Changelog

## [Unreleased] — v2 Clean-Room Flagship & Operator Studio

### Fixed
- **Stripe Dynamic Payment Methods Compatibility (`STRIPE-01`)**: Removed the deprecated `payment_method_types[0]=card` parameter from `POST /v1/checkout/sessions` in `web/app/api/orders/[code]/pay-card/route.ts` so Stripe accounts on newer API versions using dashboard-managed Dynamic Payment Methods create hosted Checkout Sessions (`cs_test_...`) without error.

### Added & Updated (P1–P4 Milestones)
- **Multi-SKU Shopping Cart (`CART-01`)**:
  - Renamed all storefront terminology from "Shopping Bag" to **Shopping Cart / Cart (`購物車` / `加入購物車`)**.
  - Added migration `0007_multi_sku_fps_media.sql` creating `order_items` (`order_id`, `product_id`, `product_slug`, `quantity`, `unit_price_cents`, `line_total_cents`).
  - Updated `POST /api/orders/reserve`, `GET /api/orders/{code}`, `POST /api/webhooks/stripe`, `confirm_fps`, `release`, `cancel`, `quarantine_return`, and `expire_stale_reservations` to atomically reserve, settle, expire, release, and quarantine **1 to all SKUs in the store** in a single transaction with sorted row locks.
  - Updated Shopping Cart drawer, Checkout Settlement page, Collector's Pass, Email Receipts, and Admin Orders console to display all line items.
- **Configurable HK FPS Settings (`FPS-01`)**:
  - Added `payment.fps_id`, `payment.fps_account_name`, and `payment.fps_qr_url` to `PUBLIC_SETTING_KEYS` and `/admin/settings` with 0-second live propagation to the FPS checkout screen and zero hardcoded FPS literals in frontend code.
- **Real Stripe Sandbox/Live Checkout & Live Transactional Emails (`STRIPE-01`, `EMAIL-01`)**:
  - Updated `POST /api/orders/[code]/pay-card` to support creating real Stripe Checkout Sessions across all cart line items when `STRIPE_SECRET_KEY` is set, verifying return `session_id`s, and proxying signed webhooks at `POST /api/webhooks/stripe`, while retaining local test card fallback when keys are unset.
  - Added `dispatchTransactionalEmail` in `web/lib/email-receipt.ts` supporting live HTML email delivery via `RESEND_API_KEY` or `SMTP_HOST`/`SMTP_USER`/`SMTP_PASS` alongside the on-screen Email Preview modal.
- **Self-Sufficient Product Photo & Lighting Modes (`MEDIA-01`)**:
  - Cached all 8 opening-drop demo images locally under `web/public/demo-assets/` and updated product URLs to `/demo-assets/*`.
  - Updated `LampStage` and `ProductStoryClient` so single-photo SKUs (`image_day_url` set, `image_lit_url` blank) render as clean plain images without artificial warm tinting or redundant 2700K tabs, while dual-photo SKUs retain their 5500K ↔ 2700K cross-fade and radial bloom.
  - Added `art_variant = "none"` to disable the 1:10 CAD schematic tab for plain-photo or non-lamp items.
- **De-branded Test & DB Identifiers (`NAME-01`)**:
  - Replaced all remaining stale working-name identifiers in Rust tests and CI workflows with neutral `STOREFRONT_TEST_DATABASE_URL` / `storefront_scratch` / `ATELIER` naming.
- **5-Stage Real-Browser UI/UX Verification (`QA-01`)**:
  - Executed and archived 29 Desktop (`1440x900`) and Mobile (`390x844`) `.jpg` screenshots and `INDEX.md` in `artifacts/ui-audit/20261016__session-01a143c3-57df-76fa-8e6f-8dd98a6389f2/`.
- **Finland Hetzner CX33 Test Deployment (`DEPLOY-01`, `OPS-01`)**:
  - Deployed isolated Docker Compose stack (`db`, `api`, `web`, `caddy`, `cloudflared`) to `/opt/storefront-v2` on `89.167.15.159`, bound to the public eth0 IP (`BIND_IP=89.167.15.159`) so existing host services are undisturbed, exposed via both `https://89-167-15-159.sslip.io` (Let's Encrypt TLS) and Cloudflare Tunnel (`*.trycloudflare.com`), and verified live Postgres backup + scratch restore.

### Earlier v2 Clean-Room Baseline
- **Clean-Room Guardrail & Architecture**: Wiped legacy `web/` from disk and Git history on orphan branch `v2-clean-room`, locked predecessor `webpage/web` via OS ACLs, and documented the pure backend contract in `API_CONTRACT.md`.
- **Storefront Exhibition UI/UX**:
  - Sticky glassmorphic header with live Brand Wordmark, `Daylight 5500K` ↔ `Nocturne 2700K` room dimmer (`L` shortcut), route-preserving `EN / 繁體中文` switcher, and `Bag (N)` trigger.
  - Interactive `01–16` Collector's Edition Matrix, Family filter pills, and dual *Exhibition Grid* / *Archival Index* views.
  - Product Story Page with sticky 3-mode Light Studio (*Daylight Cutout*, *2700K Lit*, *1:10 CAD Scale Blueprint*), Sibling Colourway Swatches, and Visual Market Price Comparison Bar.
- **Money Path, Payment Settlement & Email Receipts**:
  - Slide-over Collector's Bag Drawer + Express Buy Now checkout.
  - `/[lang]/checkout/[code]` settlement screen with live countdown timer, interactive Stripe Card Gateway (`POST /api/orders/[code]/pay-card` signing verified HMAC webhooks to transition `reserved -> paid`), HK FPS transfer reference submission (`POST /api/orders/{code}/submit-fps`), self-service hold cancellation (`POST /api/orders/{code}/cancel`), Printable Collector's Pass, and Sent Email Notification viewer.
  - Public Collector Self-Service Order Lookup (`/[lang]/orders`).
- **Operator-First Admin Studio**:
  - 0-second live storefront propagation (`revalidatePath("/", "layout")`) on every Admin mutation and crash-proof rendering for newly created SKUs with 0 pieces (`Open Edition`).
  - Unified Product Studio (`/admin/products/[id]`) with real-time stage preview, direct photo upload (`/admin/api/upload`), 1-click SKU cloning (`POST /api/admin/products/{id}/clone`), human Dollar inputs, and Quarantine inspection (`POST /api/admin/products/{id}/inspect-quarantine`).
- **5-Stage Real-Browser UI/UX Verification**:
  - Captured and inspected 22 timestamped Desktop (`1440x900`) and Mobile (`390x844`) `.jpg` screenshots in `artifacts/ui-audit/20261015__session-01a1399a-1232-75ba-a543-4a3fb52f25b3/` with `INDEX.md`.
