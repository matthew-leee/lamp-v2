import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildWhatsAppUrl,
  computeSavingsPercent,
  formatCentsToDollarsInput,
  formatEditionBadge,
  formatMoney,
  formatProductTitle,
  interpolateCopy,
  parseDollarsToCents,
} from "../lib/format.ts";
import { resolveSiteConfig } from "../lib/api.ts";
import {
  buildOrderEmailHtml,
  claimPaidOrderEmail,
  dispatchTransactionalEmail,
  escapeHtml,
  recordOrderEmailDelivery,
} from "../lib/email-receipt.ts";
import { buildOrderReceiptEmail } from "../lib/order-email.ts";
import {
  MAX_UPLOAD_BYTES,
  sniffImageMime,
  validateUploadImage,
} from "../lib/upload-validation.ts";
import {
  SETTING_VALUE_RULES,
  validateSettingValue,
} from "../lib/settings-validation.ts";
import {
  PENDING_ATTEMPT_MAX_AGE_MS,
  PENDING_ATTEMPT_STORAGE_KEY,
  clearPendingAttemptKey,
  getOrCreatePendingAttemptKey,
  hasPendingAttemptForSlug,
  newIdempotencyKey,
  pendingAttemptFingerprint,
} from "../lib/recent-order.ts";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function listSourceFiles(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...listSourceFiles(full));
    } else if (entry.isFile() && /\.(ts|tsx)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:\\])\/\/.*$/gm, "$1");
}

test("white-label static scan: zero hardcoded brand or maker literals in web/{app,components,lib}", () => {
  const dirs = ["app", "components", "lib"].map((d) => path.join(webRoot, d));
  const files = dirs.flatMap(listSourceFiles);
  assert.ok(files.length >= 20, `expected >= 20 source files, got ${files.length}`);

  for (const file of files) {
    const rel = path.relative(webRoot, file);
    const code = stripComments(fs.readFileSync(file, "utf8"));
    assert.doesNotMatch(
      code,
      /\bHVILE\b|\bHAVN\b|\bLYSIE\b/i,
      `${rel} must not hardcode brand names`,
    );
    assert.doesNotMatch(
      code,
      /10849201/,
      `${rel} must not hardcode FPS merchant ID (FPS-01)`,
    );
  }
});

