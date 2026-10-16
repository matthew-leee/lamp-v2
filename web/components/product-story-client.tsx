"use client";

import { useState, type FormEvent } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
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

interface ProductStoryClientProps {
  lang: Locale;
  product: CatalogueProduct;
  siblings: CatalogueProduct[];
  totalPieces: number;
  currency: string;
  whatsappNumber: string | null;
  copy: {
    disclosure: string;
    warrantyPending: string;
    referencePending: string;
    enquiryMessage: string;
  };
}

export default function ProductStoryClient({
  lang,
  product,
  siblings,
  totalPieces,
  currency,
  whatsappNumber,
  copy,
}: ProductStoryClientProps) {
  const { lit, setLit, stockMap, addToBag, refreshStock } = useStorefront();
  const router = useRouter();

  const [stageMode, setStageMode] = useState<"photo" | "scale">("photo");
  const [quantity, setQuantity] = useState<number>(1);
  const [expressCheckoutOpen, setExpressCheckoutOpen] = useState<boolean>(false);
  const [paymentMethod, setPaymentMethod] = useState<"card" | "fps">("card");
  const [customerName, setCustomerName] = useState("");
  const [customerEmail, setCustomerEmail] = useState("");
  const [customerPhone, setCustomerPhone] = useState("");
  const [deliveryAddress, setDeliveryAddress] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const localeCopy = product.content[lang] ?? product.content.en ?? {};
  const title = formatProductTitle(
    product.model,
    localeCopy.model_alt,
    localeCopy.colourway,
    lang,
  );
  const badge = formatEditionBadge(product.pieces, totalPieces, lang);

  const stockInfo = stockMap?.[product.slug];
  const avail = stockInfo ? stockInfo.available : null;
  const isSoldOut = avail !== null && avail <= 0;
  const maxSelectable = avail !== null && avail > 0 ? avail : 1;

  const ref = product.reference_price;
  const savings = ref
    ? computeSavingsPercent(
        product.price_cents,
        ref.min_price_cents,
        ref.max_price_cents,
      )
    : null;

  const whatsappHref = buildWhatsAppUrl(whatsappNumber, copy.enquiryMessage);

  const specRows = [
    { label: t(ui.specDesigner, lang), value: product.designer },
    { label: t(ui.specYear, lang), value: product.design_year },
    { label: t(ui.specDimensions, lang), value: product.dimensions },
    { label: t(ui.specLightSource, lang), value: product.light_source },
  ].filter((r) => r.value && r.value.trim() !== "");

  async function handleExpressCheckout(e: FormEvent) {
    e.preventDefault();
    if (submitting || isSoldOut) return;
    setSubmitting(true);
    setErrorMsg(null);
    try {
      const res = await fetch("/api/orders/reserve", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          slug: product.slug,
          quantity,
          payment_method: paymentMethod,
          customer_name: customerName.trim(),
          customer_email: customerEmail.trim(),
          customer_phone: customerPhone.trim(),
          delivery_address: deliveryAddress.trim(),
          locale: lang,
        }),
      });
      const data = (await res.json().catch(() => ({}))) as {
        order_code?: string;
        error?: string;
      };
      if (!res.ok || !data.order_code) {
        setErrorMsg(
          data.error ??
            (lang === "zh-HK"
              ? "未能建立訂單，請檢查輸入資料及存倉狀態。"
              : "Could not create order hold. Please check your details and stock."),
        );
        await refreshStock();
        return;
      }
      await refreshStock();
      router.push(`/${lang}/checkout/${encodeURIComponent(data.order_code)}`);
    } catch {
      setErrorMsg(
        lang === "zh-HK"
          ? "連線異常，請稍後再試。"
          : "Network error. Please try again.",
      );
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="mx-auto max-w-7xl px-5 pb-28 pt-8 sm:px-10">
      {/* Breadcrumb */}
      <div className="flex items-center justify-between border-b border-[var(--border-hairline)] pb-4 text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)]">
        <Link
          href={`/${lang}#collection`}
          className="transition-colors hover:text-[var(--text-primary)]"
        >
          ← {t(ui.backToCollection, lang)}
        </Link>
        <span className="tabular-nums">{badge}</span>
      </div>

      {/* Main Two-Column Architecture */}
      <div className="mt-10 grid items-start gap-12 lg:grid-cols-12 lg:gap-16">
        {/* Left Column: Sticky 3-Mode Interactive Light Studio */}
        <div className="lg:sticky lg:top-24 lg:col-span-7">
          <div className="card-exhibition p-4 sm:p-6">
            <LampStage
              hue={product.art_hue}
              variant={product.art_variant}
              lit={lit}
              imageDayUrl={product.image_day_url}
              imageLitUrl={product.image_lit_url}
              badgeText={badge}
              dimensionsText={product.dimensions}
              mode={stageMode}
              aspectClass="aspect-square"
              altText={title}
            />

            {/* Light Studio Switcher (hides 2700K when single-photo only, hides Scale when art_variant === "none") */}
            {(() => {
              const hasLitPhoto =
                product.image_lit_url.trim() !== "" ||
                (product.image_day_url.trim() === "" &&
                  product.art_variant !== "none");
              const hasScaleTab = product.art_variant !== "none";
              if (!hasLitPhoto && !hasScaleTab) return null;
              return (
                <div
                  className={`mt-4 grid gap-2 ${
                    hasLitPhoto && hasScaleTab ? "grid-cols-3" : "grid-cols-2"
                  }`}
                >
                  <button
                    type="button"
                    onClick={() => {
                      setStageMode("photo");
                      setLit(false);
                    }}
                    aria-pressed={stageMode === "photo" && (!lit || !hasLitPhoto)}
                    className={`border py-2.5 text-xs tracking-[0.14em] uppercase transition-colors ${
                      stageMode === "photo" && (!lit || !hasLitPhoto)
                        ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)] font-medium"
                        : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                    }`}
                  >
                    {t(ui.stageModeDay, lang)}
                  </button>

                  {hasLitPhoto && (
                    <button
                      type="button"
                      onClick={() => {
                        setStageMode("photo");
                        setLit(true);
                      }}
                      aria-pressed={stageMode === "photo" && lit}
                      className={`border py-2.5 text-xs tracking-[0.14em] uppercase transition-colors ${
                        stageMode === "photo" && lit
                          ? "border-[var(--color-glow)] bg-[var(--color-glow)] text-[#11100e] font-semibold"
                          : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                      }`}
                    >
                      {t(ui.stageModeNight, lang)}
                    </button>
                  )}

                  {hasScaleTab && (
                    <button
                      type="button"
                      onClick={() => setStageMode("scale")}
                      aria-pressed={stageMode === "scale"}
                      className={`border py-2.5 text-xs tracking-[0.14em] uppercase transition-colors ${
                        stageMode === "scale"
                          ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)] font-medium"
                          : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                      }`}
                    >
                      {t(ui.stageModeScale, lang)}
                    </button>
                  )}
                </div>
              );
            })()}
          </div>
        </div>

        {/* Right Column: Collector Decision, Swatches, Market Price & Specs */}
        <div className="lg:col-span-5">
          <p className="text-xs tracking-[0.2em] uppercase text-[var(--text-secondary)]">
            {[product.brand, product.designer, product.design_year]
              .filter(Boolean)
              .join(" · ")}
          </p>

          <h1
            lang={htmlLang[lang]}
            className="mt-2 font-display text-3xl leading-snug text-[var(--text-primary)] sm:text-4xl"
          >
            {title}
          </h1>

          {/* Price & Live Stock Header */}
          <div className="mt-6 flex flex-wrap items-baseline justify-between gap-4 border-y border-[var(--border-hairline)] py-5">
            <div>
              <p className="text-[10px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
                {t(ui.housePriceLabel, lang)}
              </p>
              <p className="tabular-nums mt-1 font-sans text-4xl font-semibold tracking-tight text-[var(--text-primary)]">
                {formatMoney(product.price_cents, currency)}
              </p>
            </div>

            <div className="flex items-center gap-2 border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3.5 py-2 text-xs">
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

          {/* Sibling Colourway Swatches (when family has multiple SKUs) */}
          {siblings.length > 1 && (
            <div className="mt-6">
              <p className="text-[11px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
                {t(ui.colourwaysHeading, lang)} ({siblings.length})
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                {(() => {
                  const allSameModel = siblings.every(
                    (s) => s.model === product.model,
                  );
                  return siblings.map((sib) => {
                    const sibCopy = sib.content[lang] ?? sib.content.en ?? {};
                    const sibLabel = allSameModel
                      ? sibCopy.colourway || sib.model
                      : formatProductTitle(
                          sib.model,
                          sibCopy.model_alt,
                          sibCopy.colourway,
                          lang,
                        );
                    const isCurrent = sib.slug === product.slug;
                    return (
                      <Link
                        key={sib.slug}
                        href={`/${lang}/products/${sib.slug}`}
                        aria-current={isCurrent ? "page" : undefined}
                        className={`flex items-center gap-2 border px-3 py-2 text-xs transition-all ${
                          isCurrent
                            ? "border-[var(--color-glow)] bg-[var(--text-primary)] text-[var(--surface-page)] font-medium"
                            : "border-[var(--border-hairline)] bg-[var(--surface-card)] text-[var(--text-primary)] hover:border-[var(--color-glow)]"
                        }`}
                      >
                        <span
                          className="inline-block size-2.5 rounded-full border border-black/20"
                          style={{
                            backgroundColor: `hsl(${sib.art_hue} 65% 65%)`,
                          }}
                        />
                        <span>{sibLabel}</span>
                      </Link>
                    );
                  });
                })()}
              </div>
            </div>
          )}

          {/* Purchase & Instant Checkout Controls */}
          <div className="mt-8 flex flex-col gap-3">
            {!isSoldOut && (
              <div className="flex items-center gap-3">
                <div className="flex items-center border border-[var(--border-hairline)] bg-[var(--surface-card)]">
                  <button
                    type="button"
                    onClick={() => setQuantity((q) => Math.max(1, q - 1))}
                    className="px-3.5 py-3 text-sm hover:bg-[var(--surface-elevated)]"
                    aria-label="Decrease quantity"
                  >
                    −
                  </button>
                  <span className="tabular-nums px-4 text-sm font-medium">
                    {quantity}
                  </span>
                  <button
                    type="button"
                    disabled={quantity >= maxSelectable}
                    onClick={() =>
                      setQuantity((q) => Math.min(maxSelectable, q + 1))
                    }
                    className="px-3.5 py-3 text-sm hover:bg-[var(--surface-elevated)] disabled:opacity-30"
                    aria-label="Increase quantity"
                  >
                    +
                  </button>
                </div>

                <button
                  type="button"
                  onClick={() => addToBag(product.slug, quantity, true)}
                  className="flex-1 border border-[var(--border-strong)] bg-[var(--surface-card)] py-3.5 text-xs font-medium tracking-[0.18em] uppercase text-[var(--text-primary)] transition-colors hover:border-[var(--color-glow)]"
                >
                  {t(ui.addToBag, lang)}
                </button>
              </div>
            )}

            <button
              type="button"
              disabled={isSoldOut}
              onClick={() => setExpressCheckoutOpen((o) => !o)}
              className="w-full bg-[var(--text-primary)] py-4 text-xs font-semibold tracking-[0.22em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-35"
            >
              {isSoldOut
                ? t(ui.stockSoldOut, lang)
                : `${t(ui.buyNow, lang)} · ${formatMoney(product.price_cents * quantity, currency)}`}
            </button>

            <p className="text-[11px] text-[var(--text-secondary)]">
              {t(ui.deliveryNote, lang)}
            </p>
          </div>

          {/* Inline Express Checkout Panel (When "Buy Now" is clicked) */}
          {expressCheckoutOpen && !isSoldOut && (
            <form
              onSubmit={handleExpressCheckout}
              className="mt-5 flex flex-col gap-4 border border-[var(--color-glow)] bg-[var(--surface-card)] p-5"
            >
              <div className="flex items-center justify-between border-b border-[var(--border-hairline)] pb-3">
                <h2 className="font-display text-lg">
                  {t(ui.checkoutTitle, lang)}
                </h2>
                <button
                  type="button"
                  onClick={() => setExpressCheckoutOpen(false)}
                  className="text-xs text-[var(--text-secondary)] underline"
                >
                  {lang === "zh-HK" ? "收起" : "Close"}
                </button>
              </div>

              {errorMsg && (
                <div
                  role="alert"
                  className="border-l-4 border-[var(--color-error)] p-3 text-xs text-[var(--text-primary)]"
                >
                  {errorMsg}
                </div>
              )}

              <div className="grid grid-cols-2 gap-2">
                <button
                  type="button"
                  onClick={() => setPaymentMethod("card")}
                  className={`border py-2.5 text-xs tracking-[0.12em] uppercase ${
                    paymentMethod === "card"
                      ? "border-[var(--color-glow)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                      : "border-[var(--border-hairline)] text-[var(--text-secondary)]"
                  }`}
                >
                  {lang === "zh-HK" ? "信用卡 (Stripe)" : "Credit Card (Stripe)"}
                </button>
                <button
                  type="button"
                  onClick={() => setPaymentMethod("fps")}
                  className={`border py-2.5 text-xs tracking-[0.12em] uppercase ${
                    paymentMethod === "fps"
                      ? "border-[var(--color-glow)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                      : "border-[var(--border-hairline)] text-[var(--text-secondary)]"
                  }`}
                >
                  {lang === "zh-HK" ? "轉數快 (FPS)" : "HK FPS Transfer"}
                </button>
              </div>

              <div className="grid gap-3 sm:grid-cols-2">
                <div className="flex flex-col gap-1">
                  <label htmlFor="pdp-cust-name" className="text-xs text-[var(--text-secondary)]">
                    {lang === "zh-HK" ? "姓名 *" : "Full Name *"}
                  </label>
                  <input
                    id="pdp-cust-name"
                    type="text"
                    required
                    value={customerName}
                    onChange={(e) => setCustomerName(e.target.value)}
                    className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-sm"
                  />
                </div>
                <div className="flex flex-col gap-1">
                  <label htmlFor="pdp-cust-email" className="text-xs text-[var(--text-secondary)]">
                    {lang === "zh-HK" ? "電郵地址 *" : "Email Address *"}
                  </label>
                  <input
                    id="pdp-cust-email"
                    type="email"
                    required
                    value={customerEmail}
                    onChange={(e) => setCustomerEmail(e.target.value)}
                    className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-sm"
                  />
                </div>
              </div>

              <div className="grid gap-3 sm:grid-cols-2">
                <div className="flex flex-col gap-1">
                  <label htmlFor="pdp-cust-phone" className="text-xs text-[var(--text-secondary)]">
                    {lang === "zh-HK" ? "聯絡電話 / WhatsApp *" : "Phone / WhatsApp *"}
                  </label>
                  <input
                    id="pdp-cust-phone"
                    type="tel"
                    required
                    value={customerPhone}
                    onChange={(e) => setCustomerPhone(e.target.value)}
                    className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-sm"
                  />
                </div>
                <div className="flex flex-col gap-1">
                  <label htmlFor="pdp-cust-addr" className="text-xs text-[var(--text-secondary)]">
                    {lang === "zh-HK" ? "香港送遞地址 *" : "HK Delivery Address *"}
                  </label>
                  <input
                    id="pdp-cust-addr"
                    type="text"
                    required
                    value={deliveryAddress}
                    onChange={(e) => setDeliveryAddress(e.target.value)}
                    className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-sm"
                  />
                </div>
              </div>

              <button
                type="submit"
                disabled={submitting}
                className="w-full bg-[var(--color-glow)] py-3.5 text-xs font-semibold tracking-[0.2em] uppercase text-[#11100e] disabled:opacity-50"
              >
                {submitting
                  ? lang === "zh-HK"
                    ? "正在鎖定存倉及開啟付款頁..."
                    : "Locking Stock & Opening Payment Gateway..."
                  : lang === "zh-HK"
                    ? "確認留貨並進入付款頁 →"
                    : "Continue to Payment Settlement →"}
              </button>
            </form>
          )}

          {/* Market Price Transparency Card */}
          <div className="card-exhibition mt-8 p-5">
            <div className="flex items-center justify-between">
              <p className="text-[11px] tracking-[0.18em] uppercase text-[var(--text-secondary)]">
                {t(ui.marketPriceTitle, lang)}
              </p>
              {savings && (
                <span className="bg-[var(--color-glow)] px-2 py-0.5 text-[10px] font-semibold tracking-[0.14em] uppercase text-[#11100e]">
                  {lang === "zh-HK"
                    ? `節省 ${savings.minPct}%–${savings.maxPct}%`
                    : `SAVE ${savings.minPct}%–${savings.maxPct}%`}
                </span>
              )}
            </div>

            {ref ? (
              <div className="mt-4">
                <div className="flex items-baseline justify-between text-xs">
                  <span className="text-[var(--text-secondary)]">
                    {t(ui.dealerRangeLabel, lang)}
                  </span>
                  <span className="tabular-nums font-medium">
                    {formatMoney(ref.min_price_cents, currency)} –{" "}
                    {formatMoney(ref.max_price_cents, currency)}
                  </span>
                </div>

                {/* Visual comparison bar */}
                <div className="mt-2.5 h-2 w-full overflow-hidden bg-[var(--border-hairline)]">
                  <div
                    className="h-full bg-[var(--color-glow)]"
                    style={{
                      width: `${Math.min(
                        95,
                        Math.max(
                          25,
                          Math.round(
                            (product.price_cents / ref.max_price_cents) * 100,
                          ),
                        ),
                      )}%`,
                    }}
                  />
                </div>

                <p className="mt-3 text-[11px] text-[var(--text-secondary)]">
                  {t(ui.verifiedOnLabel, lang)} {ref.checked_on} ·{" "}
                  <a
                    href={ref.source_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    lang="en"
                    className="underline hover:text-[var(--text-primary)]"
                  >
                    {ref.source_name} ↗
                  </a>
                </p>
              </div>
            ) : (
              <p
                lang={htmlLang[lang]}
                className="mt-3 text-xs leading-relaxed text-[var(--text-secondary)]"
              >
                {copy.referencePending}
              </p>
            )}
          </div>

          {/* Swiss-Grid Architectural Specifications */}
          {specRows.length > 0 && (
            <dl className="mt-8 grid grid-cols-2 gap-x-6 gap-y-4 border-t border-[var(--border-hairline)] pt-6">
              {specRows.map((row) => (
                <div key={row.label}>
                  <dt className="text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                    {row.label}
                  </dt>
                  <dd
                    lang="en"
                    className="tabular-nums mt-1 text-sm text-[var(--text-primary)]"
                  >
                    {row.value}
                  </dd>
                </div>
              ))}
            </dl>
          )}

          {/* Electrical, Warranty & Provenance Accordions/Blocks */}
          <dl className="mt-8 space-y-6 border-t border-[var(--border-hairline)] pt-6 text-sm">
            {localeCopy.electrical && (
              <div>
                <dt className="text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                  {t(ui.specElectrical, lang)}
                </dt>
                <dd
                  lang={htmlLang[lang]}
                  className="mt-1.5 leading-relaxed text-[var(--text-primary)]"
                >
                  {localeCopy.electrical}
                </dd>
              </div>
            )}

            <div>
              <dt className="text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                {t(ui.specWarranty, lang)}
              </dt>
              <dd
                lang={htmlLang[lang]}
                className="mt-1.5 leading-relaxed text-[var(--text-primary)]"
              >
                {copy.warrantyPending}
              </dd>
            </div>

            <div>
              <dt className="text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                {t(ui.specDisclosure, lang)}
              </dt>
              <dd
                lang={htmlLang[lang]}
                className="mt-1.5 leading-relaxed text-[var(--text-primary)]"
              >
                {copy.disclosure}
              </dd>
            </div>
          </dl>

          {whatsappHref && (
            <div className="mt-8 border-t border-[var(--border-hairline)] pt-6">
              <a
                href={whatsappHref}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center border border-[var(--border-hairline)] bg-[var(--surface-card)] px-5 py-3 text-xs tracking-[0.16em] uppercase text-[var(--text-primary)] hover:border-[var(--color-glow)]"
              >
                {t(ui.conciergeCta, lang)} ↗
              </a>
            </div>
          )}
        </div>
      </div>

      {/* Below the Fold: Architectural Heritage Story */}
      {localeCopy.heritage && (
        <section className="mt-24 border-t border-[var(--border-hairline)] pt-14">
          <p className="text-xs tracking-[0.22em] uppercase text-[var(--color-glow)]">
            {t(ui.specHeritage, lang)}
          </p>
          <p
            lang={htmlLang[lang]}
            className="mt-5 max-w-4xl font-display text-2xl leading-relaxed text-[var(--text-primary)] sm:text-3xl"
          >
            {localeCopy.heritage}
          </p>
        </section>
      )}
    </div>
  );
}
