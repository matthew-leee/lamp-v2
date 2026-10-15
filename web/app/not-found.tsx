import Link from "next/link";
import "./globals.css";

export default function GlobalNotFoundPage() {
  return (
    <html lang="en">
      <body className="flex min-h-dvh flex-col items-center justify-center px-6 text-center">
        <p className="text-xs tracking-[0.24em] uppercase text-[var(--text-secondary)]">
          404 · EXHIBITION ARCHIVE
        </p>
        <h1 className="mt-3 font-serif text-4xl">Page not found.</h1>
        <p className="mt-4 max-w-md text-sm text-[var(--text-secondary)]">
          The requested exhibition route does not exist or has been retired.
        </p>
        <div className="mt-8 flex gap-4 text-xs tracking-[0.18em] uppercase">
          <Link
            href="/en"
            className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-6 py-3 text-[var(--surface-page)]"
          >
            English Edition
          </Link>
          <Link
            href="/zh-HK"
            lang="zh-Hant-HK"
            className="border border-[var(--border-hairline)] px-6 py-3 text-[var(--text-primary)]"
          >
            繁體中文版
          </Link>
        </div>
      </body>
    </html>
  );
}
