"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import "./globals.css";

export default function GlobalNotFoundPage() {
  const pathname = usePathname() ?? "";
  const isZh = pathname === "/zh-HK" || pathname.startsWith("/zh-HK/");

  return (
    <html lang={isZh ? "zh-Hant-HK" : "en"}>
      <body className="flex min-h-dvh flex-col items-center justify-center px-6 text-center">
        <p className="text-xs tracking-[0.24em] uppercase text-[var(--text-secondary)]">
          {isZh ? "404 · 典藏庫" : "404 · EXHIBITION ARCHIVE"}
        </p>
        <h1 className="mt-3 font-serif text-4xl">
          {isZh ? "找不到此頁面。" : "Page not found."}
        </h1>
        <p className="mt-4 max-w-md text-sm text-[var(--text-secondary)]">
          {isZh
            ? "閣下所查詢之典藏頁面不存在或已下架。"
            : "The requested exhibition route does not exist or has been retired."}
        </p>
        <div className="mt-8 flex gap-4 text-xs tracking-[0.18em] uppercase">
          <Link
            href={isZh ? "/zh-HK" : "/en"}
            className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-6 py-3 text-[var(--surface-page)]"
          >
            {isZh ? "返回典藏首頁" : "Return to Exhibition"}
          </Link>
        </div>
      </body>
    </html>
  );
}
