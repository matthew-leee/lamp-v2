import type { Metadata } from "next";
import Link from "next/link";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";
import { fetchLiveStockMap, loadStorefrontData, type ArtVariant } from "@/lib/api";
import { formatMoney } from "@/lib/format";
import LampStage from "@/components/lamp-stage";

export const metadata: Metadata = { title: "Products & Pieces" };
export const dynamic = "force-dynamic";

interface AdminProductRow {
  id: number;
  slug: string;
  brand: string;
  model: string;
  price_cents: number;
  family: string;
  art_variant: ArtVariant;
  art_hue: number;
  active: boolean;
  stock_total: number;
  stock_reserved: number;
}

export default async function AdminProductsPage({
  searchParams,
}: {
  searchParams: Promise<{ saved?: string; error?: string }>;
}) {
  await requireAdmin();
  const { saved, error } = await searchParams;

  const [prodsRes, stockMap, storefront] = await Promise.all([
    adminFetch("/api/admin/products"),
    fetchLiveStockMap(),
    loadStorefrontData().catch(() => null),
  ]);

  const rows = Array.isArray(prodsRes.data)
    ? (prodsRes.data as AdminProductRow[])
    : [];
  const currency = storefront?.config.currency ?? "HKD";
  const catalogueMap = new Map(
    (storefront?.catalogue.products ?? []).map((p) => [p.slug, p]),
  );

  const inputClass =
    "w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-sm";

  return (
    <div className="space-y-8">
      <div className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
        <div>
          <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
            CATALOGUE & EDITION MANAGEMENT
          </p>
          <h1 className="mt-1 font-serif text-3xl">Products ({rows.length})</h1>
        </div>
        <a
          href="#create-product-form"
          className="bg-[var(--text-primary)] px-5 py-2.5 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)]"
        >
          + Create New Product
        </a>
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
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm text-[var(--color-error)] font-medium"
        >
          {error}
        </div>
      )}

      {/* Products Table */}
      <div className="overflow-x-auto border border-[var(--border-hairline)] bg-[var(--surface-card)]">
        <table className="w-full border-collapse text-left text-sm">
          <thead>
            <tr className="border-b border-[var(--border-hairline)] text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]">
              <th className="px-4 py-3">Stage</th>
              <th className="px-4 py-3">SKU & Identity</th>
              <th className="px-4 py-3">House Price</th>
              <th className="px-4 py-3">Status</th>
              <th className="px-4 py-3">
                Stock (Total / Rsv / Quar / Saleable)
              </th>
              <th className="px-4 py-3 text-right">Actions</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const catProd = catalogueMap.get(row.slug);
              const stock = stockMap?.[row.slug];
              const q = stock?.quarantined ?? 0;
              const avail =
                stock?.available ??
                Math.max(0, row.stock_total - row.stock_reserved - q);
              return (
                <tr
                  key={row.id}
                  className="border-b border-[var(--border-hairline)] hover:bg-[var(--surface-elevated)]"
                >
                  <td className="px-4 py-3">
                    <div className="w-14">
                      <LampStage
                        hue={row.art_hue}
                        variant={row.art_variant}
                        lit={true}
                        imageDayUrl={catProd?.image_day_url}
                        imageLitUrl={catProd?.image_lit_url}
                        aspectClass="aspect-square"
                      />
                    </div>
                  </td>
                  <td className="px-4 py-3">
                    <div className="font-medium">
                      {row.brand} · {row.model}
                    </div>
                    <div className="font-mono text-xs text-[var(--text-secondary)]">
                      #{row.id} · {row.slug}
                      {row.family ? ` · ${row.family}` : ""}
                    </div>
                  </td>
                  <td className="tabular-nums px-4 py-3 font-medium">
                    {formatMoney(row.price_cents, currency)}
                  </td>
                  <td className="px-4 py-3">
                    <span
                      className={`border px-2.5 py-0.5 text-xs uppercase ${
                        row.active
                          ? "border-[var(--color-glow)] text-[var(--text-primary)]"
                          : "border-[var(--border-hairline)] text-[var(--text-secondary)]"
                      }`}
                    >
                      {row.active ? "Active" : "Hidden"}
                    </span>
                  </td>
                  <td className="tabular-nums px-4 py-3 font-mono text-xs">
                    {row.stock_total} / {row.stock_reserved} / {q} →{" "}
                    <strong className="text-[var(--color-glow)]">
                      {avail} avail
                    </strong>
                  </td>
                  <td className="px-4 py-3 text-right">
                    <div className="flex justify-end gap-3 text-xs">
                      <Link
                        href={`/admin/products/${row.id}`}
                        className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-3.5 py-1.5 uppercase tracking-[0.12em] text-[var(--surface-page)]"
                      >
                        Open Studio
                      </Link>
                      {row.active && (
                        <Link
                          href={`/en/products/${row.slug}`}
                          target="_blank"
                          className="border border-[var(--border-hairline)] px-3 py-1.5 uppercase tracking-[0.12em]"
                        >
                          Live ↗
                        </Link>
                      )}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* Create Product Form (Human Dollars + Auto-Redirects to Studio) */}
      <form
        id="create-product-form"
        method="post"
        action="/admin/api/products"
        className="card-exhibition max-w-3xl space-y-5 p-6 sm:p-8"
      >
        <div>
          <h2 className="font-serif text-2xl">Create New Product</h2>
          <p className="mt-1 text-xs text-[var(--text-secondary)]">
            Creates the SKU and immediately opens its Unified Studio so you can
            upload photos, edit bilingual copy, or assign edition numbers. Newly
            created active SKUs render cleanly on the storefront right away as
            an Open Edition.
          </p>
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <div>
            <label htmlFor="create-slug" className="text-xs text-[var(--text-secondary)]">
              Slug (lowercase letters, digits, hyphens) *
            </label>
            <input
              id="create-slug"
              name="slug"
              type="text"
              required
              placeholder="e.g. ph-5-mini-monochrome"
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor="create-brand" className="text-xs text-[var(--text-secondary)]">
              Maker / Brand *
            </label>
            <input
              id="create-brand"
              name="brand"
              type="text"
              required
              placeholder="e.g. Louis Poulsen"
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor="create-model" className="text-xs text-[var(--text-secondary)]">
              Model Name *
            </label>
            <input
              id="create-model"
              name="model"
              type="text"
              required
              placeholder="e.g. PH 5 Mini"
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor="create-price" className="text-xs text-[var(--text-secondary)]">
              Price in {currency} (Human Dollars, e.g. 4800) *
            </label>
            <input
              id="create-price"
              name="price_dollars"
              type="text"
              required
              placeholder="e.g. 4800"
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor="create-family" className="text-xs text-[var(--text-secondary)]">
              Family Group (optional)
            </label>
            <input
              id="create-family"
              name="family"
              type="text"
              placeholder="e.g. ph-5-mini"
              className={inputClass}
            />
          </div>
          <div>
            <label htmlFor="create-variant" className="text-xs text-[var(--text-secondary)]">
              Silhouette Variant
            </label>
            <select
              id="create-variant"
              name="art_variant"
              defaultValue="dome"
              className={inputClass}
            >
              <option value="dome">dome</option>
              <option value="shades">shades</option>
              <option value="shades-tall">shades-tall</option>
              <option value="piano">piano</option>
              <option value="none">none (plain photo / no CAD schematic)</option>
            </select>
          </div>
          <div>
            <label htmlFor="create-hue" className="text-xs text-[var(--text-secondary)]">
              Glow Hue (0–359)
            </label>
            <input
              id="create-hue"
              name="art_hue"
              type="number"
              min={0}
              max={359}
              defaultValue={38}
              required
              className={inputClass}
            />
          </div>
          <div className="flex items-center pt-5">
            <label className="flex cursor-pointer items-center gap-2.5 text-sm">
              <input
                type="checkbox"
                name="active"
                value="true"
                defaultChecked
                className="size-4 accent-[var(--color-glow)]"
              />
              <input type="hidden" name="active" value="false" />
              <span>List as Active on Storefront</span>
            </label>
          </div>
        </div>

        <button
          type="submit"
          className="bg-[var(--text-primary)] px-7 py-3 text-xs font-medium tracking-[0.2em] uppercase text-[var(--surface-page)]"
        >
          Create Product & Open Studio →
        </button>
      </form>
    </div>
  );
}
