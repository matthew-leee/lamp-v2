import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import { buildOrderEmailHtml } from "@/lib/email-receipt";
import { formatProductTitle } from "@/lib/format";
import { isLocale, type Locale } from "@/lib/i18n";

export const dynamic = "force-dynamic";

export async function GET(
  request: Request,
  { params }: { params: Promise<{ code: string }> },
): Promise<Response> {
  const { code } = await params;
  const url = new URL(request.url);
  const rawLocale = url.searchParams.get("locale") ?? "en";
  const locale: Locale = isLocale(rawLocale) ? rawLocale : "en";

  try {
    const res = await fetch(
      `${getApiBaseUrl()}/api/orders/${encodeURIComponent(code)}`,
      { cache: "no-store" },
    );
    const data = (await res.json().catch(() => ({}))) as Record<string, unknown>;
    if (!res.ok) {
      return Response.json(data, {
        status: res.status,
        headers: { "Cache-Control": "no-store" },
      });
    }

    let emailReceipt: { subject: string; html: string } | null = null;
    try {
      const { catalogue, config } = await loadStorefrontData();
      const prod = catalogue.products.find((p) => p.slug === data.product_slug);
      const copy = prod?.content[locale] ?? {};
      const title = prod
        ? formatProductTitle(prod.model, copy.model_alt, copy.colourway, locale)
        : String(data.product_slug ?? "");
      emailReceipt = buildOrderEmailHtml({
        orderCode: String(data.order_code ?? code),
        status: String(data.status ?? "reserved"),
        paymentMethod: String(data.payment_method ?? "card"),
        productTitle: title,
        productSlug: String(data.product_slug ?? ""),
        quantity: Number(data.quantity ?? 1),
        unitPriceCents: Number(data.unit_price_cents ?? 0),
        totalCents: Number(data.total_cents ?? 0),
        currency: String(data.currency ?? config.currency),
        fpsReference: String(data.fps_reference ?? ""),
        expiresAt: String(data.expires_at ?? ""),
        createdAt: String(data.created_at ?? ""),
        brandName: config.brandName,
        siteUrl: config.siteUrl,
        locale,
      });
    } catch {
      // ignore
    }

    return Response.json(
      { ...data, email_receipt: emailReceipt },
      { status: 200, headers: { "Cache-Control": "no-store" } },
    );
  } catch {
    return Response.json(
      { error: "Order lookup service unavailable" },
      { status: 503, headers: { "Cache-Control": "no-store" } },
    );
  }
}
