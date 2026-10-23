import net from "node:net";
import tls from "node:tls";
import { formatMoney } from "./format.ts";
import type { Locale } from "./i18n.ts";

export interface EmailOrderItemSummary {
  productTitle: string;
  productSlug: string;
  quantity: number;
  unitPriceCents: number;
  lineTotalCents: number;
}

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
  items?: EmailOrderItemSummary[];
  customerName?: string;
  customerEmail?: string;
  fpsReference?: string;
  expiresAt: string;
  createdAt: string;
  brandName: string;
  siteUrl: string;
  locale: Locale;
}

export interface EmailDeliveryResult {
  dispatched: boolean;
  transport: "resend" | "smtp" | "preview-only";
  detail?: string;
}

export function escapeHtml(raw: string): string {
  return raw
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function localizedStatusLabel(status: string, isZh: boolean): string {
  if (!isZh) return status.toUpperCase();
  switch (status) {
    case "paid":
      return "已付款 · 訂單已確認";
    case "fulfilled":
      return "已完成 · 已安排派遞";
    case "reserved":
      return "已留貨 · 待完成付款";
    case "expired":
      return "留貨時限已過";
    case "cancelled":
      return "已取消";
    case "late_payment_review":
      return "款項正由專員核實中";
    case "returned_quarantined":
      return "已退回 · 檢驗中";
    default:
      return status;
  }
}

function localizedPaymentMethodLabel(method: string, isZh: boolean): string {
  if (!isZh) return method.toUpperCase();
  return method === "fps" ? "香港轉數快 (FPS)" : "信用卡 (Stripe)";
}

/**
 * Generates a rich bilingual/localized HTML email receipt for a single-SKU or
 * multi-SKU order and optionally dispatches via Resend or SMTP when configured.
 */
export function buildOrderEmailHtml(payload: EmailNotificationPayload): {
  subject: string;
  html: string;
} {
  const isPaid = payload.status === "paid" || payload.status === "fulfilled";
  const isZh = payload.locale === "zh-HK";
  const formattedTotal = escapeHtml(formatMoney(payload.totalCents, payload.currency));
  const checkoutUrl = escapeHtml(
    `${payload.siteUrl}/${payload.locale}/checkout/${encodeURIComponent(payload.orderCode)}`,
  );
  const safeBrand = escapeHtml(payload.brandName);
  const safeOrderCode = escapeHtml(payload.orderCode);
  const safeMethod = escapeHtml(
    localizedPaymentMethodLabel(payload.paymentMethod, isZh),
  );
  const safeStatus = escapeHtml(localizedStatusLabel(payload.status, isZh));
  const safeFpsRef = payload.fpsReference
    ? escapeHtml(payload.fpsReference)
    : "";

  const resolvedItems: EmailOrderItemSummary[] =
    payload.items && payload.items.length > 0
      ? payload.items
      : [
          {
            productTitle: payload.productTitle,
            productSlug: payload.productSlug,
            quantity: payload.quantity,
            unitPriceCents: payload.unitPriceCents,
            lineTotalCents: payload.totalCents,
          },
        ];

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
      ? "Payment Confirmed · Your Pieces Are Officially Secured"
      : "Pieces Reserved · Complete Settlement Before Hold Expiry";

  const itemsRowsHtml = resolvedItems
    .map(
      (item) => `
      <tr>
        <td style="padding:10px 0;color:#1e1b18;font-weight:600;">
          ${escapeHtml(item.productTitle)} × ${Number(item.quantity)}
        </td>
        <td style="padding:10px 0;text-align:right;color:#1e1b18;font-weight:600;">
          ${escapeHtml(formatMoney(item.lineTotalCents, payload.currency))}
        </td>
      </tr>`,
    )
    .join("");

  const html = `<!DOCTYPE html>
<html lang="${isZh ? "zh-Hant-HK" : "en"}">
<head><meta charset="utf-8" /><title>${escapeHtml(subject)}</title></head>
<body style="margin:0;padding:32px;background:#f6f1eb;color:#1e1b18;font-family:Georgia,serif;">
  <div style="max-width:600px;margin:0 auto;background:#fbf9f5;color:#1e1b18;border:1px solid #ddd3c2;padding:40px;">
    <div style="font-family:sans-serif;font-size:11px;letter-spacing:0.24em;text-transform:uppercase;color:#5a534a;">
      ${safeBrand} · ${isZh ? "香港典藏庫" : "HONG KONG VAULT"}
    </div>
    <h1 style="font-size:24px;font-weight:normal;margin:16px 0 8px;color:#1e1b18;">${statusHeading}</h1>
    <p style="font-family:sans-serif;font-size:14px;color:#5a534a;margin:0 0 24px;">
      ${isZh ? "訂單編號" : "Order Reference"}: <strong style="color:#1e1b18;">${safeOrderCode}</strong>
    </p>
    <table style="width:100%;border-collapse:collapse;font-family:sans-serif;font-size:14px;color:#1e1b18;border-top:1px solid #ddd3c2;border-bottom:1px solid #ddd3c2;margin:20px 0;">
      ${itemsRowsHtml}
      <tr>
        <td style="padding:12px 0;border-top:1px solid #ece5d8;color:#5a534a;">${isZh ? "付款方式" : "Settlement Method"}</td>
        <td style="padding:12px 0;border-top:1px solid #ece5d8;text-align:right;color:#1e1b18;font-weight:600;">${safeMethod}</td>
      </tr>
      <tr>
        <td style="padding:12px 0;color:#5a534a;">${isZh ? "訂單狀態" : "Current Status"}</td>
        <td style="padding:12px 0;text-align:right;font-weight:600;color:${isPaid ? "#2a6b3c" : "#b97a1f"};">${safeStatus}</td>
      </tr>
      ${safeFpsRef ? `<tr><td style="padding:12px 0;color:#5a534a;">${isZh ? "轉數快參考編號" : "FPS Reference"}</td><td style="padding:12px 0;text-align:right;color:#1e1b18;font-weight:600;">${safeFpsRef}</td></tr>` : ""}
      <tr>
        <td style="padding:14px 0;border-top:1px solid #ddd3c2;color:#1e1b18;font-weight:600;">${isZh ? "總計金額" : "Total Amount"}</td>
        <td style="padding:14px 0;border-top:1px solid #ddd3c2;text-align:right;font-size:18px;color:#1e1b18;font-weight:600;">${formattedTotal}</td>
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

/**
 * Sends a live HTML transactional email via Resend (`RESEND_API_KEY`) or
 * SMTP (`SMTP_HOST` / `SMTP_USER` / `SMTP_PASS`) when configured (EMAIL-01).
 * Never throws or blocks checkout if credentials are unset or unreachable.
 */
export async function dispatchTransactionalEmail(opts: {
  to: string | undefined;
  subject: string;
  html: string;
  brandName: string;
}): Promise<EmailDeliveryResult> {
  const recipient = (opts.to ?? "").trim();
  if (!recipient || !recipient.includes("@")) {
    return { dispatched: false, transport: "preview-only", detail: "no recipient email" };
  }

  const fromEmail =
    process.env.EMAIL_FROM?.trim() ||
    process.env.SMTP_FROM?.trim() ||
    process.env.SMTP_USER?.trim() ||
    "concierge@example.com";
  const fromHeader = `${opts.brandName} <${fromEmail}>`;

  // 1. Resend API transport when RESEND_API_KEY is configured. A Resend
  //    failure is NOT the end of the attempt (G15): warn and fall through to
  //    the SMTP transport below so one broken provider can never silence a
  //    paying customer.
  const resendKey = process.env.RESEND_API_KEY?.trim();
  let resendFailureDetail: string | null = null;
  if (resendKey) {
    try {
      const res = await fetch("https://api.resend.com/emails", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${resendKey}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          from: fromHeader,
          to: [recipient],
          subject: opts.subject,
          html: opts.html,
        }),
      });
      if (res.ok) {
        return { dispatched: true, transport: "resend" };
      }
      resendFailureDetail = `Resend HTTP ${res.status}`;
    } catch (err) {
      resendFailureDetail =
        err instanceof Error ? err.message : "Resend network error";
    }
    console.warn(
      `[email-receipt] Resend delivery to ${recipient} failed (${resendFailureDetail}); trying SMTP fallback if configured.`,
    );
  }

  // 2. Direct SMTPS (465) or STARTTLS (587) transport when SMTP_USER and SMTP_PASS are set
  const smtpUser = process.env.SMTP_USER?.trim();
  const rawPass = process.env.SMTP_PASS?.trim();
  const smtpHost =
    process.env.SMTP_HOST?.trim() ||
    (smtpUser && rawPass ? "smtp.gmail.com" : "");
  const smtpPort = Number.parseInt(process.env.SMTP_PORT?.trim() || "587", 10);
  // Google Workspace / Gmail App Passwords are 16 alphanumeric characters;
  // normalize any spaces or hyphens (e.g. xxxx-xxxx-xxxx-xxxx) for AUTH LOGIN.
  const smtpPass =
    rawPass && smtpHost.includes("gmail.com")
      ? rawPass.replace(/[\s-]+/g, "")
      : rawPass;

  if (smtpHost && smtpUser && smtpPass) {
    try {
      await sendViaSmtp({
        host: smtpHost,
        port: Number.isFinite(smtpPort) ? smtpPort : 587,
        user: smtpUser,
        pass: smtpPass,
        fromEmail,
        fromHeader,
        to: recipient,
        subject: opts.subject,
        html: opts.html,
      });
      console.log(
        `[email-receipt] Dispatched email via SMTP (${smtpHost}:${smtpPort}) to ${recipient} — Subject: ${opts.subject}`,
      );
      return { dispatched: true, transport: "smtp" };
    } catch (err) {
      const msg = err instanceof Error ? err.message : "SMTP error";
      console.error(
        `[email-receipt] SMTP delivery failed (${smtpHost}:${smtpPort}) to ${recipient}: ${msg}`,
      );
      return {
        dispatched: false,
        transport: "smtp",
        detail: [resendFailureDetail, msg].filter(Boolean).join("; "),
      };
    }
  }

  return {
    dispatched: false,
    transport: resendFailureDetail ? "resend" : "preview-only",
    detail: resendFailureDetail ?? "Email transport not configured or failed",
  };
}

/**
 * Atomically claims THE one paid-confirmation email send for an order
 * (`POST /api/orders/{order_code}/email-delivery` with `{ claim: true, sent:
 * false }`, findings G02). Returns `true` only when THIS caller won the claim
 * (`data.claimed === true`): concurrent settlement paths (webhook-only,
 * session-verify return, direct card, admin FPS confirmation) must dispatch
 * the paid receipt exactly once, and only the claimer may send it. Any doubt
 * (HTTP error, network hiccup) answers `false` — a missing receipt is
 * visible and repairable via "Re-send Receipt", a duplicate is not.
 */
export async function claimPaidOrderEmail(
  apiBaseUrl: string,
  orderCode: string,
): Promise<boolean> {
  const base = apiBaseUrl.replace(/\/+$/, "");
  try {
    const res = await fetch(
      `${base}/api/orders/${encodeURIComponent(orderCode)}/email-delivery`,
      {
        method: "POST",
        cache: "no-store",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ claim: true, sent: false }),
      },
    );
    if (!res.ok) {
      console.warn(
        `[email-receipt] Could not claim the paid email for ${orderCode} (HTTP ${res.status})`,
      );
      return false;
    }
    const data = (await res.json().catch(() => ({}))) as { claimed?: boolean };
    return data.claimed === true;
  } catch (err) {
    console.warn(
      `[email-receipt] Could not claim the paid email for ${orderCode}: ${
        err instanceof Error ? err.message : "network error"
      }`,
    );
    return false;
  }
}

/**
 * Reports the OUTCOME of one email attempt to the Rust API delivery ledger
 * (`POST /api/orders/{order_code}/email-delivery`, findings G02). Success on
 * the paid receipt stamps `paid_email_sent_at` (the durable "customer was
 * told" flag that de-duplicates later sends); a FAILED attempt (`sent:
 * false`) RELEASES the in-flight claim and stores `email_last_error`, so a
 * retry or the admin "Re-send Receipt" surface can still reach the customer —
 * without ever wiping an already-confirmed `paid_email_sent_at`.
 *
 * `kind` names which email this outcome belongs to: `"reservation"` (the hold
 * confirmation) only clears `email_last_error` and never stamps the PAID
 * email ledger; `"paid"` (the default) is the paid receipt. Best-effort by
 * design: a ledger hiccup must never break checkout or the webhook reply.
 */
export type EmailDeliveryKind = "reservation" | "paid";

export async function recordOrderEmailDelivery(
  apiBaseUrl: string,
  orderCode: string,
  result: EmailDeliveryResult,
  kind?: EmailDeliveryKind,
): Promise<void> {
  const base = apiBaseUrl.replace(/\/+$/, "");
  // The API rejects NUL bytes and notes > 2000 chars — sanitize first.
  const errorNote = result.dispatched
    ? ""
    : (result.detail ?? "Email transport not configured or failed")
        .replace(/\u0000/g, " ")
        .slice(0, 2000);
  try {
    const res = await fetch(
      `${base}/api/orders/${encodeURIComponent(orderCode)}/email-delivery`,
      {
        method: "POST",
        cache: "no-store",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          sent: result.dispatched,
          error: errorNote,
          ...(kind ? { kind } : {}),
        }),
      },
    );
    if (!res.ok) {
      console.warn(
        `[email-receipt] Could not record email delivery for ${orderCode} (HTTP ${res.status})`,
      );
    }
  } catch (err) {
    console.warn(
      `[email-receipt] Could not record email delivery for ${orderCode}: ${
        err instanceof Error ? err.message : "network error"
      }`,
    );
  }
}

async function sendViaSmtp(cfg: {
  host: string;
  port: number;
  user: string;
  pass: string;
  fromEmail: string;
  fromHeader: string;
  to: string;
  subject: string;
  html: string;
}): Promise<void> {
  return new Promise((resolve, reject) => {
    const implicitTls = cfg.port === 465;
    let activeSocket: net.Socket | tls.TLSSocket = implicitTls
      ? tls.connect({ host: cfg.host, port: cfg.port, servername: cfg.host })
      : net.createConnection({ host: cfg.host, port: cfg.port });

    activeSocket.setTimeout(8000);
    let buffer = "";
    let step = 0;
    let tlsUpgraded = implicitTls;

    const writeCmd = (cmd: string) => {
      activeSocket.write(`${cmd}\r\n`);
    };

    const fail = (msg: string) => {
      activeSocket.destroy();
      reject(new Error(msg));
    };

    const attachListeners = (sock: net.Socket | tls.TLSSocket) => {
      sock.on("timeout", () => fail("SMTP connection timed out"));
      sock.on("error", (err) => fail(err.message));
      sock.on("data", onData);
    };

    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      const lines = buffer.split("\r\n");
      buffer = lines.pop() ?? "";

      for (const line of lines) {
        if (line.length < 3) continue;
        if (line[3] === "-") continue;
        const code = Number.parseInt(line.slice(0, 3), 10);
        if (code >= 400) {
          fail(`SMTP error ${line}`);
          return;
        }

        if (step === 0 && code === 220) {
          step = 1;
          writeCmd("EHLO localhost");
        } else if (step === 1 && code === 250) {
          if (!tlsUpgraded) {
            step = 10;
            writeCmd("STARTTLS");
          } else {
            step = 2;
            writeCmd("AUTH LOGIN");
          }
        } else if (step === 10 && code === 220) {
          // Upgrade plaintext socket to TLS before sending credentials
          activeSocket.removeAllListeners("data");
          activeSocket.removeAllListeners("error");
          activeSocket.removeAllListeners("timeout");
          const upgraded = tls.connect({
            socket: activeSocket,
            servername: cfg.host,
          });
          upgraded.setTimeout(8000);
          activeSocket = upgraded;
          tlsUpgraded = true;
          buffer = "";
          step = 1;
          attachListeners(upgraded);
          upgraded.once("secureConnect", () => {
            writeCmd("EHLO localhost");
          });
          return;
        } else if (step === 2 && code === 334) {
          step = 3;
          writeCmd(Buffer.from(cfg.user, "utf8").toString("base64"));
        } else if (step === 3 && code === 334) {
          step = 4;
          writeCmd(Buffer.from(cfg.pass, "utf8").toString("base64"));
        } else if (step === 4 && code === 235) {
          step = 5;
          writeCmd(`MAIL FROM:<${cfg.fromEmail}>`);
        } else if (step === 5 && code === 250) {
          step = 6;
          writeCmd(`RCPT TO:<${cfg.to}>`);
        } else if (step === 6 && code === 250) {
          step = 7;
          writeCmd("DATA");
        } else if (step === 7 && code === 354) {
          step = 8;
          const b64Subject = `=?UTF-8?B?${Buffer.from(cfg.subject, "utf8").toString("base64")}?=`;
          const b64Html = Buffer.from(cfg.html, "utf8")
            .toString("base64")
            .replace(/(.{76})/g, "$1\r\n");
          const msg = [
            `From: ${cfg.fromHeader}`,
            `To: <${cfg.to}>`,
            `Subject: ${b64Subject}`,
            "MIME-Version: 1.0",
            'Content-Type: text/html; charset="UTF-8"',
            "Content-Transfer-Encoding: base64",
            "",
            b64Html,
            ".",
          ].join("\r\n");
          writeCmd(msg);
        } else if (step === 8 && code === 250) {
          step = 9;
          writeCmd("QUIT");
          activeSocket.end();
          resolve();
        }
      }
    };

    attachListeners(activeSocket);
  });
}
