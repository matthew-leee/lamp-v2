import { interpolateCopy } from "./format.ts";
import type { Locale } from "./i18n.ts";

export type ArtVariant = "dome" | "shades" | "shades-tall" | "piano" | "none";

export interface ProductLocaleCopy {
  model_alt?: string;
  colourway?: string;
  heritage?: string;
  electrical?: string;
}

export interface ReferencePrice {
  min_price_cents: number;
  max_price_cents: number;
  source_name: string;
  source_url: string;
  checked_on: string;
}

export interface CatalogueProduct {
  slug: string;
  brand: string;
  model: string;
  price_cents: number;
  family: string;
  active: boolean;
  art_variant: ArtVariant;
  art_hue: number;
  image_day_url: string;
  image_lit_url: string;
  designer: string;
  design_year: string;
  dimensions: string;
  light_source: string;
  content: Record<string, ProductLocaleCopy>;
  reference_price: ReferencePrice | null;
  pieces: number[];
  units_in_drop: number;
}

export interface CatalogueResponse {
  total_pieces: number;
  products: CatalogueProduct[];
}

export type SiteContentMap = Record<string, Record<string, string>>;
export type SettingsMap = Record<string, string>;

export interface LiveStockRow {
  id: number;
  slug: string;
  brand: string;
  model: string;
  price_cents: number;
  stock_total: number;
  stock_reserved: number;
  stock_quarantined: number;
  created_at: string;
}

export interface SiteConfig {
  brandName: string;
  brandNameNative: string | null;
  /** ENV-01: admin-editable company logo (`brand.logo_url`) for the header slot. */
  brandLogoUrl: string | null;
  siteUrl: string;
  whatsappNumber: string | null;
  currency: string;
  gaMeasurementId: string | null;
  gscVerification: string | null;
  fpsId: string | null;
  fpsAccountName: string | null;
  fpsQrUrl: string | null;
}

export function getApiBaseUrl(): string {
  const url = process.env.NEXT_PUBLIC_API_URL?.trim();
  if (!url) {
    return "http://127.0.0.1:18080";
  }
  return url.replace(/\/+$/, "");
}

function resolveLayered(
  overrides: SettingsMap,
  key: string,
  envVal: string | undefined,
): string | null {
  if (key in overrides) {
    const val = overrides[key]?.trim() ?? "";
    return val === "" ? null : val;
  }
  const trimmedEnv = envVal?.trim() ?? "";
  return trimmedEnv === "" ? null : trimmedEnv;
}

export function resolveSiteConfig(overrides: SettingsMap): SiteConfig {
  const brandName =
    resolveLayered(overrides, "brand.name", process.env.NEXT_PUBLIC_BRAND_NAME) ??
    process.env.NEXT_PUBLIC_BRAND_NAME?.trim() ??
    "";
  const rawNative = resolveLayered(
    overrides,
    "brand.name_native",
    process.env.NEXT_PUBLIC_BRAND_NAME_NATIVE,
  );
  const brandNameNative = rawNative && rawNative !== brandName ? rawNative : null;
  // ENV-01: the company logo is a swap-in slot, never a hardcoded asset. An
  // EMPTY override means "no logo, render the wordmark" — same presence-wins
  // semantics as every other public setting.
  const brandLogoUrl = resolveLayered(
    overrides,
    "brand.logo_url",
    process.env.NEXT_PUBLIC_BRAND_LOGO_URL,
  );
  const siteUrl = (
    resolveLayered(overrides, "site.url", process.env.NEXT_PUBLIC_SITE_URL) ??
    process.env.NEXT_PUBLIC_SITE_URL?.trim() ??
    "http://localhost:3002"
  ).replace(/\/+$/, "");
  const whatsappNumber = resolveLayered(
    overrides,
    "contact.whatsapp",
    process.env.NEXT_PUBLIC_WHATSAPP_NUMBER,
  );
  const currency =
    resolveLayered(overrides, "currency", process.env.NEXT_PUBLIC_CURRENCY) ?? "HKD";
  const rawGa = resolveLayered(
    overrides,
    "analytics.ga_measurement_id",
    process.env.NEXT_PUBLIC_GA_MEASUREMENT_ID,
  );
  const gaMeasurementId = rawGa && /^G-[A-Z0-9]{4,12}$/.test(rawGa) ? rawGa : null;
  const gscVerification = resolveLayered(
    overrides,
    "analytics.gsc_verification",
    process.env.NEXT_PUBLIC_GSC_VERIFICATION,
  );
  const fpsId = resolveLayered(
    overrides,
    "payment.fps_id",
    process.env.NEXT_PUBLIC_FPS_ID,
  );
  const fpsAccountName = resolveLayered(
    overrides,
    "payment.fps_account_name",
    process.env.NEXT_PUBLIC_FPS_ACCOUNT_NAME,
  );
  const fpsQrUrl = resolveLayered(
    overrides,
    "payment.fps_qr_url",
    process.env.NEXT_PUBLIC_FPS_QR_URL,
  );

  return {
    brandName,
    brandNameNative,
    brandLogoUrl,
    siteUrl,
    whatsappNumber,
    currency,
    gaMeasurementId,
    gscVerification,
    fpsId,
    fpsAccountName,
    fpsQrUrl,
  };
}

