"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useRef } from "react";
import { useStorefront } from "./storefront-context";
import {
  htmlLang,
  isLocale,
  languageEndonyms,
  locales,
  t,
  ui,
  type BilingualText,
  type Locale,
} from "@/lib/i18n";

interface ExhibitionHeaderProps {
  lang: Locale;
  brandName: string;
  brandNameNative: string | null;
  brandLogoUrl?: string | null;
}

/**
 * NAME-01 / ENV-01 presentational fallback.
 *
 * The brand name is strictly env/settings-supplied, so a fresh database (or a
 * deploy that never configured `brand.name` / `brand.name_native`) can hand us
 * two empty strings. Rendering those would collapse the wordmark to a 0px box
 * and leave a blank, suspicious-looking header. This is presentation-only: the
 * configured brand always wins, and these strings are the exhibition label —
 * never a hardcoded brand identity. Mirrors `ui.defaultExhibitionTitle`.
 */
const FALLBACK_BRAND_WORDMARK: BilingualText = {
  en: "LIGHTING EXHIBITION",
  "zh-HK": "建築燈飾典藏展",
};

/**
 * Resolves the locale-appropriate wordmark (and the ghost label that reserves
 * the other locale's intrinsic width) with the presentational fallback applied.
 */
function resolveBrandWordmarks(
  brandName: string,
  brandNameNative: string | null,
  lang: Locale,
): { displayBrand: string; alternateBrand: string } {
  const primary = (brandName ?? "").trim();
  const native = (brandNameNative ?? "").trim();

  if (primary === "" && native === "") {
    return {
      displayBrand: FALLBACK_BRAND_WORDMARK[lang],
      // Reserve the other locale's intrinsic width too, so the `.intrinsic-stack`
      // cell keeps a stable width across `/en` <-> `/zh-HK` for the fallback.
      alternateBrand:
        FALLBACK_BRAND_WORDMARK[lang === "en" ? "zh-HK" : "en"],
    };
  }

  const displayBrand = lang === "zh-HK" ? native || primary : primary || native;
  const candidateAlternate = lang === "zh-HK" ? primary : native;
  return {
    displayBrand,
    alternateBrand:
      candidateAlternate === displayBrand ? "" : candidateAlternate,
  };
}

/**
 * Renders the active locale label while reserving the intrinsic width of both
 * locales (`en` and `zh-HK`) in the same CSS Grid cell (`[grid-area:1/1]`).
 * Switching between `/en` and `/zh-HK` produces zero horizontal jump at any
 * font size or zoom level without hardcoded pixel/rem widths.
 */
function BilingualIntrinsicLabel({
  text,
  lang,
}: {
  text: BilingualText;
  lang: Locale;
}) {
  const otherLocale: Locale = lang === "en" ? "zh-HK" : "en";
  return (
    <span className="intrinsic-stack">
      <span>{t(text, lang)}</span>
      <span aria-hidden="true" className="intrinsic-ghost">
        {t(text, otherLocale)}
      </span>
    </span>
  );
}

