import Link from "next/link";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";
import { fetchLiveStockMap, loadStorefrontData } from "@/lib/api";

export const dynamic = "force-dynamic";

export default async function AdminDashboardPage() {
  const { username } = await requireAdmin();

  const [prodsRes, ordersRes, stockMap, storefront] = await Promise.all([
    adminFetch("/api/admin/products"),
    adminFetch("/api/admin/orders"),
    fetchLiveStockMap(),
    loadStorefrontData().catch(() => null),
  ]);

  const products = Array.isArray(prodsRes.data)
    ? (prodsRes.data as Array<{ id: number; slug: string; active: boolean }>)
    : [];
  const orders = Array.isArray(ordersRes.data)
    ? (ordersRes.data as Array<{ status: string }>)
    : [];

  const activeSkus = products.filter((p) => p.active).length;
  const stockEntries = Object.values(stockMap ?? {});
  const totalAvail = stockEntries.reduce((s, r) => s + r.available, 0);
  const totalReserved = stockEntries.reduce((s, r) => s + r.reserved, 0);
  const totalQuarantined = stockEntries.reduce((s, r) => s + r.quarantined, 0);

  const reservedOrders = orders.filter((o) => o.status === "reserved").length;
  const paidOrders = orders.filter(
    (o) => o.status === "paid" || o.status === "fulfilled",
  ).length;

  return (
    <div className="space-y-8">
      <div className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
        <div>
          <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
            OPERATOR STUDIO OVERVIEW
          </p>
          <h1 className="mt-1 font-serif text-3xl">
            {storefront?.config.brandName ?? "Flagship"} Control Center
          </h1>
        </div>
        <p className="text-xs text-[var(--text-secondary)]">
          Signed in as <strong>{username}</strong> · All edits propagate to the
          storefront in 0 seconds
        </p>
      </div>

      {/* Live Telemetry Cards */}
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <div className="card-exhibition p-5">
          <p className="text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Catalogue SKUs
          </p>
          <p className="tabular-nums mt-2 font-serif text-3xl">
            {products.length}
          </p>
          <p className="mt-1 text-xs text-[var(--text-secondary)]">
            {activeSkus} active on storefront · {products.length - activeSkus}{" "}
            hidden
          </p>
        </div>

        <div className="card-exhibition p-5">
          <p className="text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Saleable Vault Stock
          </p>
          <p className="tabular-nums mt-2 font-serif text-3xl text-[var(--color-glow)]">
            {totalAvail}
          </p>
          <p className="mt-1 text-xs text-[var(--text-secondary)]">
            {totalReserved} reserved · {totalQuarantined} quarantined
          </p>
        </div>

        <div className="card-exhibition p-5">
          <p className="text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Settled / Paid Orders
          </p>
          <p className="tabular-nums mt-2 font-serif text-3xl">{paidOrders}</p>
          <p className="mt-1 text-xs text-[var(--text-secondary)]">
            {reservedOrders} active holds awaiting payment
          </p>
        </div>

        <div className="card-exhibition p-5">
          <p className="text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Active Brand Identity
          </p>
          <p className="mt-2 font-serif text-2xl">
            {storefront?.config.brandName ?? "—"}
          </p>
          <p className="mt-1 text-xs text-[var(--text-secondary)]">
            Currency: {storefront?.config.currency ?? "HKD"} · Drop Pieces:{" "}
            {storefront?.catalogue.total_pieces ?? 0}
          </p>
        </div>
      </div>

      {/* Quick Action Studio Cards */}
      <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
        <Link href="/admin/products" className="card-exhibition block p-6">
          <p className="text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
            01 · CATALOGUE & EDITION STUDIO
          </p>
          <h2 className="mt-2 font-serif text-xl">Products & Pieces</h2>
          <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
            Create SKUs, upload Daylight & 2700K Lit photos, clone colourways,
            edit bilingual copy, and assign pieces 01–16.
          </p>
        </Link>

        <Link href="/admin/orders" className="card-exhibition block p-6">
          <p className="text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
            02 · MONEY PATH & QUARANTINE
          </p>
          <h2 className="mt-2 font-serif text-xl">Orders & Reservations</h2>
          <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
            Confirm HK FPS transfers, mark paid orders fulfilled, release stale
            holds, and quarantine returned units.
          </p>
        </Link>

        <Link href="/admin/settings" className="card-exhibition block p-6">
          <p className="text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
            03 · WHITE-LABEL & CHANNELS
          </p>
          <h2 className="mt-2 font-serif text-xl">Company Settings</h2>
          <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
            Live-swap Brand Name, Native Chinese Wordmark, Currency, WhatsApp
            Concierge, and Analytics with 0s storefront propagation.
          </p>
        </Link>

        <Link href="/admin/site-content" className="card-exhibition block p-6">
          <p className="text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
            04 · EDITORIAL VOICE
          </p>
          <h2 className="mt-2 font-serif text-xl">Bilingual Site Copy</h2>
          <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
            Edit Hero headlines, Drop framing, Provenance disclosure, and
            Warranty copy side-by-side in English and 繁體中文.
          </p>
        </Link>

        <Link href="/admin/audit" className="card-exhibition block p-6">
          <p className="text-xs tracking-[0.18em] uppercase text-[var(--color-glow)]">
            05 · ACCOUNTABILITY
          </p>
          <h2 className="mt-2 font-serif text-xl">Audit Trail</h2>
          <p className="mt-2 text-xs leading-relaxed text-[var(--text-secondary)]">
            Inspect the last 200 transactional admin mutations with actor,
            timestamp, and JSON change payload.
          </p>
        </Link>
      </div>
    </div>
  );
}
