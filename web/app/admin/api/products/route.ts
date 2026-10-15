import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";
import { parseDollarsToCents } from "@/lib/format";

export const dynamic = "force-dynamic";

export async function POST(request: Request): Promise<Response> {
  const session = await verifyAdminSession();
  if (!session) {
    return new Response(null, { status: 303, headers: { Location: "/admin/login" } });
  }

  const form = await request.formData();
  const slug = String(form.get("slug") ?? "").trim();
  const brand = String(form.get("brand") ?? "").trim();
  const model = String(form.get("model") ?? "").trim();
  const family = String(form.get("family") ?? "").trim();
  const artVariant = String(form.get("art_variant") ?? "dome").trim();
  const artHue = Number.parseInt(String(form.get("art_hue") ?? "38"), 10);
  const active = form.get("active") === "true";

  // Accept either human dollars (price_dollars) or raw cents (price_cents)
  const rawDollars = String(form.get("price_dollars") ?? "").trim();
  const rawCents = String(form.get("price_cents") ?? "").trim();
  const priceCents =
    rawDollars !== ""
      ? (parseDollarsToCents(rawDollars) ?? -1)
      : Number.parseInt(rawCents, 10);

  const { ok, data } = await adminFetch("/api/admin/products", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      slug,
      brand,
      model,
      price_cents: Number.isFinite(priceCents) ? priceCents : -1,
      family,
      art_variant: artVariant,
      art_hue: Number.isFinite(artHue) ? artHue : 38,
      active,
    }),
  });

  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Invalid product fields";
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/products?error=${encodeURIComponent(err)}`,
      },
    });
  }

  revalidatePath("/", "layout");
  const created = data as { id?: number; slug?: string };
  if (typeof created?.id === "number") {
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/products/${created.id}?saved=${encodeURIComponent(`Created ${created.slug ?? slug}`)}`,
      },
    });
  }
  return new Response(null, {
    status: 303,
    headers: { Location: `/admin/products?saved=${encodeURIComponent(slug)}` },
  });
}