export default function ExhibitionHeader({
  lang,
  brandName,
  brandNameNative,
  brandLogoUrl,
}: ExhibitionHeaderProps) {
  const { lit, toggleLit, bag, setBagOpen } = useStorefront();
  const pathname = usePathname() ?? `/${lang}`;

  const headerRef = useRef<HTMLElement | null>(null);
  const headerInnerRef = useRef<HTMLDivElement | null>(null);

  // Route-preserving language switch path
  const segments = pathname.split("/").filter(Boolean);
  const subSegments =
    segments.length > 0 && isLocale(segments[0]!) ? segments.slice(1) : segments;
  const suffix = subSegments.length > 0 ? `/${subSegments.join("/")}` : "";

  const { displayBrand, alternateBrand } = resolveBrandWordmarks(
    brandName,
    brandNameNative,
    lang,
  );
  const logoUrl = typeof brandLogoUrl === "string" ? brandLogoUrl.trim() : "";

  const totalBagUnits = bag.reduce((sum, item) => sum + item.quantity, 0);
  const activeAtmosphereLabel = lit
    ? t(ui.atmosphereNight, lang)
    : t(ui.atmosphereDay, lang);
  const compactAtmosphereLabel = lit
    ? t(ui.atmosphereNightShort, lang)
    : t(ui.atmosphereDayShort, lang);
  const allAtmosphereLabels = [
    ui.atmosphereDay.en,
    ui.atmosphereDay["zh-HK"],
    ui.atmosphereNight.en,
    ui.atmosphereNight["zh-HK"],
  ];

  /**
   * Truthful `--header-height` synchronisation.
   *
   * The header is two-tiered on phones and single-row on desktop, so any static
   * guess is wrong for one of them. `--header-height` drives anchor offsets
   * (`#collection`, `#provenance`), the PDP sticky top and the homepage
   * first-fold height, so it must equal the real rendered height at all times.
   *
   * The measured element is the *inner* wrapper, never the `<header>` box: the
   * header carries `min-block-size: var(--header-height)` (glass-header), so
   * measuring the header itself would feed an inflated value back into the
   * observer and pin it at the tallest tier combination forever.
   */
  useEffect(() => {
    const header = headerRef.current;
    const inner = headerInnerRef.current;
    if (!header || !inner) return;

    let frame: number | null = null;

    const apply = () => {
      frame = null;
      const styles = window.getComputedStyle(header);
      const top = Number.parseFloat(styles.borderTopWidth);
      const bottom = Number.parseFloat(styles.borderBottomWidth);
      const chrome =
        (Number.isFinite(top) ? top : 0) + (Number.isFinite(bottom) ? bottom : 0);
      const measured = Math.round(inner.getBoundingClientRect().height + chrome);
      if (measured > 0) {
        document.documentElement.style.setProperty(
          "--header-height",
          `${measured}px`,
        );
      }
    };

    // Coalesce bursts of notifications into one measurement per frame.
    const schedule = () => {
      if (frame === null) frame = window.requestAnimationFrame(apply);
    };

    apply();

    let observer: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(schedule);
      // Tiers switching at `md`, wordmark wrapping, locale swaps and font
      // swaps all resize this wrapper — one observer covers every one of them.
      observer.observe(inner);
    }
    // Fallback for environments without ResizeObserver (and for zoom changes
    // that do not alter the wrapper's border-box).
    window.addEventListener("resize", schedule);

    // Web fonts can land after first paint and change the wordmark's height.
    if (typeof document !== "undefined" && "fonts" in document && document.fonts) {
      document.fonts.ready.then(schedule).catch(() => undefined);
    }

    return () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      observer?.disconnect();
      window.removeEventListener("resize", schedule);
    };
  }, []);

  return (
    <header
      ref={headerRef}
      className="glass-header no-print sticky top-0 z-40 w-full"
    >
      <div
        ref={headerInnerRef}
        className="mx-auto max-w-7xl px-4 sm:px-8 lg:px-10"
      >
        {/* Tier 1 on phones (flex, brand left / utilities right) and the 3-zone
            structural grid on desktop (start · centre nav · end utilities).
            `md:grid-cols-[1fr_auto_1fr]` keeps the centre nav optically centred
            and stops `EN ↔ 中文` growth from shoving the utility cluster. */}
        <div className="flex items-center justify-between gap-2 py-2 md:grid md:grid-cols-[1fr_auto_1fr] md:items-center md:gap-4 md:py-2.5">
          {/* Zone 1 (Start): Brand Wordmark / Logo.
              `shrink-0` below `sm` protects the brand slot: the tighter
              `tracking-[0.08em]` wordmark (~132px for "LIGHTING EXHIBITION" at
              11px) fits beside the compact utility cluster on a single line at
              320px, so the link never shrinks or clips its last letter. From
              `sm` up the slot may flex again so a long operator brand can wrap
              inside the 640–767px band instead of pushing the utilities off. */}
          <div className="flex shrink-0 items-center justify-self-start sm:min-w-0 sm:flex-1 md:flex-none">
            <Link
              href={`/${lang}`}
              // `inline-flex min-h-7 py-1` gives the brand link a real block-sized
              // hit area (>= 24px at every width) instead of collapsing to the
              // glyph line-height — a single-line CJK wordmark at `text-xs` is
              // only ~16.5px tall on its own, which reads as a squeezed slot.
              className="inline-flex min-h-7 min-w-0 items-center py-1 transition-opacity hover:opacity-80"
            >
              {logoUrl !== "" ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  src={logoUrl}
                  alt={displayBrand}
                  className="h-7 w-auto max-w-full object-contain sm:h-8"
                />
              ) : (
                <span className="intrinsic-stack-start text-[11px] leading-snug font-medium tracking-[0.08em] uppercase text-[var(--text-primary)] sm:text-lg sm:tracking-[0.24em]">
                  <span>{displayBrand}</span>
                  {alternateBrand !== "" && (
                    <span aria-hidden="true" className="intrinsic-ghost">
                      {alternateBrand}
                    </span>
                  )}
                </span>
              )}
            </Link>
          </div>

          {/* Zone 2 (Centre, desktop only) + Tier 2 source on phones is rendered
              below as the touch-friendly 3-column anchor row. */}
          <nav
            aria-label="Primary Exhibition Navigation"
            className="hidden items-center justify-self-center gap-7 text-xs tracking-[0.14em] uppercase text-[var(--text-secondary)] md:flex lg:gap-9"
          >
            <Link
              href={`/${lang}#collection`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              <BilingualIntrinsicLabel text={ui.navCollection} lang={lang} />
            </Link>
            <Link
              href={`/${lang}#provenance`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              <BilingualIntrinsicLabel text={ui.navProvenance} lang={lang} />
            </Link>
            <Link
              href={`/${lang}/orders`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              <BilingualIntrinsicLabel text={ui.navOrderLookup} lang={lang} />
            </Link>
          </nav>

          {/* Zone 3 (End): Room Dimmer + Language Switcher + Shopping Cart */}
          <div className="flex shrink-0 items-center justify-self-end gap-1 sm:gap-2.5">
            {/* Room Dimmer: compact glowing Kelvin read-out on phones, the full
                four-label intrinsic stack (Daylight 5500K / Nocturne 2700K, both
                locales) plus the `L` shortcut hint from `sm` upwards. */}
            <button
              type="button"
              onClick={toggleLit}
              aria-pressed={lit}
              aria-label={activeAtmosphereLabel}
              title={activeAtmosphereLabel}
              className="group inline-flex min-h-9 min-w-8 shrink-0 items-center justify-center gap-1.5 whitespace-nowrap border border-[var(--border-hairline)] bg-[var(--surface-card)] px-1.5 text-[10px] tracking-[0.08em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)] sm:gap-2 sm:px-3 sm:text-xs sm:tracking-[0.12em]"
            >
              <span
                className={`inline-block size-2 shrink-0 rounded-full transition-all duration-500 sm:size-2.5 ${
                  lit
                    ? "bg-[var(--color-glow)] shadow-[0_0_10px_var(--color-glow)]"
                    : "border border-[var(--text-secondary)] bg-transparent"
                }`}
              />
              {/* Below 360px the 19-character wordmark and the Kelvin read-out
                  cannot share a 320px line, so the button keeps its state dot
                  (plus aria-label/title) at the narrowest width and restores
                  the compact Kelvin label from 360px up. */}
              <span
                aria-hidden="true"
                className="tabular-nums hidden min-[360px]:inline sm:hidden"
              >
                {compactAtmosphereLabel}
              </span>
              <span className="intrinsic-stack sm-only-inline-grid">
                <span>{activeAtmosphereLabel}</span>
                {allAtmosphereLabels.map((ghost) =>
                  ghost === activeAtmosphereLabel ? null : (
                    <span
                      key={ghost}
                      aria-hidden="true"
                      className="intrinsic-ghost"
                    >
                      {ghost}
                    </span>
                  ),
                )}
              </span>
              <kbd className="hidden border border-[var(--border-hairline)] px-1.5 py-0.5 text-[10px] leading-none text-[var(--text-secondary)] sm:inline-block">
                L
              </kbd>
            </button>

            {/* Route-Preserving Language Switcher (EN | 中文 — intrinsic 2-col grid) */}
            <nav
              aria-label={lang === "zh-HK" ? "語言" : "Language"}
              className="grid shrink-0 grid-cols-2 items-stretch overflow-clip whitespace-nowrap border border-[var(--border-hairline)] bg-[var(--surface-card)] text-[10px] sm:text-xs"
            >
              {locales.map((targetLocale) => {
                const active = targetLocale === lang;
                return (
                  <Link
                    key={targetLocale}
                    href={`/${targetLocale}${suffix}`}
                    lang={htmlLang[targetLocale]}
                    aria-current={active ? "true" : undefined}
                    className={`inline-flex min-h-9 items-center justify-center px-1.5 whitespace-nowrap tracking-[0.06em] transition-colors sm:px-3 ${
                      active
                        ? "bg-[var(--text-primary)] font-medium text-[var(--surface-page)]"
                        : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                    }`}
                  >
                    {languageEndonyms[targetLocale]}
                  </Link>
                );
              })}
            </nav>

            {/* Shopping Cart Drawer Trigger */}
            <button
              type="button"
              onClick={() => setBagOpen(true)}
              aria-label={`${t(ui.navBag, lang)} (${totalBagUnits})`}
              className="relative inline-flex min-h-9 shrink-0 items-center justify-center gap-1.5 whitespace-nowrap border border-[var(--border-strong)] bg-[var(--text-primary)] px-2 text-[10px] tracking-[0.08em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 sm:gap-2 sm:px-3.5 sm:text-xs sm:tracking-[0.14em]"
            >
              {/* Below sm: the compact cart button shows the basket glyph +
                  count pill so the brand wordmark on the left always has
                  generous breathing room; from sm: up it shows the full
                  bilingual "Shopping Cart / 購物車" label. */}
              <svg
                viewBox="0 0 24 24"
                aria-hidden="true"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="size-4 sm:hidden"
              >
                <path d="M3.5 5h2l1.7 9.4a1.6 1.6 0 0 0 1.58 1.32h7.8a1.6 1.6 0 0 0 1.57-1.24L19.6 7.2H6.2" />
                <circle cx="9.6" cy="19.2" r="1.15" />
                <circle cx="16.6" cy="19.2" r="1.15" />
              </svg>
              <span className="hidden sm:inline-flex">
                <BilingualIntrinsicLabel text={ui.navBag} lang={lang} />
              </span>
              {/* Below sm: the count rides as a corner badge on the basket
                  glyph so the ~34px icon button leaves room for the wordmark
                  at 320px; from sm: it returns to the inline pill beside the
                  bilingual "Shopping Cart / 購物車" label. */}
              <span className="tabular-nums absolute -right-1 -top-1 inline-flex items-center justify-center rounded-full bg-[var(--color-glow)] px-1 py-0.5 text-[10px] leading-none font-semibold text-[var(--text-on-glow)] sm:static sm:right-auto sm:top-auto sm:min-w-0 sm:px-1.5 sm:text-[11px]">
                {totalBagUnits}
              </span>
            </button>
          </div>
        </div>

        {/* Tier 2 (phones/tablets only): touch-friendly 3-column anchor row with
            hairline column dividers. Locale-prefixed so the visitor never leaves
            their chosen language (LANG-01). */}
        <div className="grid grid-cols-3 items-center divide-x divide-[var(--border-hairline)] border-t border-[var(--border-hairline)] py-1.5 text-[11px] tracking-[0.12em] uppercase text-[var(--text-secondary)] md:hidden">
          <Link
            href={`/${lang}#collection`}
            className="inline-flex min-h-9 items-center justify-center px-1 text-center transition-colors hover:text-[var(--text-primary)]"
          >
            <BilingualIntrinsicLabel text={ui.navCollection} lang={lang} />
          </Link>
          <Link
            href={`/${lang}#provenance`}
            className="inline-flex min-h-9 items-center justify-center px-1 text-center transition-colors hover:text-[var(--text-primary)]"
          >
            <BilingualIntrinsicLabel text={ui.navProvenance} lang={lang} />
          </Link>
          <Link
            href={`/${lang}/orders`}
            className="inline-flex min-h-9 items-center justify-center px-1 text-center transition-colors hover:text-[var(--text-primary)]"
          >
            <BilingualIntrinsicLabel text={ui.navOrderLookup} lang={lang} />
          </Link>
        </div>
      </div>
    </header>
  );
}
