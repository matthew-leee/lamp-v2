import { notFound } from "next/navigation";
import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import { buildOrderEmailHtml } from "@/lib/email-receipt";
import { formatProductTitle } from "@/lib/format";
import { isLocale, type Locale } from "@/lib/i18n";
import CheckoutSettlementClient, {
  type PublicOrderData,
} from "@/components/checkout-settlement-client";

export const dynamic = "force-dynamic";

export default async function CheckoutSettlementPage({
  params,
}: {
  params: Promise<{ lang: string; code: string }>;
}) {
  const { lang, code } = await params;
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

  const copy = product?.content[locale] ?? product?.content.en ?? {};
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
    fpsReference: order.fps_reference,
    expiresAt: order.expires_at,
    createdAt: order.created_at,
    brandName: config.brandName,
    siteUrl: config.siteUrl,
    locale,
  });

  return (
    <CheckoutSettlementClient
      lang={locale}
      initialOrder={{ ...order, email_receipt: emailReceipt }}
      product={product}
      totalPieces={catalogue.total_pieces}
      brandName={config.brandName}
      whatsappNumber={config.whatsappNumber}
    />
  );
}
