import crypto from "node:crypto";
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

async function sendPaidConfirmationIfSettled(opts: {
  orderCode: string;
  recipientEmail?: string;
  locale: Locale;
  idempotent?: boolean;
}) {
  if (opts.idempotent) return;
  try {
    const [orderRes, storefront] = await Promise.all([
      fetch(`${getApiBaseUrl()}/api/orders/${encodeURIComponent(opts.orderCode)}`, {
        cache: "no-store",
      }),
      loadStorefrontData(),
    ]);
    if (!orderRes.ok) return;
    const order = (await orderRes.json()) as {
      order_code: string;
      product_slug: string;
      quantity: number;
      unit_price_cents: number;
      total_cents: number;
      currency: string;
      payment_method: string;
      status: string;
      fps_reference: string;
      expires_at: string;
      created_at: string;
      items?: Array<{
        product_slug: string;
        quantity: number;
        unit_price_cents: number;
        line_total_cents: number;
      }>;
    };
    if (order.status !== "paid" && order.status !== "fulfilled") return;

    const { catalogue, config } = storefront;
    const rawItems =
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
      const p = catalogue.products.find((cp) => cp.slug === it.product_slug);
      const copy = p?.content[opts.locale] ?? {};
      const title = p
        ? formatProductTitle(p.model, copy.model_alt, copy.colourway, opts.locale)
        : it.product_slug;
      return {
        productTitle: title,
        productSlug: it.product_slug,
        quantity: it.quantity,
        unitPriceCents: it.unit_price_cents,
        lineTotalCents: it.line_total_cents,
      };
    });

    const prod = catalogue.products.find((p) => p.slug === order.product_slug);
    const copy = prod?.content[opts.locale] ?? {};
    const title = prod
      ? formatProductTitle(prod.model, copy.model_alt, copy.colourway, opts.locale)
      : order.product_slug;

    const receipt = buildOrderEmailHtml({
      orderCode: order.order_code,
      status: order.status,
      paymentMethod: order.payment_method,
      productTitle: title,
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
      locale: opts.locale,
    });

    await dispatchTransactionalEmail({
      to: opts.recipientEmail,
      subject: receipt.subject,
      html: receipt.html,
      brandName: config.brandName,
    });
  } catch {
    // Non-fatal
  }
}

