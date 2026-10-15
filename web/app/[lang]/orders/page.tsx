"use client";

import { useState, type FormEvent } from "react";
import { useParams, useRouter } from "next/navigation";
import Link from "next/link";
import { isLocale, t, ui, type Locale } from "@/lib/i18n";

export default function OrderLookupPage() {
  const params = useParams<{ lang?: string }>();
  const lang: Locale =
    typeof params?.lang === "string" && isLocale(params.lang) ? params.lang : "en";
  const router = useRouter();

  const [orderCode, setOrderCode] = useState("");
  const [checking, setChecking] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  async function handleLookup(e: FormEvent) {
    e.preventDefault();
    const cleaned = orderCode.trim();
    if (!cleaned) return;
    setChecking(true);
    setErrorMsg(null);
    try {
      const res = await fetch(
        `/api/orders/${encodeURIComponent(cleaned)}?locale=${lang}`,
        { cache: "no-store" },
      );
      if (!res.ok) {
        setErrorMsg(
          lang === "zh-HK"
            ? `找不到訂單編號「${cleaned}」，請核對確認電郵內的編號。`
            : `Order "${cleaned}" was not found. Please check your confirmation email.`,
        );
        return;
      }
      router.push(`/${lang}/checkout/${encodeURIComponent(cleaned)}`);
    } catch {
      setErrorMsg(
        lang === "zh-HK"
          ? "查詢服務暫時未能連線，請稍後再試。"
          : "Order lookup service temporarily unavailable.",
      );
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="mx-auto max-w-2xl px-5 py-20 sm:px-10">
      <Link
        href={`/${lang}`}
        className="text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
      >
        ← {t(ui.backToCollection, lang)}
      </Link>

      <div className="card-exhibition mt-8 p-8 sm:p-10">
        <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
          {lang === "zh-HK" ? "藏家自助服務" : "COLLECTOR SELF-SERVICE"}
        </p>
        <h1 className="mt-2 font-display text-3xl">
          {t(ui.navOrderLookup, lang)}
        </h1>
        <p className="mt-3 text-sm leading-relaxed text-[var(--text-secondary)]">
          {lang === "zh-HK"
            ? "輸入你的訂單編號（例如 ORD-...）以查看即時留貨倒數、完成信用卡付款、提交轉數快參考編號或列印官方典藏證書。"
            : "Enter your Order Reference (e.g. ORD-...) to view your live hold timer, settle card payment, submit an FPS transfer reference, or print your official Collector's Pass."}
        </p>

        {errorMsg && (
          <div
            role="alert"
            className="mt-5 border-l-4 border-[var(--color-error)] bg-[var(--surface-elevated)] p-3.5 text-xs"
          >
            {errorMsg}
          </div>
        )}

        <form onSubmit={handleLookup} className="mt-6 flex flex-col gap-4 sm:flex-row">
          <input
            type="text"
            required
            placeholder="ORD-1760500000000-1"
            value={orderCode}
            onChange={(e) => setOrderCode(e.target.value)}
            aria-label={lang === "zh-HK" ? "訂單編號" : "Order Reference Code"}
            className="flex-1 border border-[var(--border-hairline)] bg-[var(--surface-page)] px-4 py-3 font-mono text-sm"
          />
          <button
            type="submit"
            disabled={checking}
            className="bg-[var(--text-primary)] px-7 py-3 text-xs font-medium tracking-[0.2em] uppercase text-[var(--surface-page)] disabled:opacity-50"
          >
            {checking
              ? lang === "zh-HK"
                ? "查詢中..."
                : "Searching..."
              : lang === "zh-HK"
                ? "查閱訂單 →"
                : "Open Order →"}
          </button>
        </form>
      </div>
    </div>
  );
}
