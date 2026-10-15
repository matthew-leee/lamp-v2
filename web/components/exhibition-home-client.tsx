"use client";

import { useState } from "react";
import Link from "next/link";
import { useStorefront } from "./storefront-context";
import LampStage from "./lamp-stage";
import type { CatalogueProduct } from "@/lib/api";
import {
  buildWhatsAppUrl,
  computeSavingsPercent,
  formatEditionBadge,
  formatMoney,
  formatProductTitle,
} from "@/lib/format";
import { htmlLang, t, ui, type Locale } from "@/lib/i18n";

interface ExhibitionHomeClientProps {
  lang: Locale;
  products: CatalogueProduct[];
  totalPieces: number;
  currency: string;
  brandName: string;
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

export default function ExhibitionHomeClient({
  lang,
  products,
  totalPieces,
  currency,
  whatsappNumber,
  copy,
}: ExhibitionHomeClientProps) {
  const { lit, toggleLit, stockMap, stockLoading, addToBag, setBagOpen } =
    useStorefront();

  const [heroStageMode, setHeroStageMode] = useState<"photo" | "scale">("photo");
  const [selectedPieceNo, setSelectedPieceNo] = useState<number>(1);
  const [familyFilter, setFamilyFilter] = useState<string>("all");
  const [viewMode, setViewMode] = useState<"grid" | "index">("grid");

  const heroProduct = products[0];

  // Build 01..N collector matrix entries from products' pieces
  const pieceEntries = products
    .flatMap((prod) => prod.pieces.map((no) => ({ number: no, product: prod })))
    .sort((a, b) => a.number - b.number);

  const activeMatrixEntry =
    pieceEntries.find((e) => e.number === selectedPieceNo) ?? pieceEntries[0];

  // Distinct families for filtering
  const families = Array.from(
    new Set(products.map((p) => p.family.trim()).filter((f) => f !== "")),
  );

  const filteredProducts =
    familyFilter === "all"
      ? products
      : products.filter((p) => p.family.trim() === familyFilter);

  const totalAvailableUnits = products.reduce((sum, p) => {
    const avail = stockMap?.[p.slug]?.available;
    return sum + (typeof avail === "number" ? avail : p.units_in_drop);
  }, 0);

  const whatsappHref = buildWhatsAppUrl(whatsappNumber, copy.enquiryMessage);

  return (
    <div className="mx-auto max-w-7xl px-5 pb-28 sm:px-10">
      {/* Top Exhibition Telemetry Bar */}
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-[var(--border-hairline)] py-4 text-[11px] tracking-[0.2em] uppercase text-[var(--text-secondary)]">
        <div className="flex items-center gap-3">
          <span className="inline-block size-2 rounded-full bg-[var(--color-glow)]" />
          <span lang={htmlLang[lang]}>{copy.heroEyebrow}</span>
        </div>
        <div className="tabular-nums flex flex-wrap items-center gap-6">
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

      {/* Asymmetric Exhibition Hero */}
      <section className="grid items-center gap-12 pt-12 lg:grid-cols-12 lg:gap-16 lg:pt-20">
        <div className="lg:col-span-7">
          <h1
            lang={htmlLang[lang]}
            className="font-display text-4xl leading-[1.08] tracking-tight text-[var(--text-primary)] sm:text-6xl lg:text-[4.25rem]"
          >
            {copy.heroLead}
          </h1>

          <p
            lang={htmlLang[lang]}
            className="mt-8 max-w-2xl text-base leading-relaxed text-[var(--text-secondary)] sm:text-lg"
          >
            {copy.dropFraming}
          </p>

          {/* Interactive Hero Controls */}
          <div className="mt-10 flex flex-wrap items-center gap-4">
            <a
              href="#collection"
              className="bg-[var(--text-primary)] px-7 py-4 text-xs font-medium tracking-[0.2em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90"
            >
              {lang === "zh-HK" ? "瀏覽典藏作品 ↓" : "Explore the Drop ↓"}
            </a>

            <button
              type="button"
              onClick={toggleLit}
              aria-pressed={lit}
              className="flex items-center gap-3 border border-[var(--border-strong)] bg-[var(--surface-card)] px-6 py-4 text-xs tracking-[0.18em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
            >
              <span
                className={`inline-block size-2.5 rounded-full ${
                  lit
                    ? "bg-[var(--color-glow)] shadow-[0_0_10px_var(--color-glow)]"
                    : "border border-[var(--text-secondary)]"
                }`}
              />
              <span>
                {lit
                  ? lang === "zh-HK"
                    ? "切換至日光展廳 (5500K)"
                    : "Switch to Daylight (5500K)"
                  : lang === "zh-HK"
                    ? "點亮 2700K 夜覽光效"
                    : "Ignite 2700K Nocturne"}
              </span>
            </button>
          </div>

          {copy.atmosphereCaption && (
            <p
              lang={htmlLang[lang]}
              className="mt-6 max-w-xl border-l-2 border-[var(--color-glow)] pl-4 text-xs leading-relaxed text-[var(--text-secondary)]"
            >
              {copy.atmosphereCaption}
            </p>
          )}
        </div>

        {/* Hero Interactive Lamp Stage */}
        {heroProduct && (
          <div className="lg:col-span-5">
            <div className="card-exhibition p-4 sm:p-6">
              <LampStage
                hue={heroProduct.art_hue}
                variant={heroProduct.art_variant}
                lit={lit}
                imageDayUrl={heroProduct.image_day_url}
                imageLitUrl={heroProduct.image_lit_url}
                badgeText={formatEditionBadge(
                  heroProduct.pieces,
                  totalPieces,
                  lang,
                )}
                dimensionsText={heroProduct.dimensions}
                mode={heroStageMode}
                aspectClass="aspect-square"
                altText={formatProductTitle(
                  heroProduct.model,
                  heroProduct.content[lang]?.model_alt,
                  heroProduct.content[lang]?.colourway,
                  lang,
                )}
              />

              {/* Hero Stage Footer Bar */}
              <div className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-[var(--border-hairline)] pt-4">
                <div>
                  <p className="text-[10px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
                    {heroProduct.brand} · {heroProduct.designer}
                  </p>
                  <Link
                    href={`/${lang}/products/${heroProduct.slug}`}
                    className="mt-0.5 block font-display text-lg hover:underline"
                  >
                    {formatProductTitle(
                      heroProduct.model,
                      heroProduct.content[lang]?.model_alt,
                      heroProduct.content[lang]?.colourway,
                      lang,
                    )}
                  </Link>
                </div>

                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() =>
                      setHeroStageMode((m) => (m === "photo" ? "scale" : "photo"))
                    }
                    className="border border-[var(--border-hairline)] px-3 py-1.5 text-[11px] tracking-[0.14em] uppercase text-[var(--text-secondary)] hover:border-[var(--color-glow)] hover:text-[var(--text-primary)]"
                  >
                    {heroStageMode === "photo"
                      ? t(ui.stageModeScale, lang)
                      : t(ui.stageModeDay, lang)}
                  </button>
                  <Link
                    href={`/${lang}/products/${heroProduct.slug}`}
                    className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-3.5 py-1.5 text-[11px] tracking-[0.14em] uppercase text-[var(--surface-page)]"
                  >
                    {t(ui.viewDetails, lang)}
                  </Link>
                </div>
              </div>
            </div>
          </div>
        )}
      </section>

      {/* Interactive Edition 01–16 Collector's Matrix */}
      {pieceEntries.length > 0 && (
        <section
          id="edition-matrix"
          className="mt-24 scroll-mt-24 border border-[var(--border-hairline)] bg-[var(--surface-card)] p-6 sm:p-8"
        >
          <div className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
            <div>
              <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
                {lang === "zh-HK"
                  ? "互動式典藏編號索引"
                  : "INTERACTIVE SERIALIZED EDITION INDEX"}
              </p>
              <h2 className="mt-1 font-display text-2xl">
                {lang === "zh-HK"
                  ? `首發編號矩陣（01 至 ${pad2(totalPieces)}）`
                  : `Collector's Edition Matrix (01 to ${pad2(totalPieces)})`}
              </h2>
            </div>
            <p className="text-xs text-[var(--text-secondary)]">
              {lang === "zh-HK"
                ? "點選任何編號以預覽對應燈飾及即時存倉狀態"
                : "Select any edition number to inspect its piece and live vault status"}
            </p>
          </div>

          {/* 01..16 Numbered Pill Strip */}
          <div className="mt-6 grid grid-cols-4 gap-2 sm:grid-cols-8 lg:grid-cols-16">
            {pieceEntries.map(({ number, product }) => {
              const isSelected = number === activeMatrixEntry?.number;
              const avail = stockMap?.[product.slug]?.available ?? 1;
              return (
                <button
                  key={number}
                  type="button"
                  onClick={() => setSelectedPieceNo(number)}
                  onMouseEnter={() => setSelectedPieceNo(number)}
                  aria-label={`Edition ${pad2(number)}`}
                  className={`tabular-nums flex flex-col items-center justify-center border py-3 transition-all ${
                    isSelected
                      ? "border-[var(--color-glow)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                      : "border-[var(--border-hairline)] bg-[var(--surface-page)] text-[var(--text-primary)] hover:border-[var(--color-glow)]"
                  }`}
                >
                  <span className="text-xs font-semibold tracking-[0.12em]">
                    {pad2(number)}
                  </span>
                  <span
                    className={`mt-1.5 size-1.5 rounded-full ${
                      avail > 0 ? "bg-[var(--color-glow)]" : "bg-[var(--color-error)]"
                    }`}
                  />
                </button>
              );
            })}
          </div>

          {/* Active Matrix Piece Inspector Bar */}
          {activeMatrixEntry && (
            <div className="mt-6 flex flex-wrap items-center justify-between gap-6 border-t border-[var(--border-hairline)] pt-6">
              <div className="flex items-center gap-5">
                <div className="w-16 shrink-0">
                  <LampStage
                    hue={activeMatrixEntry.product.art_hue}
                    variant={activeMatrixEntry.product.art_variant}
                    lit={lit}
                    imageDayUrl={activeMatrixEntry.product.image_day_url}
                    imageLitUrl={activeMatrixEntry.product.image_lit_url}
                    aspectClass="aspect-square"
                  />
                </div>
                <div>
                  <p className="tabular-nums text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
                    {lang === "zh-HK"
                      ? `第 ${pad2(activeMatrixEntry.number)} 號作品 / 共 ${totalPieces} 件`
                      : `EDITION NO. ${pad2(activeMatrixEntry.number)} OF ${totalPieces}`}
                  </p>
                  <h3 className="mt-1 font-display text-xl">
                    {formatProductTitle(
                      activeMatrixEntry.product.model,
                      activeMatrixEntry.product.content[lang]?.model_alt,
                      activeMatrixEntry.product.content[lang]?.colourway,
                      lang,
                    )}
                  </h3>
                  <p className="mt-0.5 text-xs text-[var(--text-secondary)]">
                    {[
                      activeMatrixEntry.product.designer,
                      activeMatrixEntry.product.design_year,
                      activeMatrixEntry.product.dimensions,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-4">
                <p className="tabular-nums font-display text-2xl">
                  {formatMoney(activeMatrixEntry.product.price_cents, currency)}
                </p>
                <button
                  type="button"
                  disabled={
                    (stockMap?.[activeMatrixEntry.product.slug]?.available ?? 1) <= 0
                  }
                  onClick={() => addToBag(activeMatrixEntry.product.slug, 1, true)}
                  className="border border-[var(--border-hairline)] px-4 py-2.5 text-xs tracking-[0.16em] uppercase hover:border-[var(--color-glow)] disabled:opacity-40"
                >
                  {t(ui.addToBag, lang)}
                </button>
                <Link
                  href={`/${lang}/products/${activeMatrixEntry.product.slug}`}
                  className="bg-[var(--text-primary)] px-5 py-2.5 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)]"
                >
                  {t(ui.viewDetails, lang)} →
                </Link>
              </div>
            </div>
          )}
        </section>
      )}

      {/* Main Collection Section: Dual View (Exhibition Grid vs Archival Index) + Family Filter */}
      <section id="collection" className="mt-24 scroll-mt-24">
        <div className="flex flex-wrap items-end justify-between gap-6 border-b border-[var(--border-hairline)] pb-6">
          <div>
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

          {/* Filter & View Switcher Controls */}
          <div className="flex flex-wrap items-center gap-3">
            {/* Family Filter Pills */}
            <div className="flex flex-wrap items-center gap-1.5">
              <button
                type="button"
                onClick={() => setFamilyFilter("all")}
                className={`border px-3 py-1.5 text-xs tracking-[0.12em] uppercase transition-colors ${
                  familyFilter === "all"
                    ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                    : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                }`}
              >
                {t(ui.filterAll, lang)} ({products.length})
              </button>
              {families.map((fam) => (
                <button
                  key={fam}
                  type="button"
                  onClick={() => setFamilyFilter(fam)}
                  className={`border px-3 py-1.5 text-xs tracking-[0.12em] uppercase transition-colors ${
                    familyFilter === fam
                      ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                      : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                  }`}
                >
                  {fam.replace(/-/g, " ")}
                </button>
              ))}
            </div>

            {/* Grid vs Archival Index Toggle */}
            <div className="flex border border-[var(--border-hairline)] bg-[var(--surface-card)] text-xs">
              <button
                type="button"
                onClick={() => setViewMode("grid")}
                className={`px-3 py-1.5 tracking-[0.12em] uppercase ${
                  viewMode === "grid"
                    ? "bg-[var(--text-primary)] text-[var(--surface-page)]"
                    : "text-[var(--text-secondary)]"
                }`}
              >
                {t(ui.viewCanvas, lang)}
              </button>
              <button
                type="button"
                onClick={() => setViewMode("index")}
                className={`px-3 py-1.5 tracking-[0.12em] uppercase ${
                  viewMode === "index"
                    ? "bg-[var(--text-primary)] text-[var(--surface-page)]"
                    : "text-[var(--text-secondary)]"
                }`}
              >
                {t(ui.viewIndex, lang)}
              </button>
            </div>
          </div>
        </div>

        {viewMode === "grid" ? (
          <ul className="mt-12 grid gap-8 sm:grid-cols-2 lg:grid-cols-3">
            {filteredProducts.map((product) => {
              const pCopy = product.content[lang] ?? {};
              const title = formatProductTitle(
                product.model,
                pCopy.model_alt,
                pCopy.colourway,
                lang,
              );
              const badge = formatEditionBadge(product.pieces, totalPieces, lang);
              const stockInfo = stockMap?.[product.slug];
              const avail = stockInfo ? stockInfo.available : null;
              const isSoldOut = avail !== null && avail <= 0;
              const savings = product.reference_price
                ? computeSavingsPercent(
                    product.price_cents,
                    product.reference_price.min_price_cents,
                    product.reference_price.max_price_cents,
                  )
                : null;

              return (
                <li
                  key={product.slug}
                  id={`piece-${product.slug}`}
                  className="card-exhibition group flex flex-col justify-between p-5"
                >
                  <div>
                    <Link
                      href={`/${lang}/products/${product.slug}`}
                      className="block"
                    >
                      <LampStage
                        hue={product.art_hue}
                        variant={product.art_variant}
                        lit={lit}
                        imageDayUrl={product.image_day_url}
                        imageLitUrl={product.image_lit_url}
                        badgeText={badge}
                        dimensionsText={product.dimensions}
                      />

                      <div className="mt-5 flex items-center justify-between gap-2 text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                        <span>
                          {[product.brand, product.designer, product.design_year]
                            .filter(Boolean)
                            .join(" · ")}
                        </span>
                      </div>

                      <h3
                        lang={htmlLang[lang]}
                        className="mt-2 font-display text-xl leading-snug text-[var(--text-primary)] group-hover:underline"
                      >
                        {title}
                      </h3>
                    </Link>

                    {/* Price + Market Comparison Pill */}
                    <div className="mt-4 flex flex-wrap items-baseline justify-between gap-2 border-t border-[var(--border-hairline)] pt-4">
                      <div>
                        <p className="tabular-nums font-sans text-2xl font-medium text-[var(--text-primary)]">
                          {formatMoney(product.price_cents, currency)}
                        </p>
                        {product.reference_price && (
                          <p className="tabular-nums mt-0.5 text-[11px] text-[var(--text-secondary)]">
                            {lang === "zh-HK" ? "陳列室參考價" : "HK Showroom"}:{" "}
                            {formatMoney(
                              product.reference_price.min_price_cents,
                              currency,
                            )}
                            –
                            {formatMoney(
                              product.reference_price.max_price_cents,
                              currency,
                            )}
                          </p>
                        )}
                      </div>

                      {savings && (
                        <span className="border border-[var(--color-glow)] px-2 py-0.5 text-[10px] font-medium tracking-[0.14em] uppercase text-[var(--color-glow)]">
                          {lang === "zh-HK"
                            ? `節省 ${savings.minPct}%–${savings.maxPct}%`
                            : `SAVE ${savings.minPct}%–${savings.maxPct}%`}
                        </span>
                      )}
                    </div>

                    {/* Live Stock Status Line */}
                    <div className="mt-3 flex items-center gap-2 text-xs text-[var(--text-secondary)]">
                      <span
                        className={`inline-block size-2 rounded-full ${
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
                    </div>
                  </div>

                  {/* Card Action Bar */}
                  <div className="mt-5 grid grid-cols-2 gap-2.5 pt-2">
                    <button
                      type="button"
                      disabled={isSoldOut}
                      onClick={() => addToBag(product.slug, 1, true)}
                      className="border border-[var(--border-strong)] bg-[var(--text-primary)] py-2.5 text-xs font-medium tracking-[0.15em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-35"
                    >
                      {isSoldOut
                        ? t(ui.stockSoldOut, lang)
                        : t(ui.addToBag, lang)}
                    </button>
                    <Link
                      href={`/${lang}/products/${product.slug}`}
                      className="flex items-center justify-center border border-[var(--border-hairline)] py-2.5 text-xs tracking-[0.15em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
                    >
                      {t(ui.viewDetails, lang)}
                    </Link>
                  </div>
                </li>
              );
            })}
          </ul>
        ) : (
          /* Archival Table Index View */
          <div className="mt-8 overflow-x-auto border border-[var(--border-hairline)] bg-[var(--surface-card)]">
            <table className="w-full border-collapse text-left text-sm">
              <thead>
                <tr className="border-b border-[var(--border-hairline)] text-[11px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "編號" : "Edition"}
                  </th>
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "作品名稱" : "Piece"}
                  </th>
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "設計師 / 年份" : "Designer / Year"}
                  </th>
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "尺寸" : "Dimensions"}
                  </th>
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "直購價" : "House Price"}
                  </th>
                  <th className="px-4 py-3">
                    {lang === "zh-HK" ? "存倉狀態" : "Vault Status"}
                  </th>
                  <th className="px-4 py-3"></th>
                </tr>
              </thead>
              <tbody>
                {filteredProducts.map((product) => {
                  const pCopy = product.content[lang] ?? {};
                  const title = formatProductTitle(
                    product.model,
                    pCopy.model_alt,
                    pCopy.colourway,
                    lang,
                  );
                  const avail = stockMap?.[product.slug]?.available ?? null;
                  return (
                    <tr
                      key={product.slug}
                      className="border-b border-[var(--border-hairline)] hover:bg-[var(--surface-elevated)]"
                    >
                      <td className="tabular-nums px-4 py-3.5 text-xs text-[var(--text-secondary)]">
                        {formatEditionBadge(product.pieces, totalPieces, lang)}
                      </td>
                      <td className="px-4 py-3.5 font-medium">
                        <Link
                          href={`/${lang}/products/${product.slug}`}
                          className="hover:underline"
                        >
                          {title}
                        </Link>
                      </td>
                      <td className="px-4 py-3.5 text-xs text-[var(--text-secondary)]">
                        {[product.designer, product.design_year]
                          .filter(Boolean)
                          .join(" · ") || "—"}
                      </td>
                      <td className="tabular-nums px-4 py-3.5 text-xs text-[var(--text-secondary)]">
                        {product.dimensions || "—"}
                      </td>
                      <td className="tabular-nums px-4 py-3.5 font-medium">
                        {formatMoney(product.price_cents, currency)}
                      </td>
                      <td className="px-4 py-3.5 text-xs">
                        {avail === null
                          ? t(ui.stockLoading, lang)
                          : avail > 0
                            ? t(ui.stockAvailable(avail), lang)
                            : t(ui.stockSoldOut, lang)}
                      </td>
                      <td className="px-4 py-3.5 text-right">
                        <button
                          type="button"
                          disabled={avail !== null && avail <= 0}
                          onClick={() => addToBag(product.slug, 1, true)}
                          className="border border-[var(--border-hairline)] px-3 py-1 text-xs tracking-[0.12em] uppercase hover:border-[var(--color-glow)] disabled:opacity-35"
                        >
                          {t(ui.addToBag, lang)}
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {/* Provenance, House Warranty & Private Concierge Section */}
      <section
        id="provenance"
        className="mt-28 scroll-mt-24 border-t border-[var(--border-hairline)] pt-16"
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