async function fetchJsonNoStore<T>(path: string): Promise<T> {
  const base = getApiBaseUrl();
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 8000);
  try {
    const res = await fetch(`${base}${path}`, {
      cache: "no-store",
      signal: controller.signal,
    });
    if (!res.ok) {
      throw new Error(`API GET ${path} returned HTTP ${res.status}`);
    }
    return (await res.json()) as T;
  } finally {
    clearTimeout(timer);
  }
}

export async function loadStorefrontData(): Promise<{
  catalogue: CatalogueResponse;
  siteContent: SiteContentMap;
  settings: SettingsMap;
  config: SiteConfig;
}> {
  // Resilient secondary-data loading (G23): the catalogue is required, but a
  // failure of the content or settings channels must degrade to empty
  // overrides and fall back to env defaults — never crash the page.
  const [catalogue, siteContentWrap, settings] = await Promise.all([
    fetchJsonNoStore<CatalogueResponse>("/api/catalogue"),
    fetchJsonNoStore<{ content: SiteContentMap }>("/api/site-content").catch(
      () => ({ content: {} as SiteContentMap }),
    ),
    fetchJsonNoStore<SettingsMap>("/api/settings").catch(() => ({}) as SettingsMap),
  ]);
  const siteContent = siteContentWrap?.content ?? {};
  const config = resolveSiteConfig(settings ?? {});
  return { catalogue, siteContent, settings: settings ?? {}, config };
}