test("CART-01 terminology: UI uses Shopping Cart / Cart (購物車) and never Bag", () => {
  const i18nSource = fs.readFileSync(path.join(webRoot, "lib/i18n.ts"), "utf8");
  assert.match(i18nSource, /Shopping Cart/);
  assert.match(i18nSource, /Add to Cart/);
  assert.match(i18nSource, /購物車/);
  assert.match(i18nSource, /加入購物車/);
  assert.doesNotMatch(i18nSource, /Add to Bag|Collector's Bag|購物袋/);
});

test("resolveSiteConfig: presence-wins settings overrides including FPS-01 keys", () => {
  const config = resolveSiteConfig({
    "brand.name": "KRONOS",
    "brand.name_native": "克羅諾斯",
    "site.url": "https://kronos.example",
    currency: "EUR",
    "contact.whatsapp": "",
    "payment.fps_id": "99887766",
    "payment.fps_account_name": "KRONOS HK LTD",
    "payment.fps_qr_url": "/uploads/fps-qr.png",
  });
  assert.equal(config.brandName, "KRONOS");
  assert.equal(config.brandNameNative, "克羅諾斯");
  assert.equal(config.siteUrl, "https://kronos.example");
  assert.equal(config.currency, "EUR");
  assert.equal(config.whatsappNumber, null);
  assert.equal(config.fpsId, "99887766");
  assert.equal(config.fpsAccountName, "KRONOS HK LTD");
  assert.equal(config.fpsQrUrl, "/uploads/fps-qr.png");
});

test("formatEditionBadge: crash-proof when a newly created product has 0 pieces", () => {
  assert.equal(formatEditionBadge([], 16, "en"), "Open Edition");
  assert.equal(formatEditionBadge([], 16, "zh-HK"), "常設單品");
  assert.equal(formatEditionBadge([1, 2], 16, "en"), "Pieces 01–02 of 16");
  assert.equal(formatEditionBadge([15], 16, "en"), "Piece 15 of 16");
  assert.equal(formatEditionBadge([15], 16, "zh-HK"), "編號 15 / 共 16 件");
});

test("formatMoney and human dollar <-> integer cents conversions", () => {
  assert.equal(formatMoney(161500, "HKD"), "HK$1,615");
  assert.equal(formatMoney(714000, "HKD"), "HK$7,140");
  assert.equal(formatMoney(42550, "USD"), "$425.50");
  assert.equal(parseDollarsToCents("1615"), 161500);
  assert.equal(parseDollarsToCents("1,615.50"), 161550);
  assert.equal(parseDollarsToCents("-5"), null);
  assert.equal(formatCentsToDollarsInput(161500), "1615");
  assert.equal(formatCentsToDollarsInput(161550), "1615.50");
});

test("computeSavingsPercent: calculates showroom discount range accurately", () => {
  const savings = computeSavingsPercent(161500, 190000, 280000);
  assert.deepEqual(savings, { minPct: 15, maxPct: 42 });
  assert.equal(computeSavingsPercent(300000, 190000, 280000), null);
});

test("resolveSiteConfig: presence-wins settings overrides and white-label swap", () => {
  const config = resolveSiteConfig({
    "brand.name": "KRONOS",
    "brand.name_native": "克羅諾斯",
    "site.url": "https://kronos.example",
    currency: "EUR",
    "contact.whatsapp": "",
    "payment.fps_id": "",
  });
  assert.equal(config.brandName, "KRONOS");
  assert.equal(config.brandNameNative, "克羅諾斯");
  assert.equal(config.siteUrl, "https://kronos.example");
  assert.equal(config.currency, "EUR");
  assert.equal(config.whatsappNumber, null);
  assert.equal(config.fpsId, null);
});

test("formatProductTitle & interpolateCopy & buildWhatsAppUrl", () => {
  assert.equal(
    formatProductTitle("Panthella 160", "Panthella 160 便攜檯燈", "米色", "zh-HK"),
    "Panthella 160 便攜檯燈（米色）",
  );
  assert.equal(
    formatProductTitle("Panthella 160", "Panthella 160 便攜檯燈", "Beige", "en"),
    "Panthella 160 (Beige)",
  );
  assert.equal(
    interpolateCopy("{pieces} pieces by {brand}", { brand: "LYSIE", pieces: 16 }),
    "16 pieces by LYSIE",
  );
  assert.equal(
    buildWhatsAppUrl("85291234567", "Hello LYSIE"),
    "https://wa.me/85291234567?text=Hello%20LYSIE",
  );
  assert.equal(buildWhatsAppUrl("", "Hello"), null);
});

test("buildOrderEmailHtml: escapes XSS in customer fields and localizes zh-HK status (EMAIL-01, LANG-01)", () => {
  assert.equal(escapeHtml('<img src=x onerror="alert(1)">'), "&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
  const receipt = buildOrderEmailHtml({
    orderCode: "ORD-100",
    status: "paid",
    paymentMethod: "fps",
    productTitle: "Panthella 160",
    productSlug: "panthella-160-v3-beige",
    quantity: 2,
    unitPriceCents: 161500,
    totalCents: 323000,
    currency: "HKD",
    fpsReference: '<script>alert("xss")</script>',
    expiresAt: "2026-10-16T12:00:00Z",
    createdAt: "2026-10-16T10:00:00Z",
    brandName: "ATELIER",
    siteUrl: "https://shop.example.com",
    locale: "zh-HK",
  });
  assert.doesNotMatch(receipt.html, /<script>/i);
  assert.match(receipt.html, /&lt;script&gt;alert\(&quot;xss&quot;\)&lt;\/script&gt;/);
  assert.match(receipt.html, /已付款 · 訂單已確認/);
  assert.match(receipt.html, /香港轉數快 \(FPS\)/);
});

// ---------------------------------------------------------------------------
// Bounded Task 2 — email receipt generation (EMAIL-01, findings G02/G15)
// ---------------------------------------------------------------------------

test("buildOrderEmailHtml: en multi-SKU paid receipt renders every line and the total (EMAIL-01)", () => {
  const receipt = buildOrderEmailHtml({
    orderCode: "ORD-200",
    status: "paid",
    paymentMethod: "card",
    productTitle: "Panthella 160",
    productSlug: "panthella-160-v3-beige",
    quantity: 1,
    unitPriceCents: 161500,
    totalCents: 232900,
    currency: "HKD",
    items: [
      {
        productTitle: "Panthella 160 (Beige)",
        productSlug: "panthella-160-v3-beige",
        quantity: 1,
        unitPriceCents: 161500,
        lineTotalCents: 161500,
      },
      {
        productTitle: "PH 3/2 Table (Chrome)",
        productSlug: "ph-3-2-table-chrome",
        quantity: 1,
        unitPriceCents: 71400,
        lineTotalCents: 71400,
      },
    ],
    expiresAt: "2026-10-16T12:00:00Z",
    createdAt: "2026-10-16T10:00:00Z",
    brandName: "ATELIER",
    siteUrl: "https://shop.example.com",
    locale: "en",
  });
  assert.match(receipt.subject, /Payment Confirmed & Collector Pass/);
  assert.match(receipt.html, /Panthella 160 \(Beige\) × 1/);
  assert.match(receipt.html, /PH 3\/2 Table \(Chrome\) × 1/);
  assert.match(receipt.html, /HK\$2,329/);
  assert.match(receipt.html, /\/en\/checkout\/ORD-200/);
});

test("buildOrderReceiptEmail: localizes product titles and status per order locale (G02, LANG-01)", () => {
  const catalogue = {
    total_pieces: 16,
    products: [
      {
        slug: "panthella-160-v3-beige",
        brand: "B",
        model: "Panthella 160",
        price_cents: 161500,
        family: "Portable",
        active: true,
        art_variant: "dome",
        art_hue: 38,
        image_day_url: "",
        image_lit_url: "",
        designer: "Verner Panton",
        design_year: "1971",
        dimensions: "H16cm",
        light_source: "LED",
        content: {
          en: { model_alt: "Panthella 160 Portable", colourway: "Beige" },
          "zh-HK": { model_alt: "Panthella 160 便攜檯燈", colourway: "米色" },
        },
        reference_price: null,
        pieces: [1, 2],
        units_in_drop: 2,
      },
    ],
  };
  const order = {
    order_code: "ORD-9",
    product_slug: "panthella-160-v3-beige",
    quantity: 1,
    unit_price_cents: 161500,
    total_cents: 161500,
    currency: "HKD",
    payment_method: "fps",
    status: "paid",
    fps_reference: "FPS-REF-1",
    expires_at: "2026-10-16T12:00:00Z",
    created_at: "2026-10-16T10:00:00Z",
    items: [
      {
        product_slug: "panthella-160-v3-beige",
        quantity: 1,
        unit_price_cents: 161500,
        line_total_cents: 161500,
      },
    ],
  };
  const brand = { brandName: "ATELIER", siteUrl: "https://shop.example.com" };
  const zh = buildOrderReceiptEmail(order, catalogue, brand, "zh-HK");
  assert.ok(zh.html.includes("Panthella 160 便攜檯燈（米色）"));
  assert.match(zh.html, /已付款 · 訂單已確認/);
  const en = buildOrderReceiptEmail(order, catalogue, brand, "en");
  // `model_alt` is the CJK-only title: English keeps the catalogue `model`.
  assert.match(en.html, /Panthella 160 \(Beige\)/);
  assert.match(en.html, /PAID/);
});

test("recordOrderEmailDelivery: posts the sent/error ledger payload to the Rust API (G02)", async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    return new Response("{}", { status: 200 });
  };
  try {
    await recordOrderEmailDelivery("http://api:8080/", "ORD-1/2", {
      dispatched: true,
      transport: "smtp",
    });
    await recordOrderEmailDelivery("http://api:8080", "ORD-9", {
      dispatched: false,
      transport: "resend",
      detail: "Resend HTTP 500",
    });
    await recordOrderEmailDelivery("http://api:8080", "ORD-10", {
      dispatched: false,
      transport: "preview-only",
    });
    // The re-sent HOLD confirmation reports `kind: "reservation"` so the API
    // never stamps the PAID-email ledger for it (findings G02).
    await recordOrderEmailDelivery(
      "http://api:8080",
      "ORD-11",
      { dispatched: true, transport: "smtp" },
      "reservation",
    );
    await recordOrderEmailDelivery(
      "http://api:8080",
      "ORD-12",
      { dispatched: true, transport: "smtp" },
      "paid",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.equal(calls.length, 5);
  assert.equal(
    calls[0].url,
    "http://api:8080/api/orders/ORD-1%2F2/email-delivery",
  );
  assert.equal(calls[0].init.method, "POST");
  assert.deepEqual(JSON.parse(calls[0].init.body), { sent: true, error: "" });
  assert.deepEqual(JSON.parse(calls[1].init.body), {
    sent: false,
    error: "Resend HTTP 500",
  });
  assert.deepEqual(JSON.parse(calls[2].init.body), {
    sent: false,
    error: "Email transport not configured or failed",
  });
  assert.deepEqual(
    JSON.parse(calls[3].init.body),
    { sent: true, error: "", kind: "reservation" },
    "the kind rides along when the caller names the email",
  );
  assert.deepEqual(JSON.parse(calls[4].init.body), {
    sent: true,
    error: "",
    kind: "paid",
  });
});

test("claimPaidOrderEmail: only the atomic claim winner may send (G02)", async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    // First call wins the claim, the duplicate loses it.
    return new Response(
      JSON.stringify({ ok: true, paid_email_sent: true, claimed: calls.length === 1 }),
      { status: 200 },
    );
  };
  try {
    assert.equal(await claimPaidOrderEmail("http://api:8080/", "ORD-7"), true);
    assert.equal(
      await claimPaidOrderEmail("http://api:8080", "ORD-7"),
      false,
      "a lost claim must stay silent (no second receipt)",
    );
    globalThis.fetch = async () => new Response("{}", { status: 503 });
    assert.equal(
      await claimPaidOrderEmail("http://api:8080", "ORD-7"),
      false,
      "an unreachable ledger must fail closed",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.equal(calls.length, 2);
  assert.equal(
    calls[0].url,
    "http://api:8080/api/orders/ORD-7/email-delivery",
  );
  assert.equal(calls[0].init.method, "POST");
  assert.deepEqual(JSON.parse(calls[0].init.body), { claim: true, sent: false });
});

test("dispatchTransactionalEmail: Resend failure falls through to SMTP instead of giving up (G15)", async () => {
  const savedEnv = {
    RESEND_API_KEY: process.env.RESEND_API_KEY,
    SMTP_HOST: process.env.SMTP_HOST,
    SMTP_PORT: process.env.SMTP_PORT,
    SMTP_USER: process.env.SMTP_USER,
    SMTP_PASS: process.env.SMTP_PASS,
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response("{}", { status: 500 });
  process.env.RESEND_API_KEY = "test_key";
  // Local-only SMTP endpoint that refuses instantly: no external network.
  process.env.SMTP_HOST = "127.0.0.1";
  process.env.SMTP_PORT = "1";
  process.env.SMTP_USER = "user";
  process.env.SMTP_PASS = "pass";
  try {
    const result = await dispatchTransactionalEmail({
      to: "collector@example.com",
      subject: "S",
      html: "<p>h</p>",
      brandName: "ATELIER",
    });
    // The attempt REACHED the SMTP branch (transport "smtp") instead of
    // returning early with the Resend failure — the fallback ran.
    assert.equal(result.dispatched, false);
    assert.equal(result.transport, "smtp");
    assert.match(result.detail ?? "", /^Resend HTTP 500; /);
  } finally {
    globalThis.fetch = originalFetch;
    for (const [key, value] of Object.entries(savedEnv)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});

test("dispatchTransactionalEmail: Resend-only failure is reported honestly when no SMTP fallback exists (G15)", async () => {
  const savedEnv = {
    RESEND_API_KEY: process.env.RESEND_API_KEY,
    SMTP_HOST: process.env.SMTP_HOST,
    SMTP_PORT: process.env.SMTP_PORT,
    SMTP_USER: process.env.SMTP_USER,
    SMTP_PASS: process.env.SMTP_PASS,
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response("{}", { status: 503 });
  process.env.RESEND_API_KEY = "test_key";
  delete process.env.SMTP_HOST;
  delete process.env.SMTP_PORT;
  delete process.env.SMTP_USER;
  delete process.env.SMTP_PASS;
  try {
    const result = await dispatchTransactionalEmail({
      to: "collector@example.com",
      subject: "S",
      html: "<p>h</p>",
      brandName: "ATELIER",
    });
    assert.equal(result.dispatched, false);
    assert.equal(result.transport, "resend");
    assert.equal(result.detail, "Resend HTTP 503");
  } finally {
    globalThis.fetch = originalFetch;
    for (const [key, value] of Object.entries(savedEnv)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});

// ---------------------------------------------------------------------------
// Bounded Task 2 — upload magic-byte validation (findings G18)
// ---------------------------------------------------------------------------

function bytesOf(...parts) {
  return Buffer.concat(
    parts.map((p) => (typeof p === "string" ? Buffer.from(p, "latin1") : Buffer.from(p))),
  );
}

test("upload validation: magic bytes prove JPEG / PNG / WebP / AVIF and pick the canonical extension (G18)", () => {
  const jpeg = bytesOf([0xff, 0xd8, 0xff, 0xe0], "JFIF-payload");
  assert.deepEqual(validateUploadImage({ declaredType: "image/jpeg", bytes: jpeg }), {
    ok: true,
    mime: "image/jpeg",
    ext: ".jpg",
  });

  const png = bytesOf([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], "payload");
  assert.deepEqual(validateUploadImage({ declaredType: "image/png", bytes: png }), {
    ok: true,
    mime: "image/png",
    ext: ".png",
  });

  const webp = bytesOf("RIFF", [0, 0, 0, 0], "WEBPVP8 ");
  assert.deepEqual(validateUploadImage({ declaredType: "image/webp", bytes: webp }), {
    ok: true,
    mime: "image/webp",
    ext: ".webp",
  });

  for (const box of ["ftypavif", "ftypavis"]) {
    const avif = bytesOf([0, 0, 0, 24], box, [0, 0, 0, 0], "payload");
    assert.deepEqual(
      validateUploadImage({ declaredType: "image/avif", bytes: avif }),
      { ok: true, mime: "image/avif", ext: ".avif" },
      `AVIF box ${box} must be accepted`,
    );
    assert.equal(sniffImageMime(avif), "image/avif");
  }
});

test("upload validation: SVG, HTML, GIF and mismatched magic bytes are all rejected (G18)", () => {
  const svg = bytesOf('<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>');
  assert.equal(validateUploadImage({ declaredType: "image/svg+xml", bytes: svg }).ok, false);
  assert.equal(validateUploadImage({ declaredType: "image/png", bytes: svg }).ok, false);

  const html = bytesOf("<!DOCTYPE html><html><body>phish</body></html>");
  assert.equal(validateUploadImage({ declaredType: "text/html", bytes: html }).ok, false);

  const gif = bytesOf("GIF89a", [0, 0, 0]);
  assert.equal(validateUploadImage({ declaredType: "image/gif", bytes: gif }).ok, false);
  assert.equal(validateUploadImage({ declaredType: "image/png", bytes: gif }).ok, false);

  // Real PNG bytes declared as JPEG: content and declaration must agree.
  const png = bytesOf([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], "x");
  const mismatch = validateUploadImage({ declaredType: "image/jpeg", bytes: png });
  assert.equal(mismatch.ok, false);
  assert.match(mismatch.error, /does not match the declared type/);

  // A 4-byte PNG prefix is NOT the PNG signature — all 8 bytes must match.
  const fakePng = bytesOf([0x89, 0x50, 0x4e, 0x47], "JUNK-not-a-png");
  assert.equal(
    validateUploadImage({ declaredType: "image/png", bytes: fakePng }).ok,
    false,
    "a truncated PNG signature must be rejected",
  );

  // Generic ISO-BMFF brand boxes are NOT proof of AVIF content (strict
  // magic bytes): `mif1`/`miaf` may wrap non-AVIF media, `heic` is another
  // format entirely.
  for (const box of ["ftypmif1", "ftypmiaf", "ftypheic"]) {
    const bmff = bytesOf([0, 0, 0, 24], box, [0, 0, 0, 0], "payload");
    assert.equal(sniffImageMime(bmff), null, `box ${box} must be refused`);
    assert.equal(
      validateUploadImage({ declaredType: "image/avif", bytes: bmff }).ok,
      false,
      `box ${box} must not pass upload validation`,
    );
  }

  const empty = validateUploadImage({ declaredType: "image/jpeg", bytes: Buffer.alloc(0) });
  assert.equal(empty.ok, false);
  assert.match(empty.error, /empty/);

  const oversized = validateUploadImage({
    declaredType: "image/jpeg",
    bytes: Buffer.alloc(MAX_UPLOAD_BYTES + 1, 0xab),
  });
  assert.equal(oversized.ok, false);
  assert.match(oversized.error, /16MB/);
});

// ---------------------------------------------------------------------------
// Bounded Task 2 — settings save-time validation (findings G23)
// ---------------------------------------------------------------------------

test("settings validation: GA / WhatsApp / site URL formats are enforced when non-empty (G23)", () => {
  assert.deepEqual(validateSettingValue("analytics.ga_measurement_id", "G-ABC12345"), {
    ok: true,
  });
  assert.equal(validateSettingValue("analytics.ga_measurement_id", "  G-ABC12345  ").ok, true);
  assert.equal(validateSettingValue("analytics.ga_measurement_id", "G-abc").ok, false);
  assert.equal(validateSettingValue("analytics.ga_measurement_id", "UA-123456").ok, false);
  assert.equal(validateSettingValue("analytics.ga_measurement_id", "<script>" ).ok, false);
  // Empty = "clear the setting" — always allowed.
  assert.deepEqual(validateSettingValue("analytics.ga_measurement_id", ""), { ok: true });

  assert.equal(validateSettingValue("contact.whatsapp", "+852 9123-4567").ok, true);
  assert.equal(validateSettingValue("contact.whatsapp", "85291234567").ok, true);
  assert.equal(validateSettingValue("contact.whatsapp", "not-a-phone").ok, false);
  assert.equal(validateSettingValue("contact.whatsapp", "+1 (555) 000-1234").ok, false);
  // Digit boundaries: 7–15 actual digits (formatting spaces/dashes aside).
  assert.equal(validateSettingValue("contact.whatsapp", "1234567").ok, true, "7 digits is the minimum");
  assert.equal(validateSettingValue("contact.whatsapp", "  +12-34 567-890-12345  ").ok, true, "15 digits is the maximum");
  assert.equal(validateSettingValue("contact.whatsapp", "123456").ok, false, "6 digits is too short");
  assert.equal(validateSettingValue("contact.whatsapp", "+1234567890123456").ok, false, "16 digits is too long");
  assert.equal(validateSettingValue("contact.whatsapp", "+852-").ok, false, "punctuation is not a number");
  assert.equal(validateSettingValue("contact.whatsapp", "").ok, true);

  assert.equal(validateSettingValue("site.url", "https://shop.example.com").ok, true);
  assert.equal(validateSettingValue("site.url", "http://localhost:3002").ok, true);
  assert.equal(validateSettingValue("site.url", "  https://shop.example.com/path?q=1#frag  ").ok, true);
  assert.equal(validateSettingValue("site.url", "ftp://shop.example.com").ok, false);
  assert.equal(validateSettingValue("site.url", "javascript:alert(1)").ok, false);
  assert.equal(validateSettingValue("site.url", "https://").ok, false, "a hostname is required");
  assert.equal(validateSettingValue("site.url", "not a url").ok, false);
  assert.equal(validateSettingValue("site.url", "").ok, true);

  // Unlisted keys (e.g. brand.name) stay free-form.
  assert.deepEqual(validateSettingValue("brand.name", "anything at all"), { ok: true });
  assert.ok(SETTING_VALUE_RULES["site.url"] instanceof RegExp === false); // rules carry hint text
  assert.equal(typeof SETTING_VALUE_RULES["site.url"].hint, "string");
});

test("newIdempotencyKey: unique per pay attempt and API-safe (G01)", () => {
  const a = newIdempotencyKey();
  const b = newIdempotencyKey();
  assert.notEqual(a, b);
  assert.ok(a.length > 0 && a.length <= 128);
  assert.ok(!a.includes("\u0000"));
});

// ---------------------------------------------------------------------------
// Bounded Task 3 — durable pay-attempt identity across reload & tabs (G01)
// ---------------------------------------------------------------------------

/** Runs `fn` with a fresh in-memory `window.localStorage` fake. */
function withFakeStorage(fn) {
  const store = new Map();
  const fake = {
    getItem: (key) => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: (key) => store.delete(key),
  };
  const originalWindow = globalThis.window;
  globalThis.window = { localStorage: fake };
  try {
    fn(store);
  } finally {
    if (originalWindow === undefined) delete globalThis.window;
    else globalThis.window = originalWindow;
  }
}

test("pendingAttemptFingerprint: identity is items + payment method + email only (G01)", () => {
  const items = [
    { slug: "a", quantity: 1 },
    { slug: "b", quantity: 2 },
  ];
  const base = pendingAttemptFingerprint(items, "card", " Ada@Example.com ");
  assert.equal(
    base,
    pendingAttemptFingerprint(
      [
        { slug: "b", quantity: 2 },
        { slug: "a", quantity: 1 },
      ],
      "card",
      "ada@example.com",
    ),
    "cart order and email casing must not change the attempt",
  );
  assert.notEqual(
    base,
    pendingAttemptFingerprint(items, "fps", "ada@example.com"),
    "a different payment method is a different attempt",
  );
  assert.notEqual(
    base,
    pendingAttemptFingerprint(items, "card", "other@example.com"),
    "a different email is a different attempt",
  );
  assert.notEqual(
    base,
    pendingAttemptFingerprint(
      [{ slug: "a", quantity: 1 }, { slug: "b", quantity: 3 }],
      "card",
      "ada@example.com",
    ),
    "different quantities are a different attempt",
  );
});

test("getOrCreatePendingAttemptKey: survives reload/tabs and retires on success (G01)", () => {
  withFakeStorage(() => {
    const fingerprint = pendingAttemptFingerprint(
      [{ slug: "a", quantity: 1 }],
      "card",
      "ada@example.com",
    );
    const key = getOrCreatePendingAttemptKey(fingerprint);
    assert.ok(key.length > 0 && key.length <= 128);
    assert.equal(
      getOrCreatePendingAttemptKey(fingerprint),
      key,
      "a refresh or a second TAB retrying the same cart must reuse the SAME key",
    );

    const otherFingerprint = pendingAttemptFingerprint(
      [{ slug: "a", quantity: 2 }],
      "card",
      "ada@example.com",
    );
    assert.notEqual(
      getOrCreatePendingAttemptKey(otherFingerprint),
      key,
      "a different cart is a different pay attempt",
    );

    clearPendingAttemptKey(fingerprint);
    assert.notEqual(
      getOrCreatePendingAttemptKey(fingerprint),
      key,
      "once the reservation succeeded the NEXT pay tap is a new order",
    );
    clearPendingAttemptKey(fingerprint);
  });
});

test("getOrCreatePendingAttemptKey: stale attempt identities expire after 30 minutes (G01)", () => {
  withFakeStorage((store) => {
    const fingerprint = "a x1|card|ada@example.com";
    const key = getOrCreatePendingAttemptKey(fingerprint);
    // Age the stored attempt past the TTL without sleeping.
    const raw = JSON.parse(store.get(PENDING_ATTEMPT_STORAGE_KEY));
    raw[fingerprint].created_at = new Date(
      Date.now() - PENDING_ATTEMPT_MAX_AGE_MS - 1000,
    ).toISOString();
    store.set(PENDING_ATTEMPT_STORAGE_KEY, JSON.stringify(raw));
    assert.notEqual(
      getOrCreatePendingAttemptKey(fingerprint),
      key,
      "an attempt older than the TTL is a NEW pay tap",
    );
  });
});

test("pendingAttemptFingerprint: `${slug}:` namespace prefix is order-insensitive (G01 recovery)", () => {
  const single = pendingAttemptFingerprint(
    [{ slug: "lamp-x", quantity: 1 }],
    "card",
    "ada@example.com",
  );
  assert.ok(
    single.startsWith("lamp-x:"),
    `single-SKU attempts live in their product namespace, got ${single}`,
  );
  const multi = pendingAttemptFingerprint(
    [
      { slug: "b", quantity: 2 },
      { slug: "a", quantity: 1 },
    ],
    "card",
    "ada@example.com",
  );
  assert.ok(
    multi.startsWith("a:"),
    "the sorted lead SKU is the namespace, cart order aside",
  );
});

test("hasPendingAttemptForSlug: a lost response on the last unit stays recoverable (G01)", () => {
  withFakeStorage((store) => {
    assert.equal(
      hasPendingAttemptForSlug("lamp-x"),
      false,
      "no attempt, no recovery affordance",
    );
    const fingerprint = pendingAttemptFingerprint(
      [{ slug: "lamp-x", quantity: 1 }],
      "card",
      "ada@example.com",
    );
    getOrCreatePendingAttemptKey(fingerprint);
    assert.equal(
      hasPendingAttemptForSlug("lamp-x"),
      true,
      "the replayable attempt opens the checkout even when stock reads 0",
    );
    assert.equal(
      hasPendingAttemptForSlug("lamp-y"),
      false,
      "another product's namespace must not match",
    );
    assert.equal(
      hasPendingAttemptForSlug("lamp"),
      false,
      "the prefix boundary is `${slug}:` — a slug that is only a PREFIX must not match",
    );

    // Expired attempts are invisible: recovery never replays a dead tap.
    const raw = JSON.parse(store.get(PENDING_ATTEMPT_STORAGE_KEY));
    raw[fingerprint].created_at = new Date(
      Date.now() - PENDING_ATTEMPT_MAX_AGE_MS - 1000,
    ).toISOString();
    store.set(PENDING_ATTEMPT_STORAGE_KEY, JSON.stringify(raw));
    assert.equal(hasPendingAttemptForSlug("lamp-x"), false);
  });
  // Storage failure / SSR: recovery is an affordance, never a blocker.
  assert.equal(hasPendingAttemptForSlug("lamp-x"), false);
});
