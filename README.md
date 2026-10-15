# Curated Architectural Lighting Flagship & Operator Studio (v2 Clean-Room)

Production-ready bilingual (`en` / `zh-HK`) ecommerce flagship and operator studio for curated designer lighting & homeware in Hong Kong.

## Key Architecture & Features

1. **Zero-Hardcode White-Label Engine (`NAME-01`, `ENV-01`, `DATA-02`, `CONFIG-01`)**:
   - Every company scalar (Brand Name, Native Chinese Wordmark, Canonical URL, Currency, WhatsApp Concierge, Analytics) is resolved dynamically from live Admin Settings overrides (`settings` table) -> Environment defaults (`.env`) -> safe code defaults.
   - All product SKUs, prices (integer minor units / cents), bilingual heritage & electrical copy, verified showroom reference prices, and numbered edition pieces (`01–16`) live in PostgreSQL and are managed through the Admin Studio with **0-second storefront propagation** (`revalidatePath("/", "layout")`).
2. **Awwwards / Webby Exhibition Storefront (`web/`)**:
   - **Synchronized Room Atmosphere (`Daylight 5500K` ↔ `Nocturne 2700K`)**: Toggle from the sticky header, hero, or `L` keyboard shortcut; cross-fades every lamp stage on the page with physically inspired 2700K radial light bloom, persisted across navigation.
   - **Interactive Collector's Edition `01–16` Matrix**: Live serialized edition strip with piece preview and real-time vault stock indicators.
   - **3-Mode Product Light Studio**: Switch between *Daylight 5500K Cutout*, *2700K Lit*, and *1:10 CAD Scale & Blueprint* (with a 220mm hardcover book reference outline), plus in-place **Sibling Colourway Swatches** and a **Visual Market Price Comparison Bar (`Save 15%–42%`)**.
   - **Crash-Proof New Products**: Newly created SKUs with `0` assigned pieces or empty copy render cleanly as **Open Edition / 常設單品**.
3. **End-to-End Shopping Bag, Checkout & Payment Settlement**:
   - Slide-over **Collector's Bag Drawer** with live vault stock caps and curated empty-bag discovery.
   - **Dedicated Checkout & Payment Settlement Page (`/[lang]/checkout/[code]`)** with live hold countdown timer, **Interactive Stripe Card Gateway** (with 1-click test card autofill `4242` and decline test `0002`), **HK FPS Faster Payment QR/ID & Reference Submission**, self-service hold cancellation, **Printable Official Collector's Pass**, and an on-screen **Dispatched Email Notification Preview**.
4. **Operator-First Admin Studio (`/admin`)**:
   - **Unified Single-Save Product Studio (`/admin/products/[id]`)** with real-time as-you-type 3-stage preview, **direct photo file upload**, **1-click Duplicate/Clone SKU**, human **Dollar** price inputs, side-by-side `EN | 繁體中文` copy editors, and **Quarantine Inspection** (*Pass Inspection -> Restock* or *Write Off Damaged*).

## Development & Verification

```bash
# Rust API (port 18080)
cd api && cargo test
# Next.js Web (port 3002)
cd web && npm run typecheck && npm run build && npm test
```