export async function fetchLiveStockMap(): Promise<Record<string, {
  available: number;
  total: number;
  reserved: number;
  quarantined: number;
}> | null> {
  try {
    const rows = await fetchJsonNoStore<LiveStockRow[]>("/api/products");
    if (!Array.isArray(rows)) return null;
    const out: Record<string, {
      available: number;
      total: number;
      reserved: number;
      quarantined: number;
    }> = {};
    for (const r of rows) {
      const q = typeof r.stock_quarantined === "number" ? r.stock_quarantined : 0;
      const available = Math.max(0, r.stock_total - r.stock_reserved - q);
      out[r.slug] = {
        available,
        total: r.stock_total,
        reserved: r.stock_reserved,
        quarantined: q,
      };
    }
    return out;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Enriched public order line items (DATA-01 / CART-01)
//
// `GET /api/orders/{order_code}` returns a metadata SNAPSHOT per line item so a
// SKU that was deactivated (`active = false`) after the sale still renders its
// real title, photograph and CAD stage on the checkout / Collector's Pass page
// — the public catalogue no longer contains it, the order still describes it.
// ---------------------------------------------------------------------------

export interface PublicOrderItem {
  product_id: number;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  line_total_cents: number;
  model?: string;
  brand?: string;
  image_day_url?: string;
  image_lit_url?: string;
  art_variant?: string;
  art_hue?: number;
  dimensions?: string;
}

const ART_VARIANTS: readonly ArtVariant[] = [
  "dome",
  "shades",
  "shades-tall",
  "piano",
  "none",
];

/** Narrows an arbitrary string (API snapshot) to a renderable {@link ArtVariant}. */
export function coerceArtVariant(
  value: unknown,
  fallback: ArtVariant = "dome",
): ArtVariant {
  return typeof value === "string" &&
    (ART_VARIANTS as readonly string[]).includes(value)
    ? (value as ArtVariant)
    : fallback;
}

/**
 * Rebuilds a render-able product from an enriched order line item.
 *
 * Used as the FALLBACK when the ordered SKU has been deactivated and therefore
 * disappeared from `catalogue.products` — the checkout page must never degrade
 * to a bare slug or a blank photo. Returns `null` when the API snapshot carries
 * no identity at all (older orders written before item enrichment), so callers
 * keep their existing slug fallback.
 */
export function catalogueProductFromOrderItem(
  item: PublicOrderItem | null | undefined,
): CatalogueProduct | null {
  if (!item) return null;
  const model = (item.model ?? "").trim();
  const brand = (item.brand ?? "").trim();
  if (model === "" && brand === "") return null;
  const hue =
    typeof item.art_hue === "number" && Number.isFinite(item.art_hue)
      ? item.art_hue
      : 38;
  return {
    slug: item.product_slug,
    brand,
    model: model === "" ? brand : model,
    price_cents: item.unit_price_cents,
    family: "",
    // The catalogue only lists active SKUs, so a reconstructed snapshot is
    // never presented as a buyable catalogue entry.
    active: false,
    art_variant: coerceArtVariant(item.art_variant),
    art_hue: hue,
    image_day_url: item.image_day_url ?? "",
    image_lit_url: item.image_lit_url ?? "",
    designer: "",
    design_year: "",
    dimensions: item.dimensions ?? "",
    light_source: "",
    content: {},
    reference_price: null,
    pieces: [],
    units_in_drop: 0,
  };
}

// ---------------------------------------------------------------------------
// 1-Click Excel/CSV Bulk Sync contract (DATA-01 / ADMIN-01)
//
// The browser never talks to the Rust API directly: it posts the spreadsheet
// to the Next.js proxy (`/admin/api/csv/{kind}[/preview|/commit]`), which adds
// the `admin_session` cookie and forwards this JSON body to
// `/api/admin/csv/{kind}[/preview|/commit]`:
//
//   { "csv": "<file text>", "apply_stock": bool,
//     "stock_conflict_mode": "keep_db" | "apply_delta" | "force_override",
//     "allow_slug_rename": bool }
//
// and receives back:
//
//   { "ok": bool, "dry_run": bool,
//     "changes": [{ "row", "slug", "field", "before", "after", "action" }],
//     "errors":  [{ "row", "column", "message" }],
//     "summary": { "created", "updated", "unchanged", "errors" } }
//
// The helpers below are pure so the proxy route and the diffs table agree on
// exactly one parsing/validation contract, and both stay unit-testable.
// ---------------------------------------------------------------------------

export type CsvSyncKind = "products" | "site-content";
export type CsvSyncPhase = "export" | "preview" | "commit";

/**
 * How an edited `stock_total` cell interacts with LIVE stock (the API accepts
 * the aliases `snapshot_aware` / `overwrite` / `skip` for these three):
 *
 *  - `keep_db`       — never write stock (text & price-only import).
 *  - `apply_delta`   — add the CSV delta to the LIVE count, so a concurrent
 *                      sale between export and import is never rewound.
 *  - `force_override`— force the CSV total onto live stock (an explicit
 *                      "this number is the truth" override).
 *
 * The UI defaults to `apply_delta`: it is the only mode that both honours the
 * operator's added stock and protects a sale that happened mid-edit.
 */
export const CSV_STOCK_CONFLICT_MODES = [
  "keep_db",
  "apply_delta",
  "force_override",
] as const;

export type CsvStockConflictMode = (typeof CSV_STOCK_CONFLICT_MODES)[number];

export const DEFAULT_CSV_STOCK_CONFLICT_MODE: CsvStockConflictMode =
  "apply_delta";

/** Hard ceiling for an uploaded spreadsheet (mirrors the upload guard). */
export const CSV_SYNC_MAX_BYTES = 8 * 1024 * 1024;

export interface CsvSyncOptions {
  applyStock: boolean;
  stockConflictMode: CsvStockConflictMode;
  allowSlugRename: boolean;
}

export interface CsvSyncChange {
  /** Physical spreadsheet line number (header is line 1). */
  row: string;
  /** Product slug, or the site-copy key for a site-content import. */
  slug: string;
  field: string;
  before: string;
  after: string;
  action: string;
}

export interface CsvSyncIssue {
  /** Physical spreadsheet line number, or the offending identifier. */
  row: string;
  column: string;
  message: string;
}

export interface CsvSyncReport {
  ok: boolean;
  dryRun: boolean;
  changes: CsvSyncChange[];
  errors: CsvSyncIssue[];
  summary: { created: number; updated: number; unchanged: number; errors: number };
}

/** Rust API path for one CSV phase (the proxy adds the `/api` host itself). */
export function csvApiPath(
  kind: CsvSyncKind,
  phase: CsvSyncPhase,
): string {
  const base = `/api/admin/csv/${kind}`;
  return phase === "export" ? base : `${base}/${phase}`;
}

/** Drops a UTF-8 BOM so the importer never sees `\uFEFFslug` as the first header. */
export function stripUtf8Bom(text: string): string {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

type CsvOptionSource =
  | { get(name: string): unknown }
  | null
  | undefined;

function readCsvFlag(source: CsvOptionSource, name: string): boolean {
  const raw = source?.get(name);
  if (typeof raw === "boolean") return raw;
  if (typeof raw === "string") {
    return ["1", "true", "on", "yes"].includes(raw.trim().toLowerCase());
  }
  return false;
}

/**
 * Reads the import options from a `FormData` upload or a JSON body.
 * Unknown/absent values fall back to the SAFE defaults: stock is never applied
 * and a slug rename is never allowed unless the operator opted in explicitly.
 */
export function parseCsvSyncOptions(source: CsvOptionSource): CsvSyncOptions {
  const rawMode = source?.get("stock_conflict_mode");
  const mode =
    typeof rawMode === "string" &&
    (CSV_STOCK_CONFLICT_MODES as readonly string[]).includes(rawMode.trim())
      ? (rawMode.trim() as CsvStockConflictMode)
      : DEFAULT_CSV_STOCK_CONFLICT_MODE;
  return {
    applyStock: readCsvFlag(source, "apply_stock"),
    stockConflictMode: mode,
    allowSlugRename: readCsvFlag(source, "allow_slug_rename"),
  };
}

export function buildCsvSyncPayload(
  csvText: string,
  options: CsvSyncOptions,
): Record<string, unknown> {
  return {
    csv: stripUtf8Bom(csvText),
    apply_stock: options.applyStock,
    stock_conflict_mode: options.stockConflictMode,
    allow_slug_rename: options.allowSlugRename,
  };
}

function readString(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  if (typeof value === "boolean") return String(value);
  return "";
}

function firstString(record: Record<string, unknown>, keys: string[]): string {
  for (const key of keys) {
    const value = readString(record[key]).trim();
    if (value !== "") return value;
  }
  return "";
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/**
 * Normalizes a preview/commit response into the diff model the table renders.
 * Tolerant of the aliases the API may use (`rows`/`diffs`, `old`/`from`, …) so
 * a naming tweak on the Rust side cannot blank the operator's diff — but every
 * field it invents is derived only from response data, never from the CSV.
 */
export function normalizeCsvSyncReport(raw: unknown): CsvSyncReport {
  const body = asRecord(raw) ?? {};
  const changeRows =
    asArray(body.changes).length > 0
      ? asArray(body.changes)
      : asArray(body.rows).length > 0
        ? asArray(body.rows)
        : asArray(body.diffs);

  const changes: CsvSyncChange[] = [];
  for (const entry of changeRows) {
    const record = asRecord(entry);
    if (!record) continue;
    const before = firstString(record, ["before", "before_value", "old", "from"]);
    const after = firstString(record, ["after", "after_value", "new", "to"]);
    const action =
      firstString(record, ["action", "change", "op"]) ||
      (before === "" ? "create" : before === after ? "unchanged" : "update");
    const row = firstString(record, ["row", "line"]);
    const slug = firstString(record, [
      "slug",
      "key",
      "product_slug",
      "product",
      "id",
    ]);
    changes.push({
      row,
      // The identifier is what the operator recognises; fall back to the line.
      slug: slug === "" ? row : slug,
      field: firstString(record, ["field", "column", "name"]),
      before,
      after,
      action,
    });
  }

  const errors: CsvSyncIssue[] = [];
  for (const entry of asArray(body.errors)) {
    if (typeof entry === "string") {
      errors.push({ row: "", column: "", message: entry.trim() });
      continue;
    }
    const record = asRecord(entry);
    if (!record) continue;
    const message = firstString(record, ["message", "error", "detail"]);
    if (message === "") continue;
    errors.push({
      row: firstString(record, ["row", "line", "slug", "key"]),
      column: firstString(record, ["column", "field"]),
      message,
    });
  }

  const rawSummary = asRecord(body.summary);
  const countFrom = (key: string): number => {
    const value = rawSummary?.[key];
    return typeof value === "number" && Number.isFinite(value) && value >= 0
      ? value
      : 0;
  };
  const computed = (action: string) =>
    changes.filter((change) => change.action === action).length;
  const providedSummaryClaims =
    rawSummary !== null &&
    ["created", "updated", "unchanged"].some(
      (key) => typeof rawSummary[key] === "number",
    );
  const summary = providedSummaryClaims
    ? {
        created: countFrom("created"),
        updated: countFrom("updated"),
        unchanged: countFrom("unchanged"),
        errors: countFrom("errors") || errors.length,
      }
    : {
        created: computed("create"),
        updated: computed("update"),
        unchanged: computed("unchanged"),
        errors: errors.length,
      };

  return {
    // An explicit `ok: false` (e.g. dry-run rejected) wins; otherwise a report
    // is only "ok" when the API said so and no row/parse error was reported.
    ok: body.ok === true && errors.length === 0,
    dryRun: body.dry_run === true || body.dryRun === true,
    changes,
    errors,
    summary,
  };
}

export function readSiteCopy(
  siteContent: SiteContentMap,
  key: string,
  locale: Locale,
  vars: { brand: string; pieces?: number | string; subject?: string },
  fallback = "",
): string {
  // LANG-01: only read the active locale's copy; if absent, use the
  // locale-matched fallback supplied by the caller (falling back to English
  // only when the caller provided no localized fallback).
  const raw =
    siteContent[key]?.[locale] ||
    fallback ||
    (locale === "en" ? siteContent[key]?.en : "") ||
    "";
  if (!raw) return fallback;
  return interpolateCopy(raw, vars);
}
