"use client";

import { useEffect, useState, type FormEvent } from "react";
import Link from "next/link";
import { useStorefront } from "./storefront-context";
import LampStage from "./lamp-stage";
import type { CatalogueProduct } from "@/lib/api";
import { formatEditionBadge, formatMoney, formatProductTitle } from "@/lib/format";
import { htmlLang, t, ui, type Locale } from "@/lib/i18n";

export interface PublicOrderItemData {
  product_id: number;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  line_total_cents: number;
}

export interface PublicOrderData {
  order_code: string;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  total_cents: number;
  currency: string;
  payment_method: string;
  status: string;
  fps_reference: string;
  expires_at: string;
  created_at: string;
  items?: PublicOrderItemData[];
  email_receipt?: {
    subject: string;
    html: string;
  } | null;
}

interface CheckoutSettlementClientProps {
  lang: Locale;
  initialOrder: PublicOrderData;
  product: CatalogueProduct | null;
  allProducts?: CatalogueProduct[];
  totalPieces: number;
  brandName: string;
  whatsappNumber: string | null;
  fpsId?: string | null;
  fpsAccountName?: string | null;
  fpsQrUrl?: string | null;
  stripePublishableKey?: string | null;
  stripeRedirectConfigured?: boolean;
  returnSessionId?: string | null;
}

function formatRemainingTime(expiresAtIso: string): {
  expired: boolean;
  label: string;
} {
  const diffMs = new Date(expiresAtIso).getTime() - Date.now();
  if (!Number.isFinite(diffMs) || diffMs <= 0) {
    return { expired: true, label: "00:00" };
  }
  const totalSec = Math.floor(diffMs / 1000);
  const hrs = Math.floor(totalSec / 3600);
  const mins = Math.floor((totalSec % 3600) / 60);
  const secs = totalSec % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  if (hrs > 0) {
    return { expired: false, label: `${pad(hrs)}:${pad(mins)}:${pad(secs)}` };
  }
  return { expired: false, label: `${pad(mins)}:${pad(secs)}` };
}

