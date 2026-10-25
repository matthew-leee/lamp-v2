"use client";

import { useEffect, useState, type FormEvent } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useStorefront } from "./storefront-context";
import LampStage from "./lamp-stage";
import { formatEditionBadge, formatMoney, formatProductTitle } from "@/lib/format";
import {
  clearPendingAttemptKey,
  getOrCreatePendingAttemptKey,
  pendingAttemptFingerprint,
  saveRecentOrder,
} from "@/lib/recent-order";
import { t, ui, type Locale } from "@/lib/i18n";

export default function BagDrawer({
  lang,
  totalDropPieces,
}: {
  lang: Locale;
  totalDropPieces: number;
}) {
  const {
    lit,
    bag,
    bagOpen,
    setBagOpen,
    addToBag,
    updateBagQuantity,
    removeFromBag,
    clearBag,
    catalogueProducts,
    stockMap,
    refreshStock,
    currency,
  } = useStorefront();

  const router = useRouter();
  const [checkoutStep, setCheckoutStep] = useState(false);
  const [paymentMethod, setPaymentMethod] = useState<"card" | "fps">("card");
  const [customerName, setCustomerName] = useState("");
  const [customerEmail, setCustomerEmail] = useState("");
  const [customerPhone, setCustomerPhone] = useState("");
  const [deliveryAddress, setDeliveryAddress] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Mobile ergonomics: a slide-over drawer must close on Escape like any other
  // dialog. Registered unconditionally (before the `!bagOpen` early return) so
  // the hook order never changes between renders.
  useEffect(() => {
    if (!bagOpen) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setBagOpen(false);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [bagOpen, setBagOpen]);

  if (!bagOpen) return null;

  const populatedItems = bag
    .map((item) => {
      const product = catalogueProducts.find((p) => p.slug === item.slug);
      if (!product) return null;
      const maxAvail = stockMap?.[item.slug]?.available ?? product.units_in_drop ?? 1;
      return {
        ...item,
        product,
        maxAvail,
        lineCents: product.price_cents * item.quantity,
      };
    })
    .filter((x): x is NonNullable<typeof x> => x !== null);

  const subtotalCents = populatedItems.reduce((sum, i) => sum + i.lineCents, 0);

  async function handleCheckoutSubmit(e: FormEvent) {
    e.preventDefault();
    if (populatedItems.length === 0 || submitting) return;
    setSubmitting(true);
    setErrorMsg(null);

    // One order per pay tap (findings G01): the DURABLE identity of THIS pay
    // attempt. The same cart + payment method + email keeps the same
    // `idempotency_key` across a refresh, a lost-response retry and even a
    // second browser tab, so the API answers with the FIRST order instead of
    // a second ghost order holding stock.
    const attemptFingerprint = pendingAttemptFingerprint(
      populatedItems.map((i) => ({ slug: i.slug, quantity: i.quantity })),
      paymentMethod,
      customerEmail,
    );
    const idempotencyKey = getOrCreatePendingAttemptKey(attemptFingerprint);

    try {
      const itemsPayload = populatedItems.map((i) => ({
        slug: i.slug,
        quantity: i.quantity,
      }));
      const primary = populatedItems[0]!;
      const res = await fetch("/api/orders/reserve", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          slug: primary.slug,
          quantity: primary.quantity,
          items: itemsPayload,
          payment_method: paymentMethod,
          customer_name: customerName.trim(),
          customer_email: customerEmail.trim(),
          customer_phone: customerPhone.trim(),
          delivery_address: deliveryAddress.trim(),
          locale: lang,
          idempotency_key: idempotencyKey,
          // CART-01 price-change detection: the total the customer actually
          // SAW in the cart. If an admin changed a price in the meantime the
          // API answers 409 instead of charging a different amount.
          expected_total_cents: subtotalCents,
        }),
      });

      const data = (await res.json().catch(() => ({}))) as {
        order_code?: string;
        created_at?: string;
        stripe_checkout_url?: string | null;
        error?: string;
      };

      if (!res.ok || !data.order_code) {
        setErrorMsg(
          data.error ??
            (lang === "zh-HK"
              ? "未能建立留貨訂單，請檢查存倉數量或輸入資料。"
              : "Could not reserve items. Please verify stock availability and your details."),
        );
        await refreshStock();
        return;
      }

      // Remember the order for one-click recovery, retire THIS pay-attempt
      // identity (the next tap is a new order), clear all reserved cart items
      // and go straight to payment.
      saveRecentOrder(data.order_code, data.created_at ?? "");
      clearPendingAttemptKey(attemptFingerprint);
      clearBag();
      setCheckoutStep(false);
      setBagOpen(false);
      await refreshStock();
      router.push(`/${lang}/checkout/${encodeURIComponent(data.order_code)}`);
    } catch {
      setErrorMsg(
        lang === "zh-HK"
          ? "網絡連線異常，請稍後再試。"
          : "Network error while creating reservation. Please try again.",
      );
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t(ui.bagTitle, lang)}
      className="no-print fixed inset-0 z-50 flex justify-end bg-black/60 backdrop-blur-xs transition-opacity"
      onClick={(e) => {
        if (e.target === e.currentTarget) setBagOpen(false);
      }}
    >
      {/* `@container` so the empty-bag discovery grid responds to the DRAWER
          width (max-w-lg) rather than the viewport, on any phone. */}
      <div className="@container flex h-full w-full max-w-lg flex-col justify-between border-l border-[var(--border-hairline)] bg-[var(--surface-page)] p-4 pb-[max(1.25rem,env(safe-area-inset-bottom))] text-[var(--text-primary)] shadow-2xl sm:p-8">
        {/* Top bar */}
        <div className="flex items-center justify-between border-b border-[var(--border-hairline)] pb-5">
          <div>
            <p className="text-[10px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
              {lang === "zh-HK" ? "香港現貨典藏" : "HONG KONG VAULT EDITION"}
            </p>
            <h2 className="mt-1 font-display text-2xl">{t(ui.bagTitle, lang)}</h2>
          </div>
          <button
            type="button"
            onClick={() => setBagOpen(false)}
            className="min-h-10 border border-[var(--border-hairline)] px-3.5 py-2 text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)] transition-colors hover:border-[var(--text-primary)] hover:text-[var(--text-primary)]"
          >
            {lang === "zh-HK" ? "關閉" : "Close"}
          </button>
        </div>

        {/* Body */}
        <div className="scroll-contain my-6 flex-1 overflow-y-auto pr-1">
          {populatedItems.length === 0 ? (
            /* Awwwards 'Outfit' Pattern: Curated Empty Bag Discovery */
            <div className="flex flex-col gap-6 py-4">
              <div className="border border-[var(--border-hairline)] bg-[var(--surface-card)] p-5">
                <p className="font-display text-lg">{t(ui.bagEmptyTitle, lang)}</p>
                <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
                  {t(ui.bagEmptySubtitle, lang)}
                </p>
              </div>
              <div className="grid grid-cols-1 gap-4 @[22rem]:grid-cols-2">
                {catalogueProducts.slice(0, 4).map((prod) => {
                  const copy = prod.content[lang] ?? {};
                  const name = formatProductTitle(
                    prod.model,
                    copy.model_alt,
                    copy.colourway,
                    lang,
                  );
                  const avail = stockMap?.[prod.slug]?.available ?? 1;
                  return (
                    <div
                      key={prod.slug}
                      className="card-exhibition row-span-3 grid grid-rows-subgrid gap-y-2 p-3"
                    >
                      <Link
                        href={`/${lang}/products/${prod.slug}`}
                        onClick={() => setBagOpen(false)}
                        className="group block"
                      >
                        <LampStage
                          hue={prod.art_hue}
                          variant={prod.art_variant}
                          lit={lit}
                          imageDayUrl={prod.image_day_url}
                          imageLitUrl={prod.image_lit_url}
                          aspectClass="aspect-square"
                          compact
                        />
                      </Link>
                      <div>
                        <Link
                          href={`/${lang}/products/${prod.slug}`}
                          onClick={() => setBagOpen(false)}
                          className="block text-xs font-medium leading-snug hover:underline"
                        >
                          {name}
                        </Link>
                        <p className="tabular-nums mt-0.5 text-xs text-[var(--text-secondary)]">
                          {formatMoney(prod.price_cents, currency)}
                        </p>
                      </div>
                      <button
                        type="button"
                        disabled={avail <= 0}
                        onClick={() => addToBag(prod.slug, 1, false)}
                        className="min-h-11 self-end w-full border border-[var(--border-hairline)] py-1.5 text-[11px] tracking-[0.14em] uppercase transition-colors hover:border-[var(--color-glow)] disabled:opacity-40"
                      >
                        {avail > 0
                          ? t(ui.addToBag, lang)
                          : t(ui.stockSoldOut, lang)}
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          ) : !checkoutStep ? (
            <div className="flex flex-col gap-5">
              {populatedItems.map(({ slug, quantity, product, maxAvail, lineCents }) => {
                const copy = product.content[lang] ?? {};
                const title = formatProductTitle(
                  product.model,
                  copy.model_alt,
                  copy.colourway,
                  lang,
                );
                const badge = formatEditionBadge(
                  product.pieces,
                  totalDropPieces,
                  lang,
                );
                return (
                  // `@container` per line item: the stepper + line total row
                  // measures THIS card's own inline width and stacks below
                  // 22rem, so a 320px phone never overflows.
                  <div
                    key={slug}
                    className="@container flex gap-4 border border-[var(--border-hairline)] bg-[var(--surface-card)] p-4"
                  >
                    <div className="w-20 shrink-0">
                      <LampStage
                        hue={product.art_hue}
                        variant={product.art_variant}
                        lit={lit}
                        imageDayUrl={product.image_day_url}
                        imageLitUrl={product.image_lit_url}
                        aspectClass="aspect-square"
                        compact
                      />
                    </div>
                    <div className="flex min-w-0 flex-1 flex-col justify-between">
                      <div className="flex items-start justify-between gap-2">
                        <div>
                          <p className="text-[10px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                            {badge}
                          </p>
                          <Link
                            href={`/${lang}/products/${slug}`}
                            onClick={() => setBagOpen(false)}
                            className="mt-0.5 block text-sm font-medium hover:underline"
                          >
                            {title}
                          </Link>
                        </div>
                        <button
                          type="button"
                          onClick={() => removeFromBag(slug)}
                          className="inline-flex min-h-9 shrink-0 items-center justify-center whitespace-nowrap px-2.5 text-xs text-[var(--text-secondary)] hover:text-[var(--color-error)]"
                        >
                          {lang === "zh-HK" ? "移除" : "Remove"}
                        </button>
                      </div>

                      <div className="mt-3 flex flex-col items-start gap-2 @[22rem]:flex-row @[22rem]:items-center @[22rem]:justify-between">
                        <div className="flex items-center border border-[var(--border-hairline)]">
                          <button
                            type="button"
                            onClick={() => updateBagQuantity(slug, quantity - 1)}
                            className="min-h-9 min-w-9 px-2.5 py-1.5 text-xs hover:bg-[var(--surface-elevated)]"
                            aria-label="Decrease quantity"
                          >
                            −
                          </button>
                          <span className="tabular-nums px-3 text-xs">
                            {quantity}
                          </span>
                          <button
                            type="button"
                            disabled={quantity >= maxAvail}
                            onClick={() => updateBagQuantity(slug, quantity + 1)}
                            className="min-h-9 min-w-9 px-2.5 py-1.5 text-xs hover:bg-[var(--surface-elevated)] disabled:opacity-30"
                            aria-label="Increase quantity"
                          >
                            +
                          </button>
                        </div>
                        <p className="tabular-nums text-sm font-medium">
                          {formatMoney(lineCents, currency)}
                        </p>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            /* Step 2 inside Drawer: Customer Details & Payment Method Selection */
            <form
              id="bag-checkout-form"
              onSubmit={handleCheckoutSubmit}
              className="flex flex-col gap-4"
            >
              <button
                type="button"
                onClick={() => setCheckoutStep(false)}
                className="self-start text-xs text-[var(--text-secondary)] underline"
              >
                ← {lang === "zh-HK" ? "返回購物車清單" : "Back to cart items"}
              </button>

              {errorMsg && (
                <div
                  role="alert"
                  className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-3 text-xs text-[var(--text-primary)]"
                >
                  {errorMsg}
                </div>
              )}

              <fieldset className="flex flex-col gap-2 border border-[var(--border-hairline)] bg-[var(--surface-card)] p-4">
                <legend className="px-1 text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "選擇付款及交收方式" : "Select Settlement Method"}
                </legend>
                <label className="flex cursor-pointer items-center gap-3 text-xs">
                  <input
                    type="radio"
                    name="payment_method"
                    checked={paymentMethod === "card"}
                    onChange={() => setPaymentMethod("card")}
                  />
                  <span>
                    {lang === "zh-HK"
                      ? "信用卡即時付款（Stripe · 30 分鐘留貨保障）"
                      : "Credit Card via Stripe (Instant Settlement · 30m Hold)"}
                  </span>
                </label>
                <label className="mt-1 flex cursor-pointer items-center gap-3 text-xs">
                  <input
                    type="radio"
                    name="payment_method"
                    checked={paymentMethod === "fps"}
                    onChange={() => setPaymentMethod("fps")}
                  />
                  <span>
                    {lang === "zh-HK"
                      ? "香港轉數快 FPS（24 小時留貨 · 免手續費）"
                      : "Hong Kong FPS Faster Payment (24h Hold · Zero Fee)"}
                  </span>
                </label>
              </fieldset>

              <div className="flex flex-col gap-1">
                <label htmlFor="bag-cust-name" className="text-xs text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "藏家姓名 *" : "Collector Full Name *"}
                </label>
                <input
                  id="bag-cust-name"
                  type="text"
                  required
                  value={customerName}
                  onChange={(e) => setCustomerName(e.target.value)}
                  placeholder={lang === "zh-HK" ? "例如：Chan Tai Man" : "e.g. Alex Chan"}
                  className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3 py-2 text-base text-[var(--text-primary)] sm:text-sm"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="bag-cust-email" className="text-xs text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "電郵地址（接收訂單確認書及收據）*" : "Email Address (for Order Pass & Receipt) *"}
                </label>
                <input
                  id="bag-cust-email"
                  type="email"
                  required
                  value={customerEmail}
                  onChange={(e) => setCustomerEmail(e.target.value)}
                  placeholder="collector@example.com"
                  className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3 py-2 text-base text-[var(--text-primary)] sm:text-sm"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="bag-cust-phone" className="text-xs text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "WhatsApp / 聯絡電話 *" : "WhatsApp / Mobile Number *"}
                </label>
                <input
                  id="bag-cust-phone"
                  type="tel"
                  required
                  value={customerPhone}
                  onChange={(e) => setCustomerPhone(e.target.value)}
                  placeholder="+852 9123 4567"
                  className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3 py-2 text-base text-[var(--text-primary)] sm:text-sm"
                />
              </div>

              <div className="flex flex-col gap-1">
                <label htmlFor="bag-cust-addr" className="text-xs text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "香港送遞地址 *" : "Hong Kong Delivery Address *"}
                </label>
                <textarea
                  id="bag-cust-addr"
                  rows={2}
                  required
                  value={deliveryAddress}
                  onChange={(e) => setDeliveryAddress(e.target.value)}
                  placeholder={
                    lang === "zh-HK"
                      ? "中環 / 九龍 / 新界詳細地址"
                      : "Flat, Floor, Building, Street, District, Hong Kong"
                  }
                  className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3 py-2 text-base text-[var(--text-primary)] sm:text-sm"
                />
              </div>
            </form>
          )}
        </div>

        {/* Footer */}
        {populatedItems.length > 0 && (
          <div className="border-t border-[var(--border-hairline)] pt-5">
            <div className="flex items-baseline justify-between">
              <span className="text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                {t(ui.bagSubtotal, lang)}
              </span>
              <span className="tabular-nums font-display text-2xl">
                {formatMoney(subtotalCents, currency)}
              </span>
            </div>
            <p className="mt-1 text-[11px] text-[var(--text-secondary)]">
              {t(ui.deliveryNote, lang)}
            </p>

            {!checkoutStep ? (
              <div className="mt-4 flex flex-col gap-2">
                <button
                  type="button"
                  onClick={() => setCheckoutStep(true)}
                  className="min-h-11 w-full bg-[var(--text-primary)] py-3.5 text-xs font-medium tracking-[0.2em] uppercase text-[var(--surface-page)] transition-opacity hover:opacity-90"
                >
                  {t(ui.bagProceedCheckout, lang)}
                </button>
                <button
                  type="button"
                  onClick={clearBag}
                  className="w-full py-1.5 text-[11px] text-[var(--text-secondary)] underline"
                >
                  {t(ui.bagClear, lang)}
                </button>
              </div>
            ) : (
              <button
                type="submit"
                form="bag-checkout-form"
                disabled={submitting}
                className="mt-4 min-h-11 w-full bg-[var(--color-glow)] py-3.5 text-xs font-semibold tracking-[0.2em] uppercase text-[var(--text-on-glow)] transition-opacity hover:opacity-90 disabled:opacity-50"
              >
                {submitting
                  ? lang === "zh-HK"
                    ? "正在鎖定存倉及前往付款..."
                    : "Locking Vault Piece & Opening Gateway..."
                  : lang === "zh-HK"
                    ? "鎖定作品並前往付款交收 →"
                    : "Lock Piece & Continue to Payment →"}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
