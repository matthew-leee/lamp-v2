import type { Metadata } from "next";
import Link from "next/link";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";
import { loadStorefrontData } from "@/lib/api";
import AdminProductStudioClient, {
  type AdminProductFullData,
  type AdminProductSummary,
} from "@/components/admin-product-studio-client";

export const metadata: Metadata = { title: "Product Studio" };
export const dynamic = "force-dynamic";

export default async function AdminProductStudioPage({
  params,
  searchParams,
}: {
  params: Promise<{ id: string }>;
  searchParams: Promise<{ saved?: string; error?: string }>;
}) {
  await requireAdmin();
  const { id } = await params;
  const { saved, error } = await searchParams;

  const [fullRes, listRes, storefront] = await Promise.all([
    adminFetch(`/api/admin/products/${id}`),
    adminFetch("/api/admin/products"),
    loadStorefrontData().catch(() => null),
  ]);

  if (!fullRes.ok || !fullRes.data) {
    return (
      <div className="card-exhibition space-y-4 p-8">
        <h1 className="font-serif text-2xl">Product Not Found</h1>
        <p className="text-sm text-[var(--text-secondary)]">
          Product #{id} could not be loaded from the API.
        </p>
        <Link href="/admin/products" className="text-xs underline">
          ← Back to Products
        </Link>
      </div>
    );
  }

  const product = fullRes.data as AdminProductFullData;
  const allProducts = Array.isArray(listRes.data)
    ? (listRes.data as AdminProductSummary[])
    : [];
  const currency = storefront?.config.currency ?? "HKD";

  return (
    <AdminProductStudioClient
      initialProduct={product}
      allProducts={allProducts}
      currency={currency}
      initialBanner={saved ?? null}
      initialError={error ?? null}
    />
  );
}