export default function CheckoutSettlementClient({
  lang,
  initialOrder,
  product,
  allProducts = [],
  totalPieces,
  brandName,
  fpsId = null,
  fpsAccountName = null,
  fpsQrUrl = null,
  stripePublishableKey = null,
  stripeRedirectConfigured = false,
  returnSessionId = null,
}: CheckoutSettlementClientProps) {
  const { lit, refreshStock } = useStorefront();
  const [order, setOrder] = useState<PublicOrderData>(initialOrder);
  const [countdown, setCountdown] = useState(() =>
    formatRemainingTime(initialOrder.expires_at),
  );

  // Card payment inputs
  const [cardholderName, setCardholderName] = useState("ALEX CHAN");
  const [cardNumber, setCardNumber] = useState("");
  const [expiry, setExpiry] = useState("");
  const [cvc, setCvc] = useState("");
  const [paying, setPaying] = useState(false);
  const [paymentError, setPaymentError] = useState<string | null>(null);

  // FPS reference input
  const [fpsRefInput, setFpsRefInput] = useState(initialOrder.fps_reference || "");
  const [fpsSubmittedMsg, setFpsSubmittedMsg] = useState<string | null>(null);

  // Email preview modal
  const [showEmailModal, setShowEmailModal] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // Two-step inline confirmation (G23) for the destructive self-service
  // cancel: the first click only arms the action, the second executes it.
  const [confirmCancel, setConfirmCancel] = useState(false);

  useEffect(() => {
    const timer = setInterval(() => {
      setCountdown(formatRemainingTime(order.expires_at));
    }, 1000);
    return () => clearInterval(timer);
  }, [order.expires_at]);

  // STRIPE-01: if returning from a hosted Stripe Checkout session with ?session_id=..., verify immediately
  useEffect(() => {
    if (!returnSessionId || order.status === "paid" || order.status === "fulfilled") return;
    let active = true;
    (async () => {
      setPaying(true);
      try {
        const res = await fetch(
          `/api/orders/${encodeURIComponent(order.order_code)}/pay-card`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              mode: "verify_session",
              sessionId: returnSessionId,
              locale: lang,
            }),
          },
        );
        if (!res.ok && active) {
          const data = (await res.json().catch(() => ({}))) as { error?: string };
          setPaymentError(
            data.error ??
              (lang === "zh-HK"
                ? "未能核實 Stripe 付款狀態。"
                : "Could not verify Stripe payment session."),
          );
        }
        if (active) {
          await reloadOrder();
        }
      } finally {
        if (active) setPaying(false);
      }
    })();
    return () => {
      active = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [returnSessionId]);

  const orderItems: PublicOrderItemData[] =
    order.items && order.items.length > 0
      ? order.items
      : [
          {
            product_id: 0,
            product_slug: order.product_slug,
            quantity: order.quantity,
            unit_price_cents: order.unit_price_cents,
            line_total_cents: order.total_cents,
          },
        ];

  const enrichedItems = orderItems.map((it) => {
    const found =
      allProducts.find((p) => p.slug === it.product_slug) ??
      (product?.slug === it.product_slug ? product : null);
    // LANG-01: strictly locale-specific copy — `zh-HK` never falls back to
    // English product copy.
    const copy = found?.content[lang] ?? {};
    const title = found
      ? formatProductTitle(found.model, copy.model_alt, copy.colourway, lang)
      : it.product_slug;
    const badge = found
      ? formatEditionBadge(found.pieces, totalPieces, lang)
      : "";
    return {
      ...it,
      product: found,
      title,
      badge,
    };
  });

  const statusEntry =
    ui.orderStatusMap[order.status] ?? ui.orderStatusMap.reserved!;
  const isPaid = order.status === "paid" || order.status === "fulfilled";
  const isReserved = order.status === "reserved";

  async function reloadOrder() {
    const res = await fetch(
      `/api/orders/${encodeURIComponent(order.order_code)}?locale=${lang}`,
      { cache: "no-store" },
    );
    if (res.ok) {
      const updated = (await res.json()) as PublicOrderData;
      setOrder(updated);
    }
    await refreshStock();
  }

  async function handleHostedStripeCheckout() {
    if (paying) return;
    setPaying(true);
    setPaymentError(null);
    try {
      const res = await fetch(
        `/api/orders/${encodeURIComponent(order.order_code)}/pay-card`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            mode: "stripe_checkout",
            locale: lang,
          }),
        },
      );
      const data = (await res.json().catch(() => ({}))) as {
        checkout_url?: string;
        error?: string;
      };
      if (!res.ok || !data.checkout_url) {
        setPaymentError(
          data.error ??
            (lang === "zh-HK"
              ? "未能建立 Stripe 結帳頁面，請使用下方信用卡表格。"
              : "Could not open hosted Stripe Checkout. Please use the card form below."),
        );
        return;
      }
      window.location.href = data.checkout_url;
    } catch {
      setPaymentError(
        lang === "zh-HK"
          ? "Stripe 連線異常，請重試。"
          : "Stripe connection error. Please retry.",
      );
    } finally {
      setPaying(false);
    }
  }

  async function handleCardPayment(e: FormEvent) {
    e.preventDefault();
    if (paying) return;
    setPaying(true);
    setPaymentError(null);
    try {
      const res = await fetch(
        `/api/orders/${encodeURIComponent(order.order_code)}/pay-card`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            mode: "direct",
            locale: lang,
            cardholderName,
            cardNumber,
            expiry,
            cvc,
          }),
        },
      );
      const data = (await res.json().catch(() => ({}))) as {
        error?: string;
      };
      if (!res.ok) {
        setPaymentError(
          data.error ??
            (lang === "zh-HK"
              ? "付款被拒絕，請檢查信用卡資料。"
              : "Card payment failed. Please check your card details."),
        );
        return;
      }
      await reloadOrder();
    } catch {
      setPaymentError(
        lang === "zh-HK"
          ? "付款閘道連線異常，請重試。"
          : "Payment gateway connection error. Please retry.",
      );
    } finally {
      setPaying(false);
    }
  }

  async function handleFpsSubmit(e: FormEvent) {
    e.preventDefault();
    if (!fpsRefInput.trim()) return;
    setPaying(true);
    setPaymentError(null);
    setFpsSubmittedMsg(null);
    try {
      const res = await fetch(
        `/api/orders/${encodeURIComponent(order.order_code)}/submit-fps`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ fps_reference: fpsRefInput.trim() }),
        },
      );
      const data = (await res.json().catch(() => ({}))) as { error?: string };
      if (!res.ok) {
        setPaymentError(
          data.error ??
            (lang === "zh-HK"
              ? "未能儲存轉數快編號。"
              : "Could not save FPS reference."),
        );
        return;
      }
      await reloadOrder();
      setFpsSubmittedMsg(
        lang === "zh-HK"
          ? "已成功登記轉數快參考編號，香港專員核實到帳後將立即確認訂單。"
          : "FPS transfer reference recorded. Our Hong Kong team will verify and confirm your order shortly.",
      );
    } finally {
      setPaying(false);
    }
  }

  async function handleCancelHold() {
    if (cancelling) return;
    if (!confirmCancel) {
      setConfirmCancel(true);
      return;
    }
    setConfirmCancel(false);
    setCancelling(true);
    try {
      const res = await fetch(
        `/api/orders/${encodeURIComponent(order.order_code)}/cancel`,
        { method: "POST" },
      );
      if (res.ok) {
        await reloadOrder();
      }
    } finally {
      setCancelling(false);
    }
  }

  function autofillSuccessCard() {
    setCardholderName("ALEX CHAN");
    setCardNumber("4242 4242 4242 4242");
    setExpiry("12/29");
    setCvc("888");
    setPaymentError(null);
  }

  function autofillDeclineCard() {
    setCardholderName("ALEX CHAN");
    setCardNumber("4000 0000 0000 0002");
    setExpiry("12/29");
    setCvc("888");
    setPaymentError(null);
  }

  const displayFpsId =
    fpsId ??
    (lang === "zh-HK" ? "請向專員查詢或於後台設定" : "Pending Admin Configuration");
  const displayFpsAccount =
    fpsAccountName ?? brandName;

  return (
    <div className="mx-auto max-w-6xl px-5 pb-28 pt-10 sm:px-10">
      {/* Top Bar */}
      <div className="no-print flex flex-wrap items-center justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
        <Link
          href={`/${lang}`}
          className="text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
        >
          ← {t(ui.backToCollection, lang)}
        </Link>
        <div className="flex flex-wrap items-center gap-3">
          {order.email_receipt && (
            <button
              type="button"
              onClick={() => setShowEmailModal(true)}
              className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3.5 py-2 text-xs tracking-[0.14em] uppercase text-[var(--text-primary)] hover:border-[var(--color-glow)]"
            >
              ✉ {lang === "zh-HK" ? "查看已發送確認電郵" : "View Sent Email Notification"}
            </button>
          )}
          <button
            type="button"
            onClick={() => window.print()}
            className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-3.5 py-2 text-xs tracking-[0.14em] uppercase text-[var(--text-primary)] hover:border-[var(--color-glow)]"
          >
            ⎙ {lang === "zh-HK" ? "列印典藏訂單證書" : "Print Official Order Pass"}
          </button>
        </div>
      </div>

      {/* Email Notification Sent Banner */}
      <div className="no-print mt-6 flex flex-wrap items-center justify-between gap-4 border border-[var(--border-hairline)] bg-[var(--surface-card)] px-5 py-3.5 text-xs">
        <div className="flex items-center gap-3">
          <span className="inline-block size-2 rounded-full bg-[var(--color-glow)]" />
          <span>
            {lang === "zh-HK"
              ? `系統已為訂單 ${order.order_code} 生成並發送電郵通知書。`
              : `Transactional email notification dispatched for Order ${order.order_code}.`}
          </span>
        </div>
        {order.email_receipt && (
          <button
            type="button"
            onClick={() => setShowEmailModal(true)}
            className="font-medium text-[var(--color-glow)] underline"
          >
            {lang === "zh-HK" ? "預覽電郵內容 →" : "Inspect Email Receipt →"}
          </button>
        )}
      </div>

      {/* Main Two-Column Checkout & Collector Pass Layout */}
      <div className="mt-8 grid items-start gap-10 lg:grid-cols-12">
        {/* Left Column: Payment Settlement or Paid Collector Certificate */}
        <div className="lg:col-span-7">
          <div className="card-exhibition p-6 sm:p-8">
            <div className="flex flex-wrap items-center justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
              <div>
                <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
                  {brandName} · {lang === "zh-HK" ? "香港典藏庫" : "HONG KONG VAULT"}
                </p>
                <h1
                  lang={htmlLang[lang]}
                  className="mt-1 font-display text-2xl sm:text-3xl"
                >
                  {isPaid
                    ? lang === "zh-HK"
                      ? "付款已確認 · 典藏證書"
                      : "Payment Confirmed · Collector's Pass"
                    : t(ui.checkoutTitle, lang)}
                </h1>
              </div>

              <span
                className={`border px-3 py-1 text-xs font-medium tracking-[0.14em] uppercase ${
                  isPaid
                    ? "border-[var(--color-success)] bg-[var(--color-success)] text-white"
                    : isReserved
                      ? "border-[var(--color-glow)] text-[var(--color-glow)]"
                      : "border-[var(--border-hairline)] text-[var(--text-secondary)]"
                }`}
              >
                {t(statusEntry, lang)}
              </span>
            </div>

            {/* Hold Countdown Banner when Reserved */}
            {isReserved && (
              <div className="mt-6 flex flex-wrap items-center justify-between gap-4 border border-[var(--color-glow)]/50 bg-[var(--surface-elevated)] p-4">
                <div>
                  <p className="text-xs font-medium text-[var(--text-primary)]">
                    {lang === "zh-HK"
                      ? "存倉已為閣下原子鎖定（防止超賣）"
                      : "Vault Stock Atomically Locked for Your Order"}
                  </p>
                  <p className="mt-0.5 text-[11px] text-[var(--text-secondary)]">
                    {lang === "zh-HK"
                      ? "請於倒數結束前完成付款，逾時配額將自動釋放予其他藏家。"
                      : "Complete settlement before the hold timer expires to secure your pieces."}
                  </p>
                </div>
                <div className="tabular-nums border border-[var(--border-hairline)] bg-[var(--surface-card)] px-4 py-2 font-mono text-lg font-semibold text-[var(--color-glow)]">
                  {countdown.label}
                </div>
              </div>
            )}

            {/* CASE 1: PAID / FULFILLED -> Official Collector Certificate */}
            {isPaid && (
              <div className="mt-8 space-y-6">
                <div className="border-2 border-[var(--color-glow)] bg-[var(--surface-elevated)] p-6">
                  <p className="text-[11px] tracking-[0.24em] uppercase text-[var(--color-glow)]">
                    {lang === "zh-HK"
                      ? "官方編號典藏認購證明"
                      : "OFFICIAL SERIALIZED EDITION PASS"}
                  </p>
                  <div className="mt-3 space-y-2">
                    {enrichedItems.map((it) => (
                      <div
                        key={it.product_slug}
                        className="flex items-baseline justify-between gap-4"
                      >
                        <div>
                          <h2 className="font-display text-xl">{it.title}</h2>
                          {it.badge && (
                            <p className="text-[11px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                              {it.badge}
                            </p>
                          )}
                        </div>
                        <span className="tabular-nums font-mono text-sm">
                          × {it.quantity}
                        </span>
                      </div>
                    ))}
                  </div>

                  <dl className="mt-6 grid grid-cols-2 gap-4 border-t border-[var(--border-hairline)] pt-4 text-xs">
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "訂單編號" : "Order Reference"}
                      </dt>
                      <dd className="tabular-nums mt-1 font-mono text-sm font-semibold">
                        {order.order_code}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "已付金額" : "Settled Amount"}
                      </dt>
                      <dd className="tabular-nums mt-1 font-sans text-sm font-semibold">
                        {formatMoney(order.total_cents, order.currency)}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "付款方式" : "Payment Method"}
                      </dt>
                      <dd className="mt-1 uppercase">{order.payment_method}</dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "交收狀態" : "Fulfilment Status"}
                      </dt>
                      <dd className="mt-1">{t(statusEntry, lang)}</dd>
                    </div>
                  </dl>
                </div>

                <p className="text-xs leading-relaxed text-[var(--text-secondary)]">
                  {t(ui.deliveryNote, lang)}
                </p>
              </div>
            )}

            {/* CASE 2: RESERVED + CARD -> Hosted Stripe Checkout or Local Test Card Gateway */}
            {isReserved && order.payment_method === "card" && (
              <div className="no-print mt-8 space-y-6">
                {stripeRedirectConfigured ? (
                  <div className="border border-[var(--color-glow)] bg-[var(--surface-elevated)] p-5">
                    {paymentError && (
                      <div
                        role="alert"
                        className="mb-4 border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-3.5 text-xs text-[var(--text-primary)]"
                      >
                        {paymentError}
                      </div>
                    )}
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <div>
                        <p className="text-xs font-semibold uppercase tracking-[0.16em] text-[var(--color-glow)]">
                          {lang === "zh-HK"
                            ? "Stripe 官方安全結帳"
                            : "Stripe Hosted Checkout"}
                        </p>
                        <p className="mt-1 text-xs text-[var(--text-secondary)]">
                          {lang === "zh-HK"
                            ? "前往 Stripe 官方安全結帳頁面完成信用卡或 Apple Pay / Google Pay 付款。"
                            : "Proceed to Stripe's hosted checkout page to settle all cart items via Card, Apple Pay, or Google Pay."}
                        </p>
                      </div>
                      <button
                        type="button"
                        disabled={paying}
                        onClick={handleHostedStripeCheckout}
                        className="bg-[var(--color-glow)] px-5 py-3 text-xs font-semibold tracking-[0.18em] uppercase text-[#11100e] hover:opacity-90 disabled:opacity-50"
                      >
                        {lang === "zh-HK"
                          ? "前往 Stripe 付款 →"
                          : "Open Stripe Checkout →"}
                      </button>
                    </div>
                  </div>
                ) : (
                <form onSubmit={handleCardPayment} className="space-y-5">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <h2 className="font-display text-xl">
                        {lang === "zh-HK"
                          ? "信用卡安全付款閘道 (Stripe)"
                          : "Secure Card Settlement (Stripe)"}
                      </h2>
                      {stripePublishableKey && (
                        <p className="font-mono text-[10px] text-[var(--text-secondary)]">
                          Key: {stripePublishableKey.slice(0, 12)}…
                        </p>
                      )}
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <button
                        type="button"
                        onClick={autofillSuccessCard}
                        className="border border-[var(--color-glow)] px-2.5 py-1 text-[11px] tracking-[0.1em] uppercase text-[var(--color-glow)] hover:bg-[var(--color-glow)] hover:text-[#11100e]"
                      >
                        {lang === "zh-HK"
                          ? "填入測試卡 (4242 成功)"
                          : "Autofill Test Card (4242)"}
                      </button>
                      <button
                        type="button"
                        onClick={autofillDeclineCard}
                        className="border border-[var(--border-hairline)] px-2.5 py-1 text-[11px] tracking-[0.1em] uppercase text-[var(--text-secondary)] hover:border-[var(--color-error)]"
                      >
                        {lang === "zh-HK"
                          ? "測試拒付卡 (0002)"
                          : "Test Decline (0002)"}
                      </button>
                    </div>
                  </div>

                  {paymentError && (
                    <div
                      role="alert"
                      className="border-l-4 border-[var(--color-error)] bg-[var(--surface-elevated)] p-3.5 text-xs text-[var(--text-primary)]"
                    >
                      {paymentError}
                    </div>
                  )}

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="cc-name" className="text-xs text-[var(--text-secondary)]">
                      {lang === "zh-HK" ? "持卡人姓名 *" : "Cardholder Name *"}
                    </label>
                    <input
                      id="cc-name"
                      type="text"
                      required
                      value={cardholderName}
                      onChange={(e) => setCardholderName(e.target.value)}
                      className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 text-sm uppercase"
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="cc-number" className="text-xs text-[var(--text-secondary)]">
                      {lang === "zh-HK" ? "信用卡號碼 *" : "Card Number *"}
                    </label>
                    <input
                      id="cc-number"
                      type="text"
                      inputMode="numeric"
                      required
                      placeholder="4242 4242 4242 4242"
                      value={cardNumber}
                      onChange={(e) => setCardNumber(e.target.value)}
                      className="tabular-nums border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 font-mono text-sm"
                    />
                  </div>

                  <div className="grid grid-cols-2 gap-4">
                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="cc-exp" className="text-xs text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "到期日 (MM/YY) *" : "Expiry (MM/YY) *"}
                      </label>
                      <input
                        id="cc-exp"
                        type="text"
                        required
                        placeholder="12/29"
                        value={expiry}
                        onChange={(e) => setExpiry(e.target.value)}
                        className="tabular-nums border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 font-mono text-sm"
                      />
                    </div>
                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="cc-cvc" className="text-xs text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "安全碼 (CVC) *" : "Security Code (CVC) *"}
                      </label>
                      <input
                        id="cc-cvc"
                        type="text"
                        inputMode="numeric"
                        required
                        placeholder="888"
                        value={cvc}
                        onChange={(e) => setCvc(e.target.value)}
                        className="tabular-nums border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 font-mono text-sm"
                      />
                    </div>
                  </div>

                  <button
                    type="submit"
                    disabled={paying}
                    className="w-full bg-[var(--color-glow)] py-4 text-xs font-semibold tracking-[0.22em] uppercase text-[#11100e] transition-opacity hover:opacity-90 disabled:opacity-50"
                  >
                    {paying
                      ? lang === "zh-HK"
                        ? "正在授權並確認付款..."
                        : "Authorizing & Settling Order..."
                      : lang === "zh-HK"
                        ? `立即授權付款 ${formatMoney(order.total_cents, order.currency)}`
                        : `Pay ${formatMoney(order.total_cents, order.currency)} Now`}
                  </button>
                </form>
                )}
              </div>
            )}

            {/* CASE 3: RESERVED + FPS -> HK FPS Instructions & Reference Submission (FPS-01 configurable) */}
            {isReserved && order.payment_method === "fps" && (
              <div className="no-print mt-8 space-y-6">
                <div className="border border-[var(--border-hairline)] bg-[var(--surface-elevated)] p-5">
                  <h2 className="font-display text-xl">
                    {lang === "zh-HK"
                      ? "香港轉數快 (FPS) 付款指示"
                      : "Hong Kong FPS Faster Payment Instructions"}
                  </h2>
                  <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
                    {lang === "zh-HK"
                      ? "請使用香港銀行應用程式轉帳以下確切金額，並於備註填寫訂單編號，然後在下方提交銀行轉帳參考編號。"
                      : "Transfer the exact amount below via your Hong Kong banking app with your Order Reference in the remarks, then submit your bank transfer reference number below."}
                  </p>

                  <dl className="mt-4 grid grid-cols-1 gap-3 border-t border-[var(--border-hairline)] pt-4 text-xs sm:grid-cols-2">
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "轉數快識別碼" : "Merchant FPS ID"}
                      </dt>
                      <dd className="tabular-nums mt-1 font-mono text-sm font-semibold">
                        {displayFpsId}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "收款帳戶名稱" : "FPS Account Name"}
                      </dt>
                      <dd className="mt-1 font-sans text-sm font-semibold">
                        {displayFpsAccount}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "應付金額" : "Exact Amount"}
                      </dt>
                      <dd className="tabular-nums mt-1 font-sans text-sm font-semibold text-[var(--color-glow)]">
                        {formatMoney(order.total_cents, order.currency)}
                      </dd>
                    </div>
                    <div>
                      <dt className="text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "轉帳備註編號" : "Required Remark"}
                      </dt>
                      <dd className="tabular-nums mt-1 font-mono text-sm font-semibold">
                        {order.order_code}
                      </dd>
                    </div>
                  </dl>

                  {fpsQrUrl && (
                    <div className="mt-4 border-t border-[var(--border-hairline)] pt-4">
                      <p className="text-[11px] tracking-[0.14em] uppercase text-[var(--text-secondary)]">
                        {lang === "zh-HK" ? "掃描轉數快二維碼" : "Scan FPS QR Code"}
                      </p>
                      {/* eslint-disable-next-line @next/next/no-img-element */}
                      <img
                        src={fpsQrUrl}
                        alt="FPS QR Code"
                        className="mt-2 h-40 w-40 border border-[var(--border-hairline)] bg-white object-contain p-2"
                      />
                    </div>
                  )}
                </div>

                {paymentError && (
                  <div
                    role="alert"
                    className="border-l-4 border-[var(--color-error)] p-3 text-xs"
                  >
                    {paymentError}
                  </div>
                )}

                {fpsSubmittedMsg && (
                  <div
                    role="status"
                    className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-elevated)] p-4 text-xs"
                  >
                    {fpsSubmittedMsg}
                  </div>
                )}

                <form onSubmit={handleFpsSubmit} className="space-y-4">
                  <div className="flex flex-col gap-1.5">
                    <label
                      htmlFor="fps-ref-input"
                      className="text-xs text-[var(--text-secondary)]"
                    >
                      {lang === "zh-HK"
                        ? "銀行 / 轉數快交易參考編號 *"
                        : "Bank / FPS Transfer Reference Number *"}
                    </label>
                    <input
                      id="fps-ref-input"
                      type="text"
                      required
                      placeholder="e.g. FPS-20261015-99281"
                      value={fpsRefInput}
                      onChange={(e) => setFpsRefInput(e.target.value)}
                      className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 font-mono text-sm"
                    />
                  </div>
                  <button
                    type="submit"
                    disabled={paying}
                    className="w-full bg-[var(--color-glow)] py-3.5 text-xs font-semibold tracking-[0.2em] uppercase text-[#11100e]"
                  >
                    {paying
                      ? lang === "zh-HK"
                        ? "正在提交..."
                        : "Recording Reference..."
                      : lang === "zh-HK"
                        ? "提交轉數快參考編號"
                        : "Submit FPS Transfer Reference"}
                  </button>
                </form>
              </div>
            )}

            {/* Self-Service Cancel Hold Button while Reserved (2-step confirm) */}
            {isReserved && (
              <div className="no-print mt-8 border-t border-[var(--border-hairline)] pt-5 text-xs">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <span className="text-[var(--text-secondary)]">
                    {confirmCancel
                      ? lang === "zh-HK"
                        ? "確定取消留貨？此操作無法復原，配額將立即釋放回香港存倉。"
                        : "Confirm cancellation? This cannot be undone — the units return to the Hong Kong vault immediately."
                      : lang === "zh-HK"
                        ? "需要更改付款方式或釋放留貨配額？"
                        : "Need to switch payment method or release this hold?"}
                  </span>
                  <div className="flex items-center gap-2">
                    {confirmCancel && (
                      <button
                        type="button"
                        disabled={cancelling}
                        onClick={() => setConfirmCancel(false)}
                        className="border border-[var(--border-hairline)] px-3.5 py-1.5 text-xs uppercase text-[var(--text-secondary)] transition-colors hover:border-[var(--text-primary)] hover:text-[var(--text-primary)]"
                      >
                        {lang === "zh-HK" ? "保留留貨" : "Keep My Hold"}
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={cancelling}
                      onClick={handleCancelHold}
                      className={`border px-3.5 py-1.5 text-xs uppercase transition-colors ${
                        confirmCancel
                          ? "border-[var(--color-error)] bg-[var(--color-error)] text-white"
                          : "border-[var(--border-hairline)] text-[var(--text-secondary)] hover:border-[var(--color-error)] hover:text-[var(--color-error)]"
                      }`}
                    >
                      {cancelling
                        ? lang === "zh-HK"
                          ? "取消中..."
                          : "Cancelling..."
                        : confirmCancel
                          ? lang === "zh-HK"
                            ? "確認取消並釋放配額"
                            : "Yes — Cancel & Release"
                          : lang === "zh-HK"
                            ? "取消留貨並釋放配額"
                            : "Cancel Hold & Release Stock"}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {/* CASE 4: EXPIRED OR CANCELLED */}
            {(order.status === "expired" || order.status === "cancelled") && (
              <div className="mt-8 space-y-4 border border-[var(--border-hairline)] bg-[var(--surface-elevated)] p-6">
                <p className="text-sm font-medium">
                  {order.status === "expired"
                    ? lang === "zh-HK"
                      ? "此留貨訂單已逾時，鎖定配額已自動釋放回香港存倉。"
                      : "This reservation hold has expired and its units have been returned to the saleable vault."
                    : lang === "zh-HK"
                      ? "此留貨訂單已取消，鎖定配額已釋放回香港存倉。"
                      : "This reservation hold has been cancelled and its units have been returned to the saleable vault."}
                </p>
                <Link
                  href={`/${lang}/products/${order.product_slug}`}
                  className="inline-block bg-[var(--text-primary)] px-6 py-3 text-xs tracking-[0.18em] uppercase text-[var(--surface-page)]"
                >
                  {lang === "zh-HK" ? "重新選購作品 →" : "Reserve Piece Again →"}
                </Link>
              </div>
            )}
          </div>
        </div>

        {/* Right Column: Order Summary Card (All Cart Line Items) */}
        <div className="lg:col-span-5">
          <div className="card-exhibition p-6 sm:p-8">
            <p className="text-[11px] tracking-[0.2em] uppercase text-[var(--text-secondary)]">
              {lang === "zh-HK" ? "典藏訂單摘要" : "EDITION ORDER SUMMARY"}
            </p>
            <p className="tabular-nums mt-1 font-mono text-sm font-medium">
              {order.order_code}
            </p>

            <div className="mt-5 divide-y divide-[var(--border-hairline)] border-y border-[var(--border-hairline)]">
              {enrichedItems.map((it) => (
                <div key={it.product_slug} className="flex gap-4 py-4">
                  {it.product && (
                    <div className="w-20 shrink-0">
                      <LampStage
                        hue={it.product.art_hue}
                        variant={it.product.art_variant}
                        lit={lit}
                        imageDayUrl={it.product.image_day_url}
                        imageLitUrl={it.product.image_lit_url}
                        aspectClass="aspect-square"
                        compact
                      />
                    </div>
                  )}
                  <div className="flex flex-1 flex-col justify-center">
                    {it.badge && (
                      <p className="text-[10px] tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                        {it.badge}
                      </p>
                    )}
                    <h3 className="mt-0.5 font-display text-base">{it.title}</h3>
                    <div className="mt-1 flex items-baseline justify-between text-xs text-[var(--text-secondary)]">
                      <span className="tabular-nums">
                        {formatMoney(it.unit_price_cents, order.currency)} ×{" "}
                        {it.quantity}
                      </span>
                      <span className="tabular-nums font-medium text-[var(--text-primary)]">
                        {formatMoney(it.line_total_cents, order.currency)}
                      </span>
                    </div>
                  </div>
                </div>
              ))}
            </div>

            <dl className="mt-5 space-y-3 text-xs">
              <div className="flex justify-between">
                <dt className="text-[var(--text-secondary)]">
                  {lang === "zh-HK" ? "香港專人送遞" : "Hong Kong Delivery"}
                </dt>
                <dd>{lang === "zh-HK" ? "免費送遞" : "Complimentary"}</dd>
              </div>
              {order.fps_reference && (
                <div className="flex justify-between">
                  <dt className="text-[var(--text-secondary)]">
                    {lang === "zh-HK" ? "已提交 FPS 編號" : "Submitted FPS Ref"}
                  </dt>
                  <dd className="font-mono font-medium">{order.fps_reference}</dd>
                </div>
              )}
              <div className="flex items-baseline justify-between border-t border-[var(--border-hairline)] pt-4 text-sm font-semibold">
                <dt>{lang === "zh-HK" ? "總計" : "Total"}</dt>
                <dd className="tabular-nums font-display text-2xl">
                  {formatMoney(order.total_cents, order.currency)}
                </dd>
              </div>
            </dl>
          </div>
        </div>
      </div>

      {/* Sent Email Receipt Preview Modal */}
      {showEmailModal && order.email_receipt && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Sent Email Notification Preview"
          className="no-print fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4 backdrop-blur-xs"
          onClick={(e) => {
            if (e.target === e.currentTarget) setShowEmailModal(false);
          }}
        >
          <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto border border-[var(--border-hairline)] bg-[var(--surface-card)] p-6">
            <div className="flex items-center justify-between border-b border-[var(--border-hairline)] pb-4">
              <div>
                <p className="text-[10px] tracking-[0.2em] uppercase text-[var(--color-glow)]">
                  {lang === "zh-HK"
                    ? "已發送之電郵通知預覽"
                    : "DISPATCHED TRANSACTIONAL EMAIL PREVIEW"}
                </p>
                <p className="mt-1 text-sm font-medium">
                  {order.email_receipt.subject}
                </p>
              </div>
              <button
                type="button"
                onClick={() => setShowEmailModal(false)}
                className="border border-[var(--border-hairline)] px-3 py-1 text-xs uppercase"
              >
                {lang === "zh-HK" ? "關閉" : "Close"}
              </button>
            </div>
            <div
              className="mt-4 overflow-hidden border border-[var(--border-hairline)]"
              dangerouslySetInnerHTML={{ __html: order.email_receipt.html }}
            />
          </div>
        </div>
      )}
    </div>
  );
}
