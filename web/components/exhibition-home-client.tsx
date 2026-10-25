"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useStorefront } from "./storefront-context";
import LampStage from "./lamp-stage";
import type { CatalogueProduct } from "@/lib/api";
import {
  buildWhatsAppUrl,
  formatEditionBadge,
  formatMoney,
  formatProductTitle,
} from "@/lib/format";
import { htmlLang, t, ui, type BilingualText, type Locale } from "@/lib/i18n";

interface ExhibitionHomeClientProps {
  lang: Locale;
  products: CatalogueProduct[];
  totalPieces: number;
  currency: string;
  brandName: string;
  brandNameNative: string | null;
  brandLogoUrl?: string | null;
  whatsappNumber: string | null;
  copy: {
    heroEyebrow: string;
    heroLead: string;
    dropFraming: string;
    piecesHeading: string;
    atmosphereCaption: string;
    disclosure: string;
    warrantyPending: string;
    enquiryIntro: string;
    enquiryMessage: string;
  };
}

const pad2 = (n: number): string => String(n).padStart(2, "0");

const HERO_SLOGAN_FALLBACK: BilingualText = {
  en: "Sculptural Danish light, in Hong Kong.",
  "zh-HK": "丹麥建築光影 · 香港現貨典藏",
};

const EXPLORE_COLLECTION_CTA: BilingualText = {
  en: "Explore Collection ↓",
  "zh-HK": "瀏覽典藏系列 ↓",
};

/**
 * NAME-01 / ENV-01 presentational fallback (identical rule to the header).
 *
 * `brand.name` / `brand.name_native` are strictly env/settings-supplied, so a
 * fresh install can hand us two empty strings — which would leave the hero
 * brand emblem frame blank. This is presentation-only and never overrides a
 * configured brand; the strings are the exhibition label, not an identity.
 * Mirrors `ui.defaultExhibitionTitle`.
 */
const FALLBACK_BRAND_WORDMARK: BilingualText = {
  en: "LIGHTING EXHIBITION",
  "zh-HK": "建築燈飾典藏展",
};

/**
 * Resolves the locale-appropriate wordmark (plus the ghost label that reserves
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

function conciseHeroSlogan(raw: string, locale: Locale): string {
  const trimmed = raw.trim();
  if (trimmed === "") return HERO_SLOGAN_FALLBACK[locale];
  if (locale === "zh-HK") {
    return trimmed.length > 14 ? HERO_SLOGAN_FALLBACK[locale] : trimmed;
  }
  const wordCount = trimmed.split(/\s+/).filter(Boolean).length;
  return wordCount > 6 ? HERO_SLOGAN_FALLBACK[locale] : trimmed;
}

/**
 * TRUST-02 (2026-10-23 Founder Decision): never compare our price against
 * competitor or local showroom retail prices anywhere on the storefront.
 */
function sanitizeFramingCopy(raw: string, locale: Locale, totalPieces: number): string {
  const trimmed = raw.trim();
  // Covers explicit showroom/retail copy *and* generic discount phrasing
  // ("Save 20% off list", "under list", "saving", "優惠") so no CMS string
  // can smuggle a competitor price comparison onto the storefront.
  const hasCompetitorComparison =
    /showroom|15%|\d+\s*%|save\b|saving|off\s+list|under\s+list|dealer|retail|competitor|陳列室|專門店|折|優惠/i.test(
      trimmed,
    );
  if (!trimmed || hasCompetitorComparison) {
    return locale === "zh-HK"
      ? `香港存倉限量 ${totalPieces} 件編號藏品，配置合規英規三腳插頭或 USB-C 充電系統，專人預約直送府上。`
      : `A numbered Hong Kong vault edition of ${totalPieces} architectural pieces, complete with Type G power compliance and direct local delivery.`;
  }
  return trimmed;
}

function IntrinsicBilingualSpan({
  text,
  lang,
  suffix = "",
}: {
  text: BilingualText;
  lang: Locale;
  suffix?: string;
}) {
  const otherLocale: Locale = lang === "en" ? "zh-HK" : "en";
  return (
    <span className="intrinsic-stack">
      <span>
        {t(text, lang)}
        {suffix}
      </span>
      <span aria-hidden="true" className="intrinsic-ghost">
        {t(text, otherLocale)}
        {suffix}
      </span>
    </span>
  );
}

