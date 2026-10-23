import { revalidatePath } from "next/cache";
import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import {
  claimPaidOrderEmail,
  dispatchTransactionalEmail,
  recordOrderEmailDelivery,
} from "@/lib/email-receipt";
import { buildOrderReceiptEmail, type ReceiptOrderLike } from "@/lib/order-email";
import { isLocale, type Locale } from "@/lib/i18n";

export const dynamic = "force-dynamic";

/** Normalized `POST /api/webhooks/stripe` reply from the Rust payment authority. */
interface WebhookSettleResponse {
  ok?: boolean;
  idempotent?: boolean;
  status?: string;
  order_code?: string;
  customer_name?: string;
  customer_email?: string;
  locale?: string;
  paid_email_sent?: boolean;
}

/**
 * G02 — "Paid… and silence" fix: when an order settles via webhook ONLY
 * (the customer paid on the gateway's hosted page and never returned to the
 * site), the Rust API now hands back the order metadata the web tier needs to
 * tell the customer: `customer_email`, stored `locale` and `paid_email_sent`
 * (the durable delivery ledger). Send the localized receipt exactly once and
 * record the outcome so a failure is visible to "Re-send Receipt". The send
 * itself goes through the ATOMIC paid-email claim first: whoever wins the
 * claim dispatches, everyone else stays silent (exactly-once, G02).
 */
async function sendPaidReceiptIfPending(
  data: WebhookSettleResponse,
): Promise<void> {
  if (
    data.status !== "paid" ||
    data.paid_email_sent ||
    !data.customer_email ||
    !data.order_code
  ) {
    return;
  }
  try {
    const apiBase = getApiBaseUrl();
    const [orderRes, storefront] = await Promise.all([
      fetch(`${apiBase}/api/orders/${encodeURIComponent(data.order_code)}`, {
        cache: "no-store",
      }),
      loadStorefrontData(),
    ]);
    if (!orderRes.ok) return;
    const order = (await orderRes.json()) as ReceiptOrderLike;
    // Exactly-once dispatch: only the atomic claim winner may send. A lost
    // claim (already sent elsewhere, or not payable) stays silent here.
    const claimed = await claimPaidOrderEmail(apiBase, order.order_code);
    if (!claimed) return;
    // Claim won: ANY failure from here on must RELEASE the claim through the
    // delivery ledger (try/catch claim release), or the receipt would stay
    // "in flight" until the claim lease expires instead of being retried.
    try {
      const locale: Locale =
        typeof data.locale === "string" && isLocale(data.locale)
          ? data.locale
          : "en";
      const { catalogue, config } = storefront;
      const receipt = buildOrderReceiptEmail(order, catalogue, config, locale);
      const result = await dispatchTransactionalEmail({
        to: data.customer_email,
        subject: receipt.subject,
        html: receipt.html,
        brandName: config.brandName,
      });
      // Record the outcome: `dispatched: false` RELEASES the claim and stores
      // the failure reason so "Re-send Receipt" can repair it.
      await recordOrderEmailDelivery(apiBase, order.order_code, result);
    } catch (err) {
      console.error("[webhooks/stripe] Paid receipt dispatch error:", err);
      await recordOrderEmailDelivery(apiBase, order.order_code, {
        dispatched: false,
        transport: "preview-only",
        detail:
          err instanceof Error ? err.message : "Paid receipt dispatch error",
      });
    }
  } catch (err) {
    console.error("[webhooks/stripe] Paid receipt email error:", err);
  }
}

/**
 * Proxies signed Stripe webhook events straight to the Rust API payment
 * authority (`POST /api/webhooks/stripe`, PAY-01 + STRIPE-01) while preserving
 * the raw request body bytes so HMAC-SHA256 verification succeeds.
 */
export async function POST(request: Request): Promise<Response> {
  const rawBody = await request.text();
  const signature = request.headers.get("stripe-signature") ?? "";

  try {
    const res = await fetch(`${getApiBaseUrl()}/api/webhooks/stripe`, {
      method: "POST",
      cache: "no-store",
      headers: {
        "Content-Type": "application/json",
        "Stripe-Signature": signature,
      },
      body: rawBody,
    });
    const data = (await res.json().catch(() => ({}))) as WebhookSettleResponse;
    if (res.ok) {
      revalidatePath("/", "layout");
      await sendPaidReceiptIfPending(data);
    }
    return Response.json(data, {
      status: res.status,
      headers: { "Cache-Control": "no-store" },
    });
  } catch {
    return Response.json(
      { error: "Stripe webhook service unavailable" },
      { status: 503, headers: { "Cache-Control": "no-store" } },
    );
  }
}
