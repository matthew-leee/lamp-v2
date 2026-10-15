import { formatMoney } from "./format";
import type { Locale } from "./i18n";

export interface EmailNotificationPayload {
  orderCode: string;
  status: string;
  paymentMethod: string;
  productTitle: string;
  productSlug: string;
  quantity: number;
  unitPriceCents: number;
  totalCents: number;
  currency: string;
  customerName?: string;
  customerEmail?: string;
  fpsReference?: string;
  expiresAt: string;
  createdAt: string;
  brandName: string;
  siteUrl: string;
  locale: Locale;
}

/**
 * Generates a rich bilingual/localized HTML email receipt for an order
 * and optionally dispatches via Resend / webhook if RESEND_API_KEY is set.
 */
export function buildOrderEmailHtml(payload: EmailNotificationPayload): {
  subject: string;
  html: string;
} {
  const isPaid = payload.status === "paid" || payload.status === "fulfilled";
  const isZh = payload.locale === "zh-HK";
  const formattedTotal = formatMoney(payload.totalCents, payload.currency);
  const checkoutUrl = `${payload.siteUrl}/${payload.locale}/checkout/${encodeURIComponent(payload.orderCode)}`;

  const subject = isZh
    ? isPaid
      ? `[${payload.brandName}] 付款確認及典藏證書 · 訂單 ${payload.orderCode}`
      : `[${payload.brandName}] 藏家留貨確認 · 訂單 ${payload.orderCode}`
    : isPaid
      ? `[${payload.brandName}] Payment Confirmed & Collector Pass · ${payload.orderCode}`
      : `[${payload.brandName}] Reservation Hold Confirmed · ${payload.orderCode}`;

  const statusHeading = isZh
    ? isPaid
      ? "付款已確認 · 作品已為閣下正式鎖定"
      : "作品已為閣下暫留 · 請於時限內完成付款"
    : isPaid
      ? "Payment Confirmed · Your Piece Is Officially Secured"
      : "Piece Reserved · Complete Settlement Before Hold Expiry";

  const html = `<!DOCTYPE html>
<html lang="${isZh ? "zh-Hant-HK" : "en"}">
<head><meta charset="utf-8" /><title>${subject}</title></head>
<body style="margin:0;padding:32px;background:#f6f1eb;color:#1e1b18;font-family:Georgia,serif;">
  <div style="max-width:600px;margin:0 auto;background:#fbf9f5;border:1px solid #ddd3c2;padding:40px;">
    <div style="font-family:sans-serif;font-size:11px;letter-spacing:0.24em;text-transform:uppercase;color:#5a534a;">
      ${payload.brandName} · HONG KONG VAULT
    </div>
    <h1 style="font-size:24px;font-weight:normal;margin:16px 0 8px;">${statusHeading}</h1>
    <p style="font-family:sans-serif;font-size:14px;color:#5a534a;margin:0 0 24px;">
      ${isZh ? "訂單編號" : "Order Reference"}: <strong style="color:#1e1b18;">${payload.orderCode}</strong>
    </p>
    <table style="width:100%;border-collapse:collapse;font-family:sans-serif;font-size:14px;border-top:1px solid #ddd3c2;border-bottom:1px solid #ddd3c2;margin:20px 0;">
      <tr>
        <td style="padding:12px 0;color:#5a534a;">${isZh ? "典藏作品" : "Edition Piece"}</td>
        <td style="padding:12px 0;text-align:right;font-weight:600;">${payload.productTitle} × ${payload.quantity}</td>
      </tr>
      <tr>
        <td style="padding:12px 0;color:#5a534a;">${isZh ? "付款方式" : "Settlement Method"}</td>
        <td style="padding:12px 0;text-align:right;text-transform:uppercase;">${payload.paymentMethod}</td>
      </tr>
      <tr>
        <td style="padding:12px 0;color:#5a534a;">${isZh ? "訂單狀態" : "Current Status"}</td>
        <td style="padding:12px 0;text-align:right;font-weight:600;color:${isPaid ? "#2a6b3c" : "#b97a1f"};">${payload.status.toUpperCase()}</td>
      </tr>
      ${payload.fpsReference ? `<tr><td style="padding:12px 0;color:#5a534a;">FPS Reference</td><td style="padding:12px 0;text-align:right;">${payload.fpsReference}</td></tr>` : ""}
      <tr>
        <td style="padding:14px 0;border-top:1px solid #ddd3c2;font-weight:600;">${isZh ? "總計金額" : "Total Amount"}</td>
        <td style="padding:14px 0;border-top:1px solid #ddd3c2;text-align:right;font-size:18px;font-weight:600;">${formattedTotal}</td>
      </tr>
    </table>
    <div style="margin:28px 0;">
      <a href="${checkoutUrl}" style="display:inline-block;background:#1e1b18;color:#f6f1eb;text-decoration:none;font-family:sans-serif;font-size:12px;letter-spacing:0.18em;text-transform:uppercase;padding:14px 28px;">
        ${isZh ? "查看訂單及付款狀態" : "Open Order & Settlement Pass"}
      </a>
    </div>
    <p style="font-family:sans-serif;font-size:12px;color:#5a534a;line-height:1.6;margin-top:28px;border-top:1px solid #ddd3c2;padding-top:16px;">
      ${isZh ? "完成付款後，專員將於香港安排免費速遞或親自交收。" : "Complimentary Hong Kong courier or personal hand-delivery is arranged directly upon settlement."}
    </p>
  </div>
</body>
</html>`;

  return { subject, html };
}
