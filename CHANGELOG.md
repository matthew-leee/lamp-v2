# Changelog

## [Unreleased] — v2 Clean-Room Flagship & Operator Studio

### Added
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
