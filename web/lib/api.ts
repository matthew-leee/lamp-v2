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