export default function ExhibitionHomeClient({
  lang,
  products,
  totalPieces,
  currency,
  brandName,
  brandNameNative,
  brandLogoUrl,
  whatsappNumber,
  copy,
}: ExhibitionHomeClientProps) {
  const { lit, toggleLit, stockMap, stockLoading, addToBag, setBagOpen } =
    useStorefront();

  const [heroStageMode, setHeroStageMode] = useState<"photo" | "scale">("photo");
  const [heroIndex, setHeroIndex] = useState(0);
  const [heroVisible, setHeroVisible] = useState(true);
  const [carouselPaused, setCarouselPaused] = useState(false);
  // Explicit, discoverable pause for touch and keyboard users: `carouselPaused`
  // above is the transient hover/focus pause and cannot be reached on a phone.
  const [manualPaused, setManualPaused] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);

  const heroCount = products.length;
  const heroIndexRef = useRef(0);
  const heroFadeTimer = useRef<number | null>(null);

  // Smooth slide change: fade the active product out for a beat before
  // swapping it, so the 2.2s rotation reads as a dissolve, not a jump.
  const goToHeroIndex = useCallback(
    (nextRaw: number) => {
      if (heroCount <= 1) return;
      const next = ((nextRaw % heroCount) + heroCount) % heroCount;
      if (next === heroIndexRef.current) return;
      if (reducedMotion) {
        heroIndexRef.current = next;
        setHeroIndex(next);
        return;
      }
      setHeroVisible(false);
      if (heroFadeTimer.current !== null) {
        window.clearTimeout(heroFadeTimer.current);
      }
      heroFadeTimer.current = window.setTimeout(() => {
        heroIndexRef.current = next;
        setHeroIndex(next);
        setHeroVisible(true);
        heroFadeTimer.current = null;
      }, 180);
    },
    [heroCount, reducedMotion],
  );

  // DROP-01: auto-rotation pauses while the visitor inspects the carousel,
  // stays paused when they press the explicit Pause control, and stops entirely
  // for `prefers-reduced-motion: reduce`.
  useEffect(() => {
    if (manualPaused || carouselPaused || reducedMotion || heroCount <= 1) return;
    const timer = window.setInterval(() => {
      goToHeroIndex(heroIndexRef.current + 1);
    }, 2200);
    return () => window.clearInterval(timer);
  }, [manualPaused, carouselPaused, reducedMotion, heroCount, goToHeroIndex]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const sync = () => setReducedMotion(media.matches);
    sync();
    media.addEventListener("change", sync);
    return () => media.removeEventListener("change", sync);
  }, []);

  useEffect(
    () => () => {
      if (heroFadeTimer.current !== null) {
        window.clearTimeout(heroFadeTimer.current);
      }
    },
    [],
  );

  const phProducts = products.filter((p) => p.family.trim() === "ph-table");
  const panthellaProducts = products.filter(
    (p) => p.family.trim() === "panthella-160-portable",
  );
  // DROP-01: any 3rd family an operator creates must still surface on the
  // homepage — rendered as its own GALLERY III lineup row below the two
  // curated lineups.
  const otherProducts = products.filter((p) => {
    const family = p.family.trim();
    return family !== "ph-table" && family !== "panthella-160-portable";
  });
  const otherFamilies = Array.from(
    new Set(otherProducts.map((p) => p.family.trim())),
  );

  const totalAvailableUnits = products.reduce((sum, p) => {
    const avail = stockMap?.[p.slug]?.available;
    return sum + (typeof avail === "number" ? avail : p.units_in_drop);
  }, 0);

  const whatsappHref = buildWhatsAppUrl(whatsappNumber, copy.enquiryMessage);
  const { displayBrand, alternateBrand } = resolveBrandWordmarks(
    brandName,
    brandNameNative,
    lang,
  );
  const logoUrl =
    typeof brandLogoUrl === "string" ? brandLogoUrl.trim() : "";
  const slogan = conciseHeroSlogan(copy.heroLead, lang);
  const activeHeroProduct = products[heroIndex] ?? products[0];
  const activeHeroCopy = activeHeroProduct
    ? (activeHeroProduct.content[lang] ?? {})
    : {};

  const activeAtmosphereLabel = lit
    ? t(ui.atmosphereNight, lang)
    : t(ui.atmosphereDay, lang);
  const allAtmosphereLabels = [
    ui.atmosphereDay.en,
    ui.atmosphereDay["zh-HK"],
    ui.atmosphereNight.en,
    ui.atmosphereNight["zh-HK"],
  ];

  const activeStageToggleLabel =
    heroStageMode === "photo"
      ? t(ui.stageModeScale, lang)
      : t(ui.stageModeDay, lang);
  const allStageToggleLabels = [
    ui.stageModeScale.en,
    ui.stageModeScale["zh-HK"],
    ui.stageModeDay.en,
    ui.stageModeDay["zh-HK"],
  ];

  const stockLine = (product: CatalogueProduct) => {
    const stockInfo = stockMap?.[product.slug];
    const avail = stockInfo ? stockInfo.available : null;
    return (
      <span className="flex items-center gap-2 text-xs text-[var(--text-secondary)]">
        <span
          className={`inline-block size-2 shrink-0 rounded-full ${
            avail === null
              ? "animate-pulse bg-[var(--color-glow)]"
              : avail > 0
                ? "bg-[var(--color-glow)]"
                : "bg-[var(--color-error)]"
          }`}
        />
        <span>
          {avail === null
            ? t(ui.stockLoading, lang)
            : avail > 0
              ? t(ui.stockAvailable(avail), lang)
              : t(ui.stockSoldOut, lang)}
        </span>
      </span>
    );
  };

  const isProductSoldOut = (product: CatalogueProduct): boolean => {
    const stockInfo = stockMap?.[product.slug];
    return stockInfo !== undefined && stockInfo.available <= 0;
  };

  // GALLERY I & III: full architectural plinth card.
  // Uses CSS subgrid (`row-span-5 grid grid-rows-subgrid`) so every internal
  // row (stage, badge+meta, title, dimensions+stock, price+CTAs) aligns
  // horizontally across sibling cards without magic `min-h-[...]` heights.
  const renderPlinthCard = (product: CatalogueProduct) => {
    const pCopy = product.content[lang] ?? {};
    const title = formatProductTitle(
      product.model,
      pCopy.model_alt,
      pCopy.colourway,
      lang,
    );
    const badge = formatEditionBadge(product.pieces, totalPieces, lang);
    const soldOut = isProductSoldOut(product);
    return (
      <article
        key={product.slug}
        id={`piece-${product.slug}`}
        className="card-exhibition row-span-5 grid grid-rows-subgrid gap-y-2.5 p-4 sm:p-5"
      >
        <Link
          href={`/${lang}/products/${product.slug}`}
          className="group block"
        >
          <LampStage
            hue={product.art_hue}
            variant={product.art_variant}
            lit={lit}
            imageDayUrl={product.image_day_url}
            imageLitUrl={product.image_lit_url}
            badgeText={badge}
            dimensionsText={product.dimensions}
            aspectClass="aspect-[4/3]"
            altText={title}
          />
        </Link>

        <div className="pt-1">
          <p className="text-[10px] tracking-[0.18em] uppercase text-[var(--color-glow)]">
            {badge}
          </p>
          <p className="mt-1 text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
            {[product.brand, product.designer, product.design_year]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>

        <h3
          lang={htmlLang[lang]}
          className="font-display text-xl leading-snug text-[var(--text-primary)]"
        >
          <Link
            href={`/${lang}/products/${product.slug}`}
            className="hover:underline"
          >
            {title}
          </Link>
        </h3>

        <div className="space-y-1.5 self-end">
          {product.dimensions && (
            <p className="tabular-nums text-xs text-[var(--text-secondary)]">
              {product.dimensions}
            </p>
          )}
          <div>{stockLine(product)}</div>
        </div>

        {/* Aligned bottom bar: price + actions */}
        <div className="@container self-end border-t border-[var(--border-hairline)] pt-4">
          <p className="text-[10px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
            {t(ui.housePriceLabel, lang)}
          </p>
          <p className="tabular-nums mt-1 font-sans text-2xl font-semibold text-[var(--text-primary)]">
            {formatMoney(product.price_cents, currency)}
          </p>

          <div className="mt-3 grid grid-cols-1 gap-2 @[15rem]:grid-cols-2">
            <button
              type="button"
              disabled={soldOut}
              onClick={() => addToBag(product.slug, 1, true)}
              className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-strong)] bg-[var(--text-primary)] px-2.5 py-2.5 text-xs font-medium tracking-[0.12em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-35"
            >
              {soldOut ? t(ui.stockSoldOut, lang) : t(ui.addToBag, lang)}
            </button>
            <Link
              href={`/${lang}/products/${product.slug}`}
              className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-hairline)] px-2.5 py-2.5 text-xs tracking-[0.12em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
            >
              {t(ui.viewDetails, lang)}
            </Link>
          </div>
        </div>
      </article>
    );
  };

  // GALLERY II: compact 5-colourway portable card.
  // Uses CSS subgrid (`row-span-4 grid grid-rows-subgrid`) so colourway titles,
  // stock indicators, and price/action bars align across all 5 columns without
  // truncating titles or hardcoding `min-h-[...]`.
  const renderPortableCard = (product: CatalogueProduct) => {
    const pCopy = product.content[lang] ?? {};
    const colour = pCopy.colourway || product.slug;
    const modelName = formatProductTitle(
      product.model,
      pCopy.model_alt,
      undefined,
      lang,
    );
    const badge = formatEditionBadge(product.pieces, totalPieces, lang);
    const soldOut = isProductSoldOut(product);
    return (
      <article
        key={product.slug}
        className="card-exhibition row-span-4 grid grid-rows-subgrid gap-y-2 p-3.5"
      >
        <Link
          href={`/${lang}/products/${product.slug}`}
          className="group block"
        >
          <LampStage
            hue={product.art_hue}
            variant={product.art_variant}
            lit={lit}
            imageDayUrl={product.image_day_url}
            imageLitUrl={product.image_lit_url}
            badgeText={badge}
            aspectClass="aspect-square"
            altText={colour}
            compact
          />
        </Link>

        <div className="pt-1">
          <h3 className="font-display text-base font-medium leading-snug text-[var(--text-primary)]">
            <Link
              href={`/${lang}/products/${product.slug}`}
              className="hover:underline"
            >
              {colour}
            </Link>
          </h3>
          <p
            lang={htmlLang[lang]}
            className="mt-0.5 text-[10px] tracking-[0.08em] uppercase text-[var(--text-secondary)]"
          >
            {modelName}
          </p>
        </div>

        <div className="self-end">{stockLine(product)}</div>

        <div className="@container self-end border-t border-[var(--border-hairline)] pt-3">
          <p className="tabular-nums font-sans text-lg font-semibold text-[var(--text-primary)]">
            {formatMoney(product.price_cents, currency)}
          </p>
          <div className="mt-2.5 grid grid-cols-1 gap-1.5 @[12rem]:grid-cols-2">
            <button
              type="button"
              disabled={soldOut}
              onClick={() => addToBag(product.slug, 1, true)}
              className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-strong)] bg-[var(--text-primary)] px-2 py-2 text-[10px] font-medium tracking-[0.08em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-35"
            >
              {soldOut ? t(ui.stockSoldOut, lang) : t(ui.addToBag, lang)}
            </button>
            <Link
              href={`/${lang}/products/${product.slug}`}
              className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-hairline)] px-2 py-2 text-[10px] tracking-[0.08em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
            >
              {t(ui.viewDetails, lang)}
            </Link>
          </div>
        </div>
      </article>
    );
  };

  return (
    <div className="mx-auto max-w-7xl px-5 pb-28 sm:px-10">
      {/* First-Fold Viewport-Bounded Stage (Telemetry Bar + Asymmetric Hero):
          Fits 100% within calc(100dvh - var(--header-height)) on 16:9 (1440×740)
          and 16:10 (1440×900) laptops without scrolling, with the carousel visual
          stage using `flex-1 min-h-0` to absorb viewport height differences. */}
      <div className="lg:flex lg:min-h-[calc(100dvh-var(--header-height))] lg:max-h-[calc(100dvh-var(--header-height))] lg:flex-col">
        {/* Top Exhibition Telemetry Bar (shrink-0 fixed chrome) */}
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-[var(--border-hairline)] py-2 text-[11px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
          <div className="flex items-center gap-2.5">
            <span className="inline-block size-2 shrink-0 rounded-full bg-[var(--color-glow)]" />
            <span lang={htmlLang[lang]}>{copy.heroEyebrow}</span>
          </div>
          <div className="tabular-nums flex flex-wrap items-center gap-5">
            <span>
              {lang === "zh-HK" ? "編號作品總數" : "NUMBERED EDITION"}:{" "}
              <strong className="text-[var(--text-primary)]">{totalPieces}</strong>
            </span>
            <span>
              {lang === "zh-HK" ? "香港存倉可認購" : "AVAILABLE IN HK VAULT"}:{" "}
              <strong className="text-[var(--text-primary)]">
                {stockLoading ? "—" : totalAvailableUnits}
              </strong>
            </span>
          </div>
        </div>

        {/* DROP-01: Asymmetric Hero */}
        <section className="grid flex-1 min-h-0 items-stretch gap-6 py-4 lg:grid-cols-12 lg:gap-10 lg:py-5">
          {/* Left: Company logo frame + short slogan + framing + controls */}
          <div className="flex min-h-0 flex-col justify-between py-1 lg:col-span-6">
            <div>
              <div className="inline-flex flex-wrap items-center gap-3">
                <div className="relative inline-flex items-center justify-center border border-[var(--border-hairline)] bg-[var(--surface-card)] px-5 py-2.5 sm:px-7 sm:py-3">
                  {/* Subtle architectural framing lines */}
                  <span
                    aria-hidden="true"
                    className="pointer-events-none absolute -left-px -top-px size-3.5 border-l border-t border-[var(--color-glow)]"
                  />
                  <span
                    aria-hidden="true"
                    className="pointer-events-none absolute -right-px -top-px size-3.5 border-r border-t border-[var(--color-glow)]"
                  />
                  <span
                    aria-hidden="true"
                    className="pointer-events-none absolute -bottom-px -left-px size-3.5 border-b border-l border-[var(--color-glow)]"
                  />
                  <span
                    aria-hidden="true"
                    className="pointer-events-none absolute -bottom-px -right-px size-3.5 border-b border-r border-[var(--color-glow)]"
                  />
                  {logoUrl !== "" ? (
                    // eslint-disable-next-line @next/next/no-img-element
                    <img
                      src={logoUrl}
                      alt={displayBrand}
                      className="h-9 w-auto max-w-full object-contain sm:h-10"
                    />
                  ) : (
                    <span className="intrinsic-stack font-display text-xl tracking-[0.24em] uppercase text-[var(--text-primary)] sm:text-2xl">
                      <span>{displayBrand}</span>
                      {alternateBrand !== displayBrand && (
                        <span aria-hidden="true" className="intrinsic-ghost">
                          {alternateBrand}
                        </span>
                      )}
                    </span>
                  )}
                </div>

                {copy.heroEyebrow && (
                  <span
                    lang={htmlLang[lang]}
                    className="inline-flex items-center gap-2 border border-[var(--border-hairline)] bg-[var(--surface-card)] px-2.5 py-1.5 text-[10px] tracking-[0.18em] uppercase text-[var(--text-secondary)]"
                  >
                    <span className="inline-block size-1.5 rounded-full bg-[var(--color-glow)]" />
                    {copy.heroEyebrow}
                  </span>
                )}
              </div>

              <h1
                lang={htmlLang[lang]}
                className="mt-5 font-display text-[clamp(1.65rem,2.4vw+0.85rem,2.65rem)] leading-[1.16] tracking-tight text-[var(--text-primary)]"
              >
                {slogan}
              </h1>

              <p
                lang={htmlLang[lang]}
                className="mt-3 max-w-xl text-sm leading-relaxed text-[var(--text-secondary)]"
              >
                {sanitizeFramingCopy(copy.dropFraming, lang, totalPieces)}
              </p>
            </div>

            <div className="mt-5">
              <div className="grid grid-cols-1 gap-2.5 min-[420px]:grid-cols-2 sm:flex sm:flex-wrap">
                <a
                  href="#collection"
                  className="inline-flex min-h-11 items-center justify-center bg-[var(--text-primary)] px-6 py-3 text-xs font-medium tracking-[0.18em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90"
                >
                  <IntrinsicBilingualSpan
                    text={EXPLORE_COLLECTION_CTA}
                    lang={lang}
                  />
                </a>

                <button
                  type="button"
                  onClick={toggleLit}
                  aria-pressed={lit}
                  className="inline-flex min-h-11 items-center justify-center gap-2.5 border border-[var(--border-strong)] bg-[var(--surface-card)] px-5 py-3 text-xs tracking-[0.15em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
                >
                  <span
                    className={`inline-block size-2.5 shrink-0 rounded-full ${
                      lit
                        ? "bg-[var(--color-glow)] shadow-[0_0_10px_var(--color-glow)]"
                        : "border border-[var(--text-secondary)]"
                    }`}
                  />
                  <span className="intrinsic-stack">
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
                </button>
              </div>

              {copy.atmosphereCaption && (
                <p
                  lang={htmlLang[lang]}
                  className="mt-4 max-w-xl border-l-2 border-[var(--color-glow)] pl-3 text-xs leading-snug text-[var(--text-secondary)]"
                >
                  {copy.atmosphereCaption}
                </p>
              )}
            </div>
          </div>

          {/* Right: Auto-rotating product carousel (2.2s, pauses on hover/focus) */}
          {activeHeroProduct && (
            <div className="flex min-h-0 flex-col lg:col-span-6">
              <div
                role="group"
                aria-roledescription="carousel"
                aria-label={
                  lang === "zh-HK" ? "精選作品輪播" : "Featured piece carousel"
                }
                onMouseEnter={() => setCarouselPaused(true)}
                onMouseLeave={() => setCarouselPaused(false)}
                onFocusCapture={() => setCarouselPaused(true)}
                onBlurCapture={() => setCarouselPaused(false)}
                className="@container card-exhibition flex flex-1 min-h-0 flex-col justify-between p-3.5 sm:p-4"
              >
                {/* Carousel controls (shrink-0): prev/next arrows, an explicit
                    pause/play toggle reachable by touch & keyboard, the blueprint
                    toggle, and the numbered indicator reel. */}
                <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => goToHeroIndex(heroIndexRef.current - 1)}
                      disabled={heroCount <= 1}
                      aria-label={t(ui.heroCarouselPrev, lang)}
                      className="inline-flex min-h-9 min-w-9 items-center justify-center border border-[var(--border-hairline)] px-2 text-xs text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)] disabled:cursor-not-allowed disabled:opacity-30"
                    >
                      ←
                    </button>
                    <button
                      type="button"
                      onClick={() => goToHeroIndex(heroIndexRef.current + 1)}
                      disabled={heroCount <= 1}
                      aria-label={t(ui.heroCarouselNext, lang)}
                      className="inline-flex min-h-9 min-w-9 items-center justify-center border border-[var(--border-hairline)] px-2 text-xs text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)] disabled:cursor-not-allowed disabled:opacity-30"
                    >
                      →
                    </button>
                    <button
                      type="button"
                      onClick={() => setManualPaused((paused) => !paused)}
                      disabled={heroCount <= 1}
                      aria-pressed={manualPaused}
                      aria-label={
                        manualPaused
                          ? t(ui.heroCarouselPlay, lang)
                          : t(ui.heroCarouselPause, lang)
                      }
                      className="inline-flex min-h-9 min-w-9 items-center justify-center border border-[var(--border-hairline)] px-2 text-xs leading-none text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)] disabled:cursor-not-allowed disabled:opacity-30"
                    >
                      <span aria-hidden="true">
                        {manualPaused ? "▶" : "⏸"}
                      </span>
                    </button>
                    {activeHeroProduct.art_variant !== "none" && (
                      <button
                        type="button"
                        onClick={() =>
                          setHeroStageMode((mode) =>
                            mode === "photo" ? "scale" : "photo",
                          )
                        }
                        className="inline-flex min-h-9 min-w-9 items-center justify-center border border-[var(--border-hairline)] px-2 text-[10px] tracking-[0.1em] uppercase text-[var(--text-secondary)] transition-colors hover:border-[var(--color-glow)] hover:text-[var(--text-primary)]"
                      >
                        <span className="intrinsic-stack">
                          <span>{activeStageToggleLabel}</span>
                          {allStageToggleLabels.map((ghost) =>
                            ghost === activeStageToggleLabel ? null : (
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
                      </button>
                    )}
                  </div>

                  {/* `justify-start` (not `justify-end`) matters once the reel
                      overflows: with `flex-end` Chrome clips the first
                      indicators into the unreachable negative scroll region
                      (scrollWidth == clientWidth), so reel-1 could never be
                      scrolled into view at 320px. Aligned right it is
                      indistinguishable when the row fits. */}
                  <div className="reel-x flex max-w-full min-w-0 items-center justify-start gap-1">
                    {products.map((product, index) => {
                      const isActive = index === heroIndex;
                      return (
                        <button
                          key={product.slug}
                          type="button"
                          onClick={() => goToHeroIndex(index)}
                          aria-label={`${pad2(index + 1)} · ${formatProductTitle(
                            product.model,
                            product.content[lang]?.model_alt,
                            product.content[lang]?.colourway,
                            lang,
                          )}`}
                          aria-current={isActive ? "true" : undefined}
                          data-active={isActive ? "true" : undefined}
                          className={`reel-${index + 1} tabular-nums inline-flex min-h-9 min-w-9 items-center justify-center border px-2 text-[10px] tracking-[0.06em] transition-colors ${
                            isActive
                              ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                              : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:border-[var(--color-glow)] hover:text-[var(--text-primary)]"
                          }`}
                        >
                          {pad2(index + 1)}
                        </button>
                      );
                    })}
                  </div>
                </div>

                {/* Active product with smooth dissolve transition & flex-1 min-h-0 stage */}
                <div
                  className={`mt-2.5 flex flex-1 min-h-0 flex-col justify-between gap-3 transition-opacity duration-300 motion-reduce:transition-none ${
                    heroVisible ? "opacity-100" : "opacity-0"
                  }`}
                >
                  <div className="flex flex-1 min-h-0 flex-col">
                    <LampStage
                      hue={activeHeroProduct.art_hue}
                      variant={activeHeroProduct.art_variant}
                      lit={lit}
                      imageDayUrl={activeHeroProduct.image_day_url}
                      imageLitUrl={activeHeroProduct.image_lit_url}
                      badgeText={formatEditionBadge(
                        activeHeroProduct.pieces,
                        totalPieces,
                        lang,
                      )}
                      dimensionsText={activeHeroProduct.dimensions}
                      mode={heroStageMode}
                      aspectClass="aspect-[16/10] lg:aspect-auto lg:flex-1 lg:min-h-0"
                      fit="contain"
                      altText={formatProductTitle(
                        activeHeroProduct.model,
                        activeHeroCopy.model_alt,
                        activeHeroCopy.colourway,
                        lang,
                      )}
                      priority
                    />
                  </div>

                  <div className="flex flex-col gap-3 border-t border-[var(--border-hairline)] pt-2.5 @[28rem]:flex-row @[28rem]:items-center @[28rem]:justify-between">
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[10px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                        {[
                          activeHeroProduct.brand,
                          activeHeroProduct.designer,
                          activeHeroProduct.design_year,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </p>
                      <Link
                        href={`/${lang}/products/${activeHeroProduct.slug}`}
                        className="mt-0.5 block font-display text-sm font-medium leading-snug text-[var(--text-primary)] hover:underline sm:text-base"
                      >
                        {formatProductTitle(
                          activeHeroProduct.model,
                          activeHeroCopy.model_alt,
                          activeHeroCopy.colourway,
                          lang,
                        )}
                      </Link>
                      <div className="mt-1 flex flex-wrap items-center gap-3">
                        <span className="tabular-nums font-sans text-xl font-semibold leading-none text-[var(--text-primary)]">
                          {formatMoney(activeHeroProduct.price_cents, currency)}
                        </span>
                        {stockLine(activeHeroProduct)}
                      </div>
                    </div>

                    <div className="grid grid-cols-2 gap-2 @[28rem]:flex @[28rem]:shrink-0">
                      <button
                        type="button"
                        disabled={isProductSoldOut(activeHeroProduct)}
                        onClick={() => addToBag(activeHeroProduct.slug, 1, true)}
                        className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-strong)] bg-[var(--text-primary)] px-3 py-2 text-[11px] font-medium tracking-[0.12em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-35"
                      >
                        <IntrinsicBilingualSpan text={ui.addToBag} lang={lang} />
                      </button>
                      <Link
                        href={`/${lang}/products/${activeHeroProduct.slug}`}
                        className="inline-flex min-h-11 items-center justify-center whitespace-nowrap border border-[var(--border-hairline)] px-3 py-2 text-[11px] tracking-[0.12em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
                      >
                        <IntrinsicBilingualSpan
                          text={ui.viewDetails}
                          lang={lang}
                          suffix=" →"
                        />
                      </Link>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          )}
        </section>
      </div>

      {/* DROP-01: two categorized product lineup galleries (no duplicate
          catalogue grid, no edition matrix, no competitor price comparisons). */}
      <section id="collection" className="mt-10 space-y-12">
        <div className="border-b border-[var(--border-hairline)] pb-6">
          <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
            {lang === "zh-HK" ? "策展作品目錄" : "CURATED EXHIBITION CATALOGUE"}
          </p>
          <h2
            lang={htmlLang[lang]}
            className="mt-2 font-display text-3xl text-[var(--text-primary)]"
          >
            {copy.piecesHeading}
          </h2>
        </div>

        {/* GALLERY I — Flagship PH Table Series (3-column subgrid plinths) */}
        {phProducts.length > 0 && (
          <div>
            <p className="mb-3 text-[11px] font-semibold tracking-[0.2em] uppercase text-[var(--color-glow)]">
              {lang === "zh-HK"
                ? "第一展區 · Poul Henningsen 經典檯燈系列"
                : "GALLERY I · POUL HENNINGSEN FLAGSHIP TABLE SERIES"}
            </p>
            <div className="grid gap-x-6 gap-y-2.5 lg:grid-cols-3">
              {phProducts.map(renderPlinthCard)}
            </div>
          </div>
        )}

        {/* GALLERY II — Panthella 160 V3 Portable Series (5-colourway subgrid shelf) */}
        {panthellaProducts.length > 0 && (
          <div>
            <div className="flex flex-wrap items-end justify-between gap-2 border-b border-[var(--border-hairline)] pb-4">
              <div>
                <p className="text-[11px] font-semibold tracking-[0.2em] uppercase text-[var(--color-glow)]">
                  {lang === "zh-HK"
                    ? "第二展區 · Verner Panton Panthella 160 V3 便攜檯燈全色系長廊"
                    : "GALLERY II · VERNER PANTON PANTHELLA 160 V3 PORTABLE SERIES"}
                </p>
                <p className="mt-1 text-xs text-[var(--text-secondary)]">
                  {lang === "zh-HK"
                    ? "5 款色調橫向並列展示，一眼飽覽全系列現貨及價格"
                    : "All 5 portable colourways side-by-side on one architectural shelf"}
                </p>
              </div>
            </div>
            <div className="mt-5 grid grid-cols-1 gap-x-4 gap-y-2 min-[420px]:grid-cols-2 md:grid-cols-3 lg:grid-cols-5">
              {panthellaProducts.map(renderPortableCard)}
            </div>
          </div>
        )}

        {/* GALLERY III — any additional operator-created family stays visible */}
        {otherFamilies.map((family) => {
          const familyProducts = otherProducts.filter(
            (p) => p.family.trim() === family,
          );
          const prettyFamily = family.replace(/[-_]+/g, " ").trim();
          const heading =
            prettyFamily === ""
              ? lang === "zh-HK"
                ? "其他精選作品"
                : "MORE CURATED PIECES"
              : lang === "zh-HK"
                ? `第三展區 · ${prettyFamily}`
                : `GALLERY III · ${prettyFamily.toUpperCase()}`;
          return (
            <div key={family || "__unfiled"}>
              <p className="mb-3 text-[11px] font-semibold tracking-[0.2em] uppercase text-[var(--color-glow)]">
                {heading}
              </p>
              <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
                {familyProducts.map(renderPlinthCard)}
              </div>
            </div>
          );
        })}
      </section>

      {/* Provenance, House Warranty & Private Concierge Section (TRUST-01) */}
      <section
        id="provenance"
        className="mt-28 border-t border-[var(--border-hairline)] pt-16"
      >
        <div className="grid gap-10 lg:grid-cols-3">
          <div className="card-exhibition p-6 sm:p-8">
            <p className="text-[11px] tracking-[0.2em] uppercase text-[var(--color-glow)]">
              01 · {t(ui.specDisclosure, lang)}
            </p>
            <p
              lang={htmlLang[lang]}
              className="mt-4 text-sm leading-relaxed text-[var(--text-primary)]"
            >
              {copy.disclosure}
            </p>
          </div>

          <div className="card-exhibition p-6 sm:p-8">
            <p className="text-[11px] tracking-[0.2em] uppercase text-[var(--color-glow)]">
              02 · {t(ui.specWarranty, lang)}
            </p>
            <p
              lang={htmlLang[lang]}
              className="mt-4 text-sm leading-relaxed text-[var(--text-primary)]"
            >
              {copy.warrantyPending}
            </p>
          </div>

          <div className="card-exhibition flex flex-col justify-between p-6 sm:p-8">
            <div>
              <p className="text-[11px] tracking-[0.2em] uppercase text-[var(--color-glow)]">
                03 · {t(ui.conciergeCta, lang)}
              </p>
              <p
                lang={htmlLang[lang]}
                className="mt-4 text-sm leading-relaxed text-[var(--text-primary)]"
              >
                {copy.enquiryIntro}
              </p>
            </div>
            <div className="mt-6 flex flex-wrap gap-3">
              {whatsappHref && (
                <a
                  href={whatsappHref}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center border border-[var(--border-strong)] bg-[var(--text-primary)] px-5 py-3 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)]"
                >
                  {t(ui.conciergeCta, lang)} ↗
                </a>
              )}
              <button
                type="button"
                onClick={() => setBagOpen(true)}
                className="border border-[var(--border-hairline)] px-5 py-3 text-xs tracking-[0.16em] uppercase hover:border-[var(--color-glow)]"
              >
                {t(ui.bagTitle, lang)}
              </button>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
