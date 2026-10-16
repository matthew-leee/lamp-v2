import { revalidatePath } from "next/cache";
import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import {
  buildOrderEmailHtml,
  dispatchTransactionalEmail,
  type EmailOrderItemSummary,
} from "@/lib/email-receipt";
import { formatProductTitle } from "@/lib/format";
import { isLocale, type Locale } from "@/lib/i18n";

export const dynamic = "force-dynamic";

export async function POST(request: Request): Promise<Response> {
  let body: Record<string, unknown>;
  try {
    body = (await request.json()) as Record<string, unknown>;
  } catch {
    return Response.json(
      { error: "Invalid JSON request body" },
      { status: 400, headers: { "Cache-Control": "no-store" } },
    );
  }

  const locale: Locale =
    typeof body.locale === "string" && isLocale(body.locale) ? body.locale : "en";

  try {
    const res = await fetch(`${getApiBaseUrl()}/api/orders/reserve`, {
      method: "POST",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        slug: body.slug ?? "",
        quantity: body.quantity ?? 1,
        items: Array.isArray(body.items) ? body.items : [],
        payment_method: body.payment_method,
        customer_name: body.customer_name,
        customer_email: body.customer_email,
        customer_phone: body.customer_phone ?? "",
        delivery_address: body.delivery_address ?? "",
      }),
    });

    const data = (await res.json().catch(() => ({}))) as Record<string, unknown>;
    if (!res.ok) {
      return Response.json(data, {
        status: res.status,
        headers: { "Cache-Control": "no-store" },
      });
    }

    revalidatePath("/", "layout");

    // Build transactional email receipt preview & dispatch live email (EMAIL-01)
    let emailReceipt: { subject: string; html: string } | null = null;
    try {
      const { catalogue, config } = await loadStorefrontData();
      const rawItems = Array.isArray(data.items)
        ? (data.items as Array<{
            product_slug: string;
            quantity: number;
            unit_price_cents: number;
            line_total_cents: number;
          }>)
        : [];

      const itemSummaries: EmailOrderItemSummary[] = rawItems.map((it) => {
        const p = catalogue.products.find((cp) => cp.slug === it.product_slug);
        const copy = p?.content[locale] ?? {};
        const title = p
          ? formatProductTitle(p.model, copy.model_alt, copy.colourway, locale)
          : it.product_slug;
        return {
          productTitle: title,
          productSlug: it.product_slug,
          quantity: Number(it.quantity ?? 1),
          unitPriceCents: Number(it.unit_price_cents ?? 0),
          lineTotalCents: Number(it.line_total_cents ?? 0),
        };
      });

      const prod = catalogue.products.find((p) => p.slug === data.product_slug);
      const copy = prod?.content[locale] ?? {};
      const title = prod
        ? formatProductTitle(prod.model, copy.model_alt, copy.colourway, locale)
        : String(data.product_slug ?? "");

      emailReceipt = buildOrderEmailHtml({
        orderCode: String(data.order_code ?? ""),
        status: String(data.status ?? "reserved"),
        paymentMethod: String(data.payment_method ?? "card"),
        productTitle: title,
        productSlug: String(data.product_slug ?? ""),
        quantity: Number(data.quantity ?? 1),
        unitPriceCents: Number(data.unit_price_cents ?? 0),
        totalCents: Number(data.total_cents ?? 0),
        currency: String(data.currency ?? config.currency),
        items: itemSummaries,
        customerName: String(data.customer_name ?? ""),
        customerEmail: String(data.customer_email ?? ""),
        expiresAt: String(data.expires_at ?? ""),
        createdAt: String(data.created_at ?? ""),
        brandName: config.brandName,
        siteUrl: config.siteUrl,
        locale,
      });

      await dispatchTransactionalEmail({
        to: String(data.customer_email ?? ""),
        subject: emailReceipt.subject,
        html: emailReceipt.html,
        brandName: config.brandName,
      });
    } catch (err) {
      console.error("[reserve/route] Email dispatch error:", err);
    }

    return Response.json(
      { ...data, email_receipt: emailReceipt },
      { status: 200, headers: { "Cache-Control": "no-store" } },
    );
  } catch {
    return Response.json(
      { error: "Reservation service temporarily unavailable" },
      { status: 503, headers: { "Cache-Control": "no-store" } },
    );
  }
}