export async function POST(
  request: Request,
  { params }: { params: Promise<{ code: string }> },
): Promise<Response> {
  const { code } = await params;
  let body: {
    mode?: "stripe_checkout" | "verify_session" | "direct";
    sessionId?: string;
    locale?: string;
    customerEmail?: string;
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

  const locale: Locale =
    typeof body.locale === "string" && isLocale(body.locale) ? body.locale : "en";
  const isZh = locale === "zh-HK";
  const apiBase = getApiBaseUrl();

  // 1. Fetch authoritative order amount, currency, and line items from the Rust API
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
    product_slug: string;
    quantity: number;
    unit_price_cents: number;
    total_cents: number;
    currency: string;
    status: string;
    items?: Array<{
      product_slug: string;
      quantity: number;
      unit_price_cents: number;
      line_total_cents: number;
    }>;
  };

  const stripeSecretKey = process.env.STRIPE_SECRET_KEY?.trim() ?? "";

  // MODE A: Create a real Stripe Sandbox/Live Checkout Session when STRIPE_SECRET_KEY is configured
  if (body.mode === "stripe_checkout" && stripeSecretKey) {
    if (order.status !== "reserved") {
      return Response.json(
        {
          error: isZh
            ? "此訂單並非處於待付款留貨狀態。"
            : "Order is no longer in a reservable payment state.",
        },
        { status: 409 },
      );
    }

    try {
      const { catalogue, config } = await loadStorefrontData();
      const rawItems =
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

      const proto =
        request.headers.get("x-forwarded-proto") ??
        new URL(request.url).protocol.replace(":", "");
      const host =
        request.headers.get("x-forwarded-host") ??
        request.headers.get("host") ??
        "";
      const publicOrigin = host ? `${proto}://${host}` : config.siteUrl;

      const form = new URLSearchParams();
      form.set("mode", "payment");
      form.set("client_reference_id", order.order_code);
      form.set("metadata[order_code]", order.order_code);
      form.set(
        "success_url",
        `${publicOrigin}/${locale}/checkout/${encodeURIComponent(order.order_code)}?session_id={CHECKOUT_SESSION_ID}`,
      );
      form.set(
        "cancel_url",
        `${publicOrigin}/${locale}/checkout/${encodeURIComponent(order.order_code)}`,
      );

      rawItems.forEach((it, idx) => {
        const prod = catalogue.products.find((p) => p.slug === it.product_slug);
        const copy = prod?.content[locale] ?? {};
        const name = prod
          ? formatProductTitle(prod.model, copy.model_alt, copy.colourway, locale)
          : it.product_slug;
        form.set(
          `line_items[${idx}][price_data][currency]`,
          order.currency.toLowerCase(),
        );
        form.set(
          `line_items[${idx}][price_data][unit_amount]`,
          String(it.unit_price_cents),
        );
        form.set(`line_items[${idx}][price_data][product_data][name]`, name);
        form.set(`line_items[${idx}][quantity]`, String(it.quantity));
      });

      const stripeRes = await fetch("https://api.stripe.com/v1/checkout/sessions", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${stripeSecretKey}`,
          "Content-Type": "application/x-www-form-urlencoded",
          "Idempotency-Key": `checkout_session_v2_${order.order_code}`,
        },
        body: form.toString(),
      });
      const sessionData = (await stripeRes.json().catch(() => ({}))) as {
        id?: string;
        url?: string;
        error?: { message?: string };
      };
      if (!stripeRes.ok || !sessionData.url) {
        return Response.json(
          {
            error:
              sessionData.error?.message ??
              (isZh
                ? "未能建立 Stripe 結帳頁面。"
                : "Could not create Stripe Checkout Session"),
          },
          { status: 502 },
        );
      }
      return Response.json(
        { ok: true, checkout_url: sessionData.url, session_id: sessionData.id },
        { status: 200, headers: { "Cache-Control": "no-store" } },
      );
    } catch {
      return Response.json(
        {
          error: isZh
            ? "Stripe 結帳閘道暫時無法連線。"
            : "Stripe Checkout gateway unreachable",
        },
        { status: 502 },
      );
    }
  }

  // MODE B: Verify return session_id against Stripe API (STRIPE-01 return verification)
  if (body.mode === "verify_session" && body.sessionId && stripeSecretKey) {
    try {
      const stripeRes = await fetch(
        `https://api.stripe.com/v1/checkout/sessions/${encodeURIComponent(body.sessionId)}`,
        {
          headers: { Authorization: `Bearer ${stripeSecretKey}` },
          cache: "no-store",
        },
      );
      if (!stripeRes.ok) {
        return Response.json(
          {
            error: isZh
              ? "未能核實 Stripe 結帳工作階段。"
              : "Could not verify Stripe Checkout Session",
          },
          { status: 400 },
        );
      }
      const sessionObj = (await stripeRes.json()) as {
        id: string;
        payment_status?: string;
        amount_total?: number;
        currency?: string;
        customer_details?: { email?: string };
        metadata?: { order_code?: string };
        client_reference_id?: string;
      };
      const sessionOrderCode =
        sessionObj.metadata?.order_code ?? sessionObj.client_reference_id ?? "";
      if (
        sessionObj.payment_status !== "paid" ||
        sessionOrderCode !== order.order_code
      ) {
        return Response.json(
          {
            error: isZh
              ? "此 Stripe 結帳工作階段尚未完成付款。"
              : "Stripe session is not paid for this order",
          },
          { status: 400 },
        );
      }

      const webhookSecret =
        process.env.STRIPE_WEBHOOK_SECRET?.trim() ||
        "whsec_test_secret_0123456789abcdef";
      const timestamp = Math.floor(Date.now() / 1000);
      const eventPayload = JSON.stringify({
        id: `evt_session_${sessionObj.id}`,
        type: "checkout.session.completed",
        data: {
          object: {
            payment_status: "paid",
            metadata: { order_code: order.order_code },
            amount_total: sessionObj.amount_total ?? order.total_cents,
            currency: sessionObj.currency ?? order.currency,
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
      const webhookData = (await webhookRes.json().catch(() => ({}))) as {
        idempotent?: boolean;
        status?: string;
      };
      revalidatePath("/", "layout");

      if (webhookRes.ok && webhookData.status === "paid") {
        await sendPaidConfirmationIfSettled({
          orderCode: order.order_code,
          recipientEmail:
            sessionObj.customer_details?.email ?? body.customerEmail,
          locale,
          idempotent: webhookData.idempotent,
        });
      }

      return Response.json(
        { ok: webhookRes.ok, webhook: webhookData },
        { status: webhookRes.status, headers: { "Cache-Control": "no-store" } },
      );
    } catch {
      return Response.json(
        {
          error: isZh
            ? "核實 Stripe 付款狀態時發生錯誤。"
            : "Failed to verify Stripe session",
        },
        { status: 502 },
      );
    }
  }

  // SECURITY GUARD (STRIPE-01 / PAY-01): When real STRIPE_SECRET_KEY is configured,
  // simulated direct card settlement is disabled so payments must go through Stripe.
  if (stripeSecretKey) {
    return Response.json(
      {
        error: isZh
          ? "已啟用正式 Stripe 結帳，請使用上方「前往 Stripe 付款」按鈕完成交易。"
          : "Real Stripe Checkout is enabled. Please use the Stripe Checkout button to complete payment.",
      },
      { status: 400 },
    );
  }

  // MODE C: Local test fallback card settlement when STRIPE_SECRET_KEY is unset
  const digits = (body.cardNumber ?? "").replace(/\D/g, "");
  const cardholder = (body.cardholderName ?? "").trim();
  const expiry = (body.expiry ?? "").trim();
  const cvc = (body.cvc ?? "").replace(/\D/g, "");

  if (!cardholder) {
    return Response.json(
      {
        error: isZh ? "請填寫持卡人姓名。" : "Cardholder name is required.",
      },
      { status: 400 },
    );
  }
  if (digits.length < 13 || digits.length > 19) {
    return Response.json(
      {
        error: isZh
          ? "請輸入有效的 16 位信用卡號碼。"
          : "Please enter a valid 16-digit card number.",
      },
      { status: 400 },
    );
  }
  // Simulated Stripe decline test card (4000 0000 0000 0002) for error-recovery verification
  if (digits.endsWith("0002")) {
    return Response.json(
      {
        error: isZh
          ? "發卡銀行拒絕此交易，請核對信用卡資料或改用其他信用卡。"
          : "Your card was declined by the issuing bank. Please check your details or try another card.",
      },
      { status: 402 },
    );
  }
  if (!/^\d{2}\s*\/\s*\d{2}$/.test(expiry)) {
    return Response.json(
      {
        error: isZh
          ? "到期日格式須為 MM/YY。"
          : "Expiry date must be in MM/YY format.",
      },
      { status: 400 },
    );
  }
  if (cvc.length < 3 || cvc.length > 4) {
    return Response.json(
      {
        error: isZh
          ? "安全碼 (CVC) 須為 3 或 4 位數字。"
          : "Security code (CVC) must be 3 or 4 digits.",
      },
      { status: 400 },
    );
  }

  // Construct & sign a Stripe Event envelope (PAY-01)
  const webhookSecret =
    process.env.STRIPE_WEBHOOK_SECRET?.trim() || "whsec_test_secret_0123456789abcdef";
  const timestamp = Math.floor(Date.now() / 1000);
  const eventPayload = JSON.stringify({
    id: `evt_card_${order.order_code}`,
    type: "checkout.session.completed",
    data: {
      object: {
        payment_status: "paid",
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

  const webhookData = (await webhookRes.json().catch(() => ({}))) as {
    idempotent?: boolean;
    status?: string;
  };
  if (!webhookRes.ok) {
    return Response.json(webhookData, { status: webhookRes.status });
  }

  revalidatePath("/", "layout");

  if (webhookData.status === "paid") {
    await sendPaidConfirmationIfSettled({
      orderCode: order.order_code,
      recipientEmail: body.customerEmail,
      locale,
      idempotent: webhookData.idempotent,
    });
  }

  // Re-read updated order status
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
