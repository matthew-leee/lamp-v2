import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";
import { loadStorefrontData } from "@/lib/api";
import {
  buildOrderEmailHtml,
  dispatchTransactionalEmail,
  type EmailOrderItemSummary,
} from "@/lib/email-receipt";
import { formatProductTitle } from "@/lib/format";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
): Promise<Response> {
  const session = await verifyAdminSession();
  if (!session) {
    return new Response(null, { status: 303, headers: { Location: "/admin/login" } });
  }
  const { id } = await params;
  const form = await request.formData();
  const action = String(form.get("action") ?? "");

  let endpoint = "";
  let body = "{}";

  if (action === "confirm_fps") {
    endpoint = `/api/admin/orders/${id}/confirm-fps`;
    body = JSON.stringify({
      fps_reference: String(form.get("fps_reference") ?? "").trim(),
    });
  } else if (action === "fulfil") {
    endpoint = `/api/admin/orders/${id}/fulfil`;
  } else if (action === "release") {
    endpoint = `/api/admin/orders/${id}/release`;
  } else if (action === "quarantine_return") {
    endpoint = `/api/admin/orders/${id}/quarantine-return`;
  } else {
    return new Response(null, {
      status: 303,
      headers: { Location: "/admin/orders?error=Unknown+order+action" },
    });
  }

  const { ok, data } = await adminFetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });

  revalidatePath("/", "layout");

  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Order action failed";
    return new Response(null, {
      status: 303,
      headers: { Location: `/admin/orders?error=${encodeURIComponent(err)}` },
    });
  }

  // EMAIL-01: When admin confirms an FPS payment (`status === "paid"`), dispatch the payment confirmation email
  if (action === "confirm_fps" && data && typeof data === "object") {
    const confirmedOrder = data as {
      order_code?: string;
      product_slug?: string;
      quantity?: number;
      unit_price_cents?: number;
      total_cents?: number;
      currency?: string;
      payment_method?: string;
      status?: string;
      customer_name?: string;
      customer_email?: string;
      fps_reference?: string;
      expires_at?: string;
      created_at?: string;
      items?: Array<{
        product_slug: string;
        quantity: number;
        unit_price_cents: number;
        line_total_cents: number;
      }>;
    };
    if (confirmedOrder.status === "paid" && confirmedOrder.customer_email) {
      try {
        const { catalogue, config } = await loadStorefrontData();
        const rawItems =
          confirmedOrder.items && confirmedOrder.items.length > 0
            ? confirmedOrder.items
            : [
                {
                  product_slug: String(confirmedOrder.product_slug ?? ""),
                  quantity: Number(confirmedOrder.quantity ?? 1),
                  unit_price_cents: Number(confirmedOrder.unit_price_cents ?? 0),
                  line_total_cents: Number(confirmedOrder.total_cents ?? 0),
                },
              ];
        const itemSummaries: EmailOrderItemSummary[] = rawItems.map((it) => {
          const p = catalogue.products.find((cp) => cp.slug === it.product_slug);
          const copy = p?.content.en ?? {};
          const title = p
            ? formatProductTitle(p.model, copy.model_alt, copy.colourway, "en")
            : it.product_slug;
          return {
            productTitle: title,
            productSlug: it.product_slug,
            quantity: it.quantity,
            unitPriceCents: it.unit_price_cents,
            lineTotalCents: it.line_total_cents,
          };
        });
        const prod = catalogue.products.find(
          (p) => p.slug === confirmedOrder.product_slug,
        );
        const title = prod
          ? formatProductTitle(
              prod.model,
              prod.content.en?.model_alt,
              prod.content.en?.colourway,
              "en",
            )
          : String(confirmedOrder.product_slug ?? "");

        const receipt = buildOrderEmailHtml({
          orderCode: String(confirmedOrder.order_code ?? ""),
          status: "paid",
          paymentMethod: String(confirmedOrder.payment_method ?? "fps"),
          productTitle: title,
          productSlug: String(confirmedOrder.product_slug ?? ""),
          quantity: Number(confirmedOrder.quantity ?? 1),
          unitPriceCents: Number(confirmedOrder.unit_price_cents ?? 0),
          totalCents: Number(confirmedOrder.total_cents ?? 0),
          currency: String(confirmedOrder.currency ?? config.currency),
          items: itemSummaries,
          customerName: confirmedOrder.customer_name,
          customerEmail: confirmedOrder.customer_email,
          fpsReference: confirmedOrder.fps_reference,
          expiresAt: String(confirmedOrder.expires_at ?? ""),
          createdAt: String(confirmedOrder.created_at ?? ""),
          brandName: config.brandName,
          siteUrl: config.siteUrl,
          locale: "en",
        });

        await dispatchTransactionalEmail({
          to: confirmedOrder.customer_email,
          subject: receipt.subject,
          html: receipt.html,
          brandName: config.brandName,
        });
      } catch {
        // Non-fatal
      }
    }
  }

  return new Response(null, {
    status: 303,
    headers: {
      Location: `/admin/orders?saved=${encodeURIComponent(`Order #${id} (${action})`)}`,
    },
  });
}
