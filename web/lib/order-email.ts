/**
 * Shared localized order-receipt builder (EMAIL-01, LANG-01, findings G02).
 *
 * Every receipt surface — reservation confirmation, Stripe webhook-only
 * settlement, direct card settlement, admin FPS confirmation and admin
 * "re-send receipt" — must render the SAME receipt in the language the buyer
 * used. Keeping the builder here means a copy/format change lands in one
 * place instead of four.
 */
import type { CatalogueResponse } from "./api.ts";
import { formatProductTitle } from "./format.ts";
import type { Locale } from "./i18n.ts";
import {
  buildOrderEmailHtml,
  type EmailOrderItemSummary,
} from "./email-receipt.ts";

export interface ReceiptOrderItem {
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  line_total_cents: number;
}

/** The order fields any receipt needs — public, admin and webhook shapes all satisfy it. */
export interface ReceiptOrderLike {
  order_code: string;
  product_slug: string;
  quantity: number;
  unit_price_cents: number;
  total_cents: number;
  currency: string;
  payment_method: string;
  status: string;
  fps_reference?: string | null;
  expires_at: string;
  created_at: string;
  customer_name?: string | null;
  customer_email?: string | null;
  items?: ReceiptOrderItem[] | null;
}

export interface ReceiptBrandConfig {
  brandName: string;
  siteUrl: string;
}

/**
 * Builds the bilingual/localized receipt for an order using the catalogue's
 * per-locale product titles. `statusOverride` lets the caller force the paid
 * wording (e.g. admin FPS confirmation lands on the exact paid moment).
 */
export function buildOrderReceiptEmail(
  order: ReceiptOrderLike,
  catalogue: CatalogueResponse,
  config: ReceiptBrandConfig,
  locale: Locale,
  opts: { statusOverride?: string } = {},
): { subject: string; html: string } {
  const rawItems: ReceiptOrderItem[] =
    order.items && order.items.length > 0
      ? order.items
      : [
          {
            product_slug: order.product_slug,
            quantity: order.quantity,
            unit_price_cents: order.unit_price_cents,
            line_total_cents: order.total_cents,
          },
        ];

  const itemSummaries: EmailOrderItemSummary[] = rawItems.map((it) => {
    const product = catalogue.products.find((cp) => cp.slug === it.product_slug);
    const copy = product?.content[locale] ?? {};
    return {
      productTitle: product
        ? formatProductTitle(product.model, copy.model_alt, copy.colourway, locale)
        : it.product_slug,
      productSlug: it.product_slug,
      quantity: it.quantity,
      unitPriceCents: it.unit_price_cents,
      lineTotalCents: it.line_total_cents,
    };
  });

  const primary = catalogue.products.find((p) => p.slug === order.product_slug);
  const primaryCopy = primary?.content[locale] ?? {};
  const primaryTitle = primary
    ? formatProductTitle(
        primary.model,
        primaryCopy.model_alt,
        primaryCopy.colourway,
        locale,
      )
    : order.product_slug;

  return buildOrderEmailHtml({
    orderCode: order.order_code,
    status: opts.statusOverride ?? order.status,
    paymentMethod: order.payment_method,
    productTitle: primaryTitle,
    productSlug: order.product_slug,
    quantity: order.quantity,
    unitPriceCents: order.unit_price_cents,
    totalCents: order.total_cents,
    currency: order.currency,
    items: itemSummaries,
    customerName: order.customer_name ?? undefined,
    customerEmail: order.customer_email ?? undefined,
    fpsReference: order.fps_reference ?? undefined,
    expiresAt: order.expires_at,
    createdAt: order.created_at,
    brandName: config.brandName,
    siteUrl: config.siteUrl,
    locale,
  });
}
