import { revalidatePath } from "next/cache";
import {
  adminFetch,
  verifyAdminSession,
  verifySameOriginRequest,
} from "@/lib/admin-auth";
import { getApiBaseUrl, loadStorefrontData } from "@/lib/api";
import {
  claimPaidOrderEmail,
  dispatchTransactionalEmail,
  recordOrderEmailDelivery,
} from "@/lib/email-receipt";
import {
  buildOrderReceiptEmail,
  type ReceiptOrderLike,
} from "@/lib/order-email";
import { isLocale, type Locale } from "@/lib/i18n";

export const dynamic = "force-dynamic";

/** The admin order list rows carry the id + stored locale on top of receipt fields. */
type AdminOrderRow = ReceiptOrderLike & { id?: number | string; locale?: string | null };

function redirect(location: string): Response {
  return new Response(null, { status: 303, headers: { Location: location } });
}

/**
 * EMAIL-01 / findings G02: "Re-send Receipt" is a WEB-layer action — it
 * rebuilds the localized receipt from the durable admin order row and
 * dispatches it again, recording the outcome in the delivery ledger so the
 * admin Orders console always shows the truth (sent vs. last error).
 */
async function resendReceipt(id: string): Promise<Response> {
  const { ok, data } = await adminFetch("/api/admin/orders");
  const orders = Array.isArray(data) ? (data as AdminOrderRow[]) : [];
  const order = orders.find((o) => Number(o?.id) === Number(id));
  if (!ok || !order) {
    return redirect(
      `/admin/orders?error=${encodeURIComponent(`Order #${id} not found for re-send`)}`,
    );
  }
  // Only live orders carry a receipt worth re-sending (reserved = the hold
  // confirmation, paid/fulfilled = the paid receipt). Expired, cancelled and
  // quarantine states have no truthful receipt to rebuild.
  const resendable =
    order.status === "reserved" ||
    order.status === "paid" ||
    order.status === "fulfilled";
  if (!resendable) {
    return redirect(
      `/admin/orders?error=${encodeURIComponent(
        `Order #${id} is ${order.status} — a receipt can only be re-sent for reserved, paid or fulfilled orders`,
      )}`,
    );
  }
  try {
    const { catalogue, config } = await loadStorefrontData();
    const locale: Locale =
      typeof order.locale === "string" && isLocale(order.locale)
        ? order.locale
        : "en";
    const receipt = buildOrderReceiptEmail(order, catalogue, config, locale);
    const result = await dispatchTransactionalEmail({
      to: order.customer_email ?? undefined,
      subject: receipt.subject,
      html: receipt.html,
      brandName: config.brandName,
    });
    // The re-sent email is the HOLD confirmation on a `reserved` order and
    // the PAID receipt on a settled one — the ledger must know which, or a
    // re-sent reservation email would falsely stamp the paid-email ledger.
    await recordOrderEmailDelivery(
      getApiBaseUrl(),
      order.order_code,
      result,
      order.status === "reserved" ? "reservation" : "paid",
    );
    if (!result.dispatched) {
      return redirect(
        `/admin/orders?error=${encodeURIComponent(
          `Receipt re-send failed for ${order.order_code}: ${result.detail ?? "Email transport not configured or failed"}`,
        )}`,
      );
    }
    return redirect(
      `/admin/orders?saved=${encodeURIComponent(
        `Receipt re-sent to ${order.customer_email} (${order.order_code})`,
      )}`,
    );
  } catch (err) {
    return redirect(
      `/admin/orders?error=${encodeURIComponent(
        `Receipt re-send failed: ${err instanceof Error ? err.message : "unknown error"}`,
      )}`,
    );
  }
}

export async function POST(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json(
      { error: "Cross-origin request rejected" },
      { status: 403 },
    );
  }
  const session = await verifyAdminSession();
  if (!session) {
    return redirect("/admin/login");
  }
  const { id } = await params;
  const form = await request.formData();
  const action = String(form.get("action") ?? "");

  if (action === "resend_receipt") {
    return resendReceipt(id);
  }

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
    return redirect("/admin/orders?error=Unknown+order+action");
  }

  const { ok, data } = await adminFetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });

  revalidatePath("/", "layout");

  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Order action failed";
    return redirect(`/admin/orders?error=${encodeURIComponent(err)}`);
  }

  // EMAIL-01: When admin confirms an FPS payment (`status === "paid"`), dispatch the payment confirmation email
  if (action === "confirm_fps" && data && typeof data === "object") {
    const confirmedOrder = data as AdminOrderRow;
    if (confirmedOrder.status === "paid" && confirmedOrder.customer_email) {
      try {
        const apiBase = getApiBaseUrl();
        // Exactly-once dispatch (G02): only the atomic paid-email claim
        // winner may send — a concurrent settlement path (webhook return)
        // may already have told the customer.
        const claimed = await claimPaidOrderEmail(
          apiBase,
          confirmedOrder.order_code,
        );
        if (claimed) {
          try {
            const { catalogue, config } = await loadStorefrontData();
            // LANG-01 / findings G02: speak the language the buyer used when
            // ordering (stored on the order row) — never hardcode English.
            const locale: Locale =
              typeof confirmedOrder.locale === "string" &&
              isLocale(confirmedOrder.locale)
                ? confirmedOrder.locale
                : "en";
            const receipt = buildOrderReceiptEmail(
              confirmedOrder,
              catalogue,
              config,
              locale,
            );
            const result = await dispatchTransactionalEmail({
              to: confirmedOrder.customer_email,
              subject: receipt.subject,
              html: receipt.html,
              brandName: config.brandName,
            });
            await recordOrderEmailDelivery(
              apiBase,
              confirmedOrder.order_code,
              result,
            );
          } catch (err) {
            // Claim won but dispatch blew up: RELEASE the claim through the
            // delivery ledger (try/catch claim release) so "Re-send Receipt"
            // can repair it instead of the send staying "in flight".
            console.error("[admin/orders] FPS receipt dispatch error:", err);
            await recordOrderEmailDelivery(
              apiBase,
              confirmedOrder.order_code,
              {
                dispatched: false,
                transport: "preview-only",
                detail:
                  err instanceof Error
                    ? err.message
                    : "FPS receipt dispatch error",
              },
            );
          }
        }
      } catch (err) {
        console.error("[admin/orders] FPS confirmation email error:", err);
      }
    }
  }

  return redirect(
    `/admin/orders?saved=${encodeURIComponent(`Order #${id} (${action})`)}`,
  );
}
