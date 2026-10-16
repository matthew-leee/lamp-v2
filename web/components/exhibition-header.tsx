"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useStorefront } from "./storefront-context";
import {
  htmlLang,
  isLocale,
  languageEndonyms,
  locales,
  t,
  ui,
  type Locale,
} from "@/lib/i18n";

interface ExhibitionHeaderProps {
  lang: Locale;
  brandName: string;
  brandNameNative: string | null;
}

export default function ExhibitionHeader({
  lang,
  brandName,
  brandNameNative,
}: ExhibitionHeaderProps) {
  const { lit, toggleLit, bag, setBagOpen } = useStorefront();
  const pathname = usePathname() ?? `/${lang}`;

  // Route-preserving language switch path
  const segments = pathname.split("/").filter(Boolean);
  const subSegments =
    segments.length > 0 && isLocale(segments[0]!) ? segments.slice(1) : segments;
  const suffix = subSegments.length > 0 ? `/${subSegments.join("/")}` : "";

  const displayBrand =
    lang === "zh-HK" && brandNameNative ? brandNameNative : brandName;

  const totalBagUnits = bag.reduce((sum, item) => sum + item.quantity, 0);

  return (
    <header className="glass-header no-print sticky top-0 z-40 w-full">
      <div className="mx-auto flex max-w-7xl flex-wrap items-center justify-between gap-4 px-5 py-4 sm:px-10">
        {/* Left: Brand Wordmark + Exhibition Navigation */}
        <div className="flex items-center gap-8 lg:gap-12">
          <Link
            href={`/${lang}`}
            className="text-lg font-medium tracking-[0.28em] uppercase text-[var(--text-primary)] transition-opacity hover:opacity-80"
          >
            {displayBrand}
          </Link>

          <nav
            aria-label="Primary Exhibition Navigation"
            className="hidden items-center gap-7 text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)] md:flex"
          >
            <Link
              href={`/${lang}#collection`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              {t(ui.navCollection, lang)}
            </Link>
            <Link
              href={`/${lang}#edition-matrix`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              {t(ui.navEditionMatrix, lang)}
            </Link>
            <Link
              href={`/${lang}#provenance`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              {t(ui.navProvenance, lang)}
            </Link>
            <Link
              href={`/${lang}/orders`}
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              {t(ui.navOrderLookup, lang)}
            </Link>
          </nav>
        </div>

        {/* Right: Global Room Dimmer + Language Switcher + Shopping Cart */}
        <div className="flex flex-wrap items-center gap-2 sm:gap-4">
          {/* Interactive Room Dimmer Switch */}
          <button
            type="button"
            onClick={toggleLit}
            aria-pressed={lit}
            aria-label={lit ? t(ui.atmosphereNight, lang) : t(ui.atmosphereDay, lang)}
            className="group flex shrink-0 items-center gap-2 whitespace-nowrap border border-[var(--border-hairline)] bg-[var(--surface-card)] px-2.5 py-1.5 text-[11px] tracking-[0.12em] uppercase text-[var(--text-primary)] transition-all hover:border-[var(--color-glow)] sm:px-3.5 sm:py-2 sm:text-xs sm:tracking-[0.14em]"
          >
            <span
              className={`inline-block size-2 rounded-full transition-all duration-500 sm:size-2.5 ${
                lit
                  ? "bg-[var(--color-glow)] shadow-[0_0_10px_var(--color-glow)]"
                  : "border border-[var(--text-secondary)] bg-transparent"
              }`}
            />
            <span>
              {lit ? t(ui.atmosphereNight, lang) : t(ui.atmosphereDay, lang)}
            </span>
            <kbd className="hidden border border-[var(--border-hairline)] px-1.5 py-0.5 text-[10px] text-[var(--text-secondary)] sm:inline-block">
              L
            </kbd>
          </button>

          {/* Route-Preserving Language Switcher */}
          <nav
            aria-label={lang === "zh-HK" ? "語言" : "Language"}
            className="flex shrink-0 items-center whitespace-nowrap border border-[var(--border-hairline)] bg-[var(--surface-card)] text-[11px] sm:text-xs"
          >
            {locales.map((targetLocale) => {
              const active = targetLocale === lang;
              return (
                <Link
                  key={targetLocale}
                  href={`/${targetLocale}${suffix}`}
                  lang={htmlLang[targetLocale]}
                  aria-current={active ? "true" : undefined}
                  className={`whitespace-nowrap px-2 py-1.5 tracking-[0.08em] transition-colors sm:px-2.5 sm:py-2 sm:tracking-[0.1em] ${
                    active
                      ? "bg-[var(--text-primary)] text-[var(--surface-page)] font-medium"
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
            className="flex shrink-0 items-center gap-1.5 whitespace-nowrap border border-[var(--border-strong)] bg-[var(--text-primary)] px-3 py-1.5 text-[11px] tracking-[0.12em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 sm:gap-2 sm:px-4 sm:py-2 sm:text-xs sm:tracking-[0.16em]"
          >
            <span>{t(ui.navBag, lang)}</span>
            <span className="tabular-nums inline-flex min-w-5 items-center justify-center rounded-full bg-[var(--color-glow)] px-1.5 py-0.5 text-[11px] font-semibold text-[#11100e]">
              {totalBagUnits}
            </span>
          </button>
        </div>
      </div>
    </header>
  );
}
