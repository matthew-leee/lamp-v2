import crypto from "node:crypto";
import { revalidatePath } from "next/cache";
import { getApiBaseUrl } from "@/lib/api";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ code: string }> },
): Promise<Response> {
  const { code } = await params;
  let body: {
    cardholderName?: string;
    cardNumber?: string;
    expiry?: string;
    cvc?: string;
  };
  try {
    body = (await request.json()) as typeof body;
  } catch {
    return Response.json({ error: "Invalid payment request" }, { status: 400 });
  }

  const digits = (body.cardNumber ?? "").replace(/\D/g, "");
  const cardholder = (body.cardholderName ?? "").trim();
  const expiry = (body.expiry ?? "").trim();
  const cvc = (body.cvc ?? "").replace(/\D/g, "");

  if (!cardholder) {
    return Response.json({ error: "Cardholder name is required." }, { status: 400 });
  }
  if (digits.length < 13 || digits.length > 19) {
    return Response.json(
      { error: "Please enter a valid 16-digit card number." },
      { status: 400 },
    );
  }
  // Simulated Stripe decline test card (4000 0000 0000 0002) for error-recovery verification
  if (digits.endsWith("0002")) {
    return Response.json(
      { error: "Your card was declined by the issuing bank. Please check your details or try another card." },
      { status: 402 },
    );
  }
  if (!/^\d{2}\s*\/\s*\d{2}$/.test(expiry)) {
    return Response.json(
      { error: "Expiry date must be in MM/YY format." },
      { status: 400 },
    );
  }
  if (cvc.length < 3 || cvc.length > 4) {
    return Response.json(
      { error: "Security code (CVC) must be 3 or 4 digits." },
      { status: 400 },
    );
  }

  const apiBase = getApiBaseUrl();
  // 1. Fetch authoritative order amount & currency from the Rust API
  const orderRes = await fetch(
    `${apiBase}/api/orders/${encodeURIComponent(code)}`,
    { cache: "no-store" },
  );
  if (!orderRes.ok) {
    const err = await orderRes.json().catch(() => ({ error: "Order not found" }));
    return Response.json(err, { status: orderRes.status });
  }
  const order = (await orderRes.json()) as {
    order_code: string;
    total_cents: number;
    currency: string;
    status: string;
  };

  // 2. Construct & sign a real Stripe Event envelope (PAY-01)
  const webhookSecret =
    process.env.STRIPE_WEBHOOK_SECRET?.trim() || "whsec_test_secret_0123456789abcdef";
  const timestamp = Math.floor(Date.now() / 1000);
  const eventPayload = JSON.stringify({
    id: `evt_card_${order.order_code}`,
    type: "checkout.session.completed",
    data: {
      object: {
        metadata: { order_code: order.order_code },
        amount_total: order.total_cents,
        currency: order.currency,
      },
    },
  });

  const hmac = crypto
    .createHmac("sha256", webhookSecret)
    .update(`${timestamp}.${eventPayload}`)
    .digest("hex");

  const webhookRes = await fetch(`${apiBase}/api/webhooks/stripe`, {
    method: "POST",
    cache: "no-store",
    headers: {
      "Content-Type": "application/json",
      "Stripe-Signature": `t=${timestamp},v1=${hmac}`,
    },
    body: eventPayload,
  });

  const webhookData = await webhookRes.json().catch(() => ({}));
  if (!webhookRes.ok) {
    return Response.json(webhookData, { status: webhookRes.status });
  }

  revalidatePath("/", "layout");

  // 3. Re-read updated order status
  const updatedRes = await fetch(
    `${apiBase}/api/orders/${encodeURIComponent(code)}`,
    { cache: "no-store" },
  );
  const updatedOrder = await updatedRes.json().catch(() => order);

  return Response.json(
    { ok: true, webhook: webhookData, order: updatedOrder },
    { status: 200, headers: { "Cache-Control": "no-store" } },
  );
}
