import type { Metadata } from "next";
import Link from "next/link";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";
import { formatMoney } from "@/lib/format";

export const metadata: Metadata = { title: "Orders & Quarantine" };
export const dynamic = "force-dynamic";

interface AdminOrderItemRow {
  product_id: number;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  line_total_cents: number;
}

interface AdminOrderRow {
  id: number;
  order_code: string;
  product_id: number;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  total_cents: number;
  currency: string;
  payment_method: string;
  status: string;
  customer_name: string;
  customer_email: string;
  customer_phone: string;
  delivery_address: string;
  fps_reference: string;
  expires_at: string;
  created_at: string;
  items?: AdminOrderItemRow[];
}

export default async function AdminOrdersPage({
  searchParams,
}: {
  searchParams: Promise<{ saved?: string; error?: string; status?: string }>;
}) {
  await requireAdmin();
  const { saved, error, status: statusFilter } = await searchParams;

  const { data } = await adminFetch("/api/admin/orders");
  const allOrders = Array.isArray(data) ? (data as AdminOrderRow[]) : [];

  const filtered =
    statusFilter && statusFilter !== "all"
      ? allOrders.filter((o) => o.status === statusFilter)
      : allOrders;

  const statuses = [
    "all",
    "reserved",
    "paid",
    "fulfilled",
    "late_payment_review",
    "returned_quarantined",
    "expired",
    "cancelled",
  ];

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
        <div>
          <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
            ORDER FULFILMENT & QUARANTINE CONSOLE
          </p>
          <h1 className="mt-1 font-serif text-3xl">
            Orders & Reservations ({filtered.length})
          </h1>
        </div>

        {/* Status Filter Bar */}
        <div className="flex flex-wrap gap-1.5 text-xs">
          {statuses.map((st) => {
            const active = (statusFilter ?? "all") === st;
            return (
              <Link
                key={st}
                href={st === "all" ? "/admin/orders" : `/admin/orders?status=${st}`}
                className={`border px-3 py-1.5 uppercase tracking-[0.1em] ${
                  active
                    ? "border-[var(--text-primary)] bg-[var(--text-primary)] text-[var(--surface-page)]"
                    : "border-[var(--border-hairline)] bg-[var(--surface-card)] text-[var(--text-secondary)]"
                }`}
              >
                {st.replace(/_/g, " ")}
              </Link>
            );
          })}
        </div>
      </div>

      {saved && (
        <div
          role="status"
          className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-card)] p-4 text-sm"
        >
          Saved: {saved}
        </div>
      )}

      {error && (
        <div
          role="alert"
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm text-[var(--color-error)]"
        >
          {error}
        </div>
      )}

      <div className="overflow-x-auto border border-[var(--border-hairline)] bg-[var(--surface-card)]">
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-[var(--border-hairline)] text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
              <th className="px-4 py-3">Order Code</th>
              <th className="px-4 py-3">SKU × Qty</th>
              <th className="px-4 py-3">Total</th>
              <th className="px-4 py-3">Method & Status</th>
              <th className="px-4 py-3">Collector & Delivery</th>
              <th className="px-4 py-3">Actions</th>
            </tr>
          </thead>
          <tbody>
            {filtered.length === 0 ? (
              <tr>
                <td
                  colSpan={6}
                  className="px-4 py-8 text-center text-xs text-[var(--text-secondary)]"
                >
                  No orders matching this filter.
                </td>
              </tr>
            ) : (
              filtered.map((row) => (
                <tr
                  key={row.id}
                  className="border-b border-[var(--border-hairline)] align-top hover:bg-[var(--surface-elevated)]"
                >
                  <td className="px-4 py-3">
                    <Link
                      href={`/en/checkout/${encodeURIComponent(row.order_code)}`}
                      target="_blank"
                      className="font-mono font-semibold underline"
                    >
                      {row.order_code} ↗
                    </Link>
                    <div className="mt-0.5 text-xs text-[var(--text-secondary)]">
                      #{row.id} · Exp: {row.expires_at.slice(0, 19).replace("T", " ")}
                    </div>
                  </td>
                  <td className="px-4 py-3 font-mono text-xs">
                    {row.items && row.items.length > 0 ? (
                      <div className="space-y-1">
                        {row.items.map((it) => (
                          <div key={it.product_slug}>
                            {it.product_slug} × {it.quantity}
                          </div>
                        ))}
                      </div>
                    ) : (
                      <span>
                        {row.product_slug} × {row.quantity}
                      </span>
                    )}
                  </td>
                  <td className="tabular-nums px-4 py-3 font-medium">
                    {formatMoney(row.total_cents, row.currency)}
                  </td>
                  <td className="px-4 py-3">
                    <span className="mr-2 uppercase text-xs text-[var(--text-secondary)]">
                      {row.payment_method}
                    </span>
                    <span
                      className={`inline-block border px-2 py-0.5 text-xs uppercase ${
                        row.status === "paid" || row.status === "fulfilled"
                          ? "border-[var(--color-success)] text-[var(--color-success)] font-semibold"
                          : row.status === "late_payment_review"
                            ? "border-[var(--color-error)] text-[var(--color-error)] font-semibold"
                            : "border-[var(--color-glow)] text-[var(--text-primary)]"
                      }`}
                    >
                      {row.status}
                    </span>
                    {row.fps_reference && (
                      <div className="mt-1 font-mono text-xs text-[var(--color-glow)]">
                        FPS Ref: {row.fps_reference}
                      </div>
                    )}
                  </td>
                  <td className="px-4 py-3 text-xs">
                    <div className="font-medium">{row.customer_name}</div>
                    <div className="text-[var(--text-secondary)]">
                      {row.customer_email}
                      {row.customer_phone ? ` · ${row.customer_phone}` : ""}
                    </div>
                    {row.delivery_address && (
                      <div className="mt-0.5 text-[var(--text-secondary)]">
                        {row.delivery_address}
                      </div>
                    )}
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex flex-col gap-2">
                      {row.payment_method === "fps" &&
                        (row.status === "reserved" || row.status === "expired") && (
                          <form
                            method="post"
                            action={`/admin/api/orders/${row.id}`}
                            className="flex flex-wrap items-center gap-1.5"
                          >
                            <input
                              type="text"
                              name="fps_reference"
                              required
                              defaultValue={row.fps_reference}
                              placeholder="FPS Ref #"
                              className="w-32 border border-[var(--border-hairline)] bg-[var(--surface-page)] px-2 py-1 font-mono text-xs"
                            />
                            <button
                              type="submit"
                              name="action"
                              value="confirm_fps"
                              className="bg-[var(--color-glow)] px-2.5 py-1 text-xs font-semibold uppercase text-[#11100e]"
                            >
                              {row.status === "expired"
                                ? "Record Late FPS"
                                : "Confirm FPS"}
                            </button>
                          </form>
                        )}

                      {row.status === "reserved" && (
                        <form method="post" action={`/admin/api/orders/${row.id}`}>
                          <button
                            type="submit"
                            name="action"
                            value="release"
                            className="border border-[var(--border-hairline)] px-2.5 py-1 text-xs uppercase hover:border-[var(--color-error)]"
                          >
                            Release Hold
                          </button>
                        </form>
                      )}

                      {row.status === "paid" && (
                        <form method="post" action={`/admin/api/orders/${row.id}`}>
                          <button
                            type="submit"
                            name="action"
                            value="fulfil"
                            className="bg-[var(--color-success)] px-3 py-1 text-xs uppercase text-white"
                          >
                            ✓ Mark Fulfilled / Dispatched
                          </button>
                        </form>
                      )}

                      {(row.status === "paid" || row.status === "fulfilled") && (
                        <form method="post" action={`/admin/api/orders/${row.id}`}>
                          <button
                            type="submit"
                            name="action"
                            value="quarantine_return"
                            className="border border-[var(--border-hairline)] px-2.5 py-1 text-xs uppercase text-[var(--color-error)] hover:border-[var(--color-error)]"
                          >
                            Quarantine Return
                          </button>
                        </form>
                      )}
                    </div>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
