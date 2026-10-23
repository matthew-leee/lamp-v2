import { notFound } from "next/navigation";
import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import {
  buildOrderEmailHtml,
  type EmailOrderItemSummary,
} from "@/lib/email-receipt";
import { formatProductTitle } from "@/lib/format";
import { isLocale, type Locale } from "@/lib/i18n";
import CheckoutSettlementClient, {
  type PublicOrderData,
} from "@/components/checkout-settlement-client";

export const dynamic = "force-dynamic";

export default async function CheckoutSettlementPage({
  params,
  searchParams,
}: {
  params: Promise<{ lang: string; code: string }>;
  searchParams: Promise<{ session_id?: string }>;
}) {
  const { lang, code } = await params;
  const { session_id: sessionId } = await searchParams;
  if (!isLocale(lang)) notFound();
  const locale: Locale = lang;

  const [orderRes, storefront] = await Promise.all([
    fetch(`${getApiBaseUrl()}/api/orders/${encodeURIComponent(code)}`, {
      cache: "no-store",
    }),
    loadStorefrontData(),
  ]);

  if (!orderRes.ok) {
    notFound();
  }

  const order = (await orderRes.json()) as PublicOrderData;
  const { catalogue, config } = storefront;
  const product =
    catalogue.products.find((p) => p.slug === order.product_slug) ?? null;

  const rawItems =
    order.items && order.items.length > 0
      ? order.items
      : [
          {
            product_id: 0,
            product_slug: order.product_slug,
            quantity: order.quantity,
            unit_price_cents: order.unit_price_cents,
            line_total_cents: order.total_cents,
          },
        ];

  const itemSummaries: EmailOrderItemSummary[] = rawItems.map((it) => {
    const p = catalogue.products.find((cp) => cp.slug === it.product_slug);
    const copy = p?.content[locale] ?? {};
    const title = p
      ? formatProductTitle(p.model, copy.model_alt, copy.colourway, locale)
      : it.product_slug;
    return {
      productTitle: title,
      productSlug: it.product_slug,
      quantity: it.quantity,
      unitPriceCents: it.unit_price_cents,
      lineTotalCents: it.line_total_cents,
    };
  });

  const copy = product?.content[locale] ?? {};
  const productTitle = product
    ? formatProductTitle(product.model, copy.model_alt, copy.colourway, locale)
    : order.product_slug;

  const emailReceipt = buildOrderEmailHtml({
    orderCode: order.order_code,
    status: order.status,
    paymentMethod: order.payment_method,
    productTitle,
    productSlug: order.product_slug,
    quantity: order.quantity,
    unitPriceCents: order.unit_price_cents,
    totalCents: order.total_cents,
    currency: order.currency,
    items: itemSummaries,
    fpsReference: order.fps_reference,
    expiresAt: order.expires_at,
    createdAt: order.created_at,
    brandName: config.brandName,
    siteUrl: config.siteUrl,
    locale,
  });

  const stripePublishableKey =
    process.env.NEXT_PUBLIC_STRIPE_PUBLISHABLE_KEY?.trim() || null;
  const stripeRedirectConfigured = Boolean(
    process.env.STRIPE_SECRET_KEY?.trim(),
  );

  return (
    <CheckoutSettlementClient
      lang={locale}
      initialOrder={{ ...order, email_receipt: emailReceipt }}
      product={product}
      allProducts={catalogue.products}
      totalPieces={catalogue.total_pieces}
      brandName={config.brandName}
      whatsappNumber={config.whatsappNumber}
      fpsId={config.fpsId}
      fpsAccountName={config.fpsAccountName}
      fpsQrUrl={config.fpsQrUrl}
      stripePublishableKey={stripePublishableKey}
      stripeRedirectConfigured={stripeRedirectConfigured}
      returnSessionId={sessionId ?? null}
    />
  );
}
