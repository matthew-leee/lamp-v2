import { revalidatePath } from "next/cache";
import { getApiBaseUrl } from "@/lib/api";

export const dynamic = "force-dynamic";

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
    const data = await res.json().catch(() => ({}));
    if (res.ok) {
      revalidatePath("/", "layout");
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
