import type { ReactNode } from "react";
import Link from "next/link";
import { requireAdmin } from "@/lib/admin-auth";
import { loadStorefrontData } from "@/lib/api";
import DevAgentation from "@/components/dev-agentation";

export const dynamic = "force-dynamic";

export default async function AdminAuthedLayout({
  children,
}: {
  children: ReactNode;
}) {
  const { username } = await requireAdmin();
  let brandName = "STUDIO";
  try {
    const { config } = await loadStorefrontData();
    brandName = config.brandName;
  } catch {
    // ignore
  }

  return (
    <div className="flex min-h-dvh flex-col">
      <header className="border-b border-[var(--border-hairline)] bg-[var(--surface-card)] px-4 py-4 sm:px-6">
        <div className="mx-auto flex max-w-7xl flex-wrap items-center justify-between gap-4">
          <div className="flex flex-wrap items-center gap-8">
            <Link
              href="/admin"
              className="font-serif text-xl tracking-[0.18em] uppercase"
            >
              {brandName} <span className="text-xs text-[var(--color-glow)]">STUDIO</span>
            </Link>
            <nav
              aria-label="Admin Studio Navigation"
              className="flex flex-wrap items-center gap-5 text-xs tracking-[0.14em] uppercase text-[var(--text-secondary)]"
            >
              <Link href="/admin" className="hover:text-[var(--text-primary)]">
                Overview
              </Link>
              <Link
                href="/admin/products"
                className="hover:text-[var(--text-primary)]"
              >
                Products & Pieces
              </Link>
              <Link
                href="/admin/orders"
                className="hover:text-[var(--text-primary)]"
              >
                Orders & Quarantine
              </Link>
              <Link
                href="/admin/site-content"
                className="hover:text-[var(--text-primary)]"
              >
                Site Copy
              </Link>
              <Link
                href="/admin/settings"
                className="hover:text-[var(--text-primary)]"
              >
                Settings
              </Link>
              <Link
                href="/admin/audit"
                className="hover:text-[var(--text-primary)]"
              >
                Audit Log
              </Link>
              <Link
                href="/en"
                className="text-[var(--color-glow)] underline"
              >
                Live Storefront ↗
              </Link>
            </nav>
          </div>

          <div className="flex items-center gap-4 text-xs">
            <span className="text-[var(--text-secondary)]">
              signed in as <strong>{username}</strong>
            </span>
            <form method="post" action="/admin/api/logout">
              <button
                type="submit"
                className="border border-[var(--border-hairline)] px-3 py-1.5 uppercase tracking-[0.12em] hover:border-[var(--text-primary)]"
              >
                Sign out
              </button>
            </form>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-7xl flex-1 px-4 py-4 sm:px-6 sm:py-8">
        {children}
      </main>
      <DevAgentation />
    </div>
  );
}
