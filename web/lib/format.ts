import { isCjk, t, ui, type Locale } from "./i18n";

const CURRENCY_PREFIX: Record<string, string> = {
  HKD: "HK$",
  USD: "$",
  EUR: "€",
  GBP: "£",
  JPY: "¥",
  CNY: "¥",
  TWD: "NT$",
  SGD: "S$",
};

/**
 * Formats integer minor units (cents) into a clean display price.
 * Never uses floating-point division for currency math.
 */
export function formatMoney(cents: number, currency = "HKD"): string {
  const safeCents = Number.isFinite(cents) ? Math.max(0, Math.trunc(cents)) : 0;
  const code = currency.trim().toUpperCase() || "HKD";
  const prefix = CURRENCY_PREFIX[code] ?? `${code} `;
  const whole = Math.floor(safeCents / 100);
  const minor = safeCents % 100;
  const formattedWhole = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  if (minor === 0) {
    return `${prefix}${formattedWhole}`;
  }
  return `${prefix}${formattedWhole}.${String(minor).padStart(2, "0")}`;
}

/**
 * Converts a human dollar string (e.g. "1615" or "1615.50") to integer cents.
 * Returns null if the input is not a valid non-negative amount.
 */
export function parseDollarsToCents(input: string): number | null {
  const cleaned = input.replace(/[,\s$HKD]/gi, "").trim();
  if (!/^\d+(\.\d{1,2})?$/.test(cleaned)) return null;
  const [wholePart = "0", fracPart = ""] = cleaned.split(".");
  const whole = Number.parseInt(wholePart, 10);
  const frac = Number.parseInt(fracPart.padEnd(2, "0"), 10);
  if (!Number.isSafeInteger(whole) || !Number.isSafeInteger(frac)) return null;
  return whole * 100 + frac;
}

/**
 * Formats integer cents as a clean human dollar input string (e.g. 161500 -> "1615").
 */
export function formatCentsToDollarsInput(cents: number): string {
  const safe = Number.isFinite(cents) ? Math.max(0, Math.trunc(cents)) : 0;
  const whole = Math.floor(safe / 100);
  const minor = safe % 100;
  return minor === 0 ? String(whole) : `${whole}.${String(minor).padStart(2, "0")}`;
}

const pad2 = (n: number): string => String(n).padStart(2, "0");

/**
 * Crash-proof edition numbering formatter.
 * When a newly created product has `pieces: []`, renders "Open Edition" / "常設單品"
 * instead of throwing a RangeError.
 */
export function formatEditionBadge(
  pieces: readonly number[],
  totalPieces: number,
  locale: Locale,
): string {
  if (!pieces || pieces.length === 0) {
    return t(ui.openEdition, locale);
  }
  const sorted = [...pieces].sort((a, b) => a - b);
  const first = sorted[0]!;
  const last = sorted[sorted.length - 1]!;
  const contiguous = sorted.every((n, i) => i === 0 || n === sorted[i - 1]! + 1);
  const rangeStr = contiguous
    ? first === last
      ? pad2(first)
      : `${pad2(first)}–${pad2(last)}`
    : sorted.map(pad2).join(", ");
  const total = Math.max(totalPieces, last);
  if (isCjk(locale)) {
    return `編號 ${rangeStr} / 共 ${total} 件`;
  }
  return first === last
    ? `Piece ${rangeStr} of ${total}`
    : `Pieces ${rangeStr} of ${total}`;
}

/**
 * Computes percentage saved vs. verified reference price range.
 */
export function computeSavingsPercent(
  priceCents: number,
  minRefCents: number,
  maxRefCents: number,
): { minPct: number; maxPct: number } | null {
  if (priceCents <= 0 || minRefCents <= priceCents) return null;
  const minPct = Math.round(((minRefCents - priceCents) / minRefCents) * 100);
  const maxPct = Math.round(((maxRefCents - priceCents) / maxRefCents) * 100);
  return { minPct: Math.max(1, minPct), maxPct: Math.max(minPct, maxPct) };
}

/**
 * Formats product display title in the chosen locale.
 */
export function formatProductTitle(
  model: string,
  modelAlt: string | undefined,
  colourway: string | undefined,
  locale: Locale,
): string {
  const base = isCjk(locale) && modelAlt ? modelAlt : model;
  if (!colourway || colourway.trim() === "") return base;
  return isCjk(locale) ? `${base}（${colourway}）` : `${base} (${colourway})`;
}

/**
 * Interpolates {brand}, {pieces}, and {subject} placeholders in site copy.
 */
export function interpolateCopy(
  template: string,
  vars: { brand: string; pieces?: number | string; subject?: string },
): string {
  let out = template.split("{brand}").join(vars.brand);
  if (vars.pieces !== undefined) {
    out = out.split("{pieces}").join(String(vars.pieces));
  }
  if (vars.subject !== undefined) {
    out = out.split("{subject}").join(vars.subject);
  }
  return out;
}

/**
 * Builds a validated WhatsApp wa.me link, or null if no valid number is configured.
 */
export function buildWhatsAppUrl(
  rawNumber: string | null | undefined,
  message: string,
): string | null {
  if (!rawNumber) return null;
  const digits = rawNumber.replace(/\D/g, "");
  if (digits.length < 7 || digits.length > 15) return null;
  return `https://wa.me/${digits}?text=${encodeURIComponent(message)}`;
}
