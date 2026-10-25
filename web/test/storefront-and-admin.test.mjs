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
import {
  CSV_STOCK_CONFLICT_MODES,
  CSV_SYNC_MAX_BYTES,
  DEFAULT_CSV_STOCK_CONFLICT_MODE,
  buildCsvSyncPayload,
  catalogueProductFromOrderItem,
  coerceArtVariant,
  csvApiPath,
  normalizeCsvSyncReport,
  parseCsvSyncOptions,
  resolveSiteConfig,
  stripUtf8Bom,
} from "../lib/api.ts";
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
import { locales, ui } from "../lib/i18n.ts";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function listSourceFiles(dir, pattern = /\.(ts|tsx)$/) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...listSourceFiles(full, pattern));
    } else if (entry.isFile() && pattern.test(entry.name)) {
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

/*
  NAME-01: forbidden white-label literals are assembled from fragments so this
  file never embeds the brand strings it scans for. That lets the scan cover
  `test/` alongside app/components/lib without tripping over its own source.
*/
const FORBIDDEN_BRAND_TOKENS = [
  ["H", "V", "I", "L", "E"].join(""),
  ["H", "A", "V", "N"].join(""),
  ["L", "Y", "S", "I", "E"].join(""),
  ["L", "A", "Z", "Y", "L", "A", "M", "P"].join(""),
  ["A", "T", "E", "L", "I", "E", "R"].join(""),
];
const FORBIDDEN_BRAND_RE = new RegExp(
  FORBIDDEN_BRAND_TOKENS.map((token) => `\\b${token}\\b`).join("|"),
  "i",
);
// FPS-01 merchant ID, fragmented for the same self-scan-safe reason.
const FORBIDDEN_FPS_ID_RE = new RegExp(["10849", "201"].join(""));

test("white-label static scan: zero hardcoded brand or maker literals in web/{app,components,lib,test}", () => {
  const sourceDirs = ["app", "components", "lib"].map((d) => path.join(webRoot, d));
  const files = [
    ...sourceDirs.flatMap((dir) => listSourceFiles(dir)),
    ...listSourceFiles(path.join(webRoot, "test"), /\.(mjs|ts|tsx)$/),
  ];
  assert.ok(files.length >= 20, `expected >= 20 source files, got ${files.length}`);

  for (const file of files) {
    const rel = path.relative(webRoot, file);
    const code = stripComments(fs.readFileSync(file, "utf8"));
    assert.doesNotMatch(
      code,
      FORBIDDEN_BRAND_RE,
      `${rel} must not hardcode brand names (NAME-01)`,
    );
    assert.doesNotMatch(
      code,
      FORBIDDEN_FPS_ID_RE,
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

test("W1-B foundation: exhibition, carousel and atmosphere labels exist in both locales", () => {
  const keys = [
    "defaultExhibitionTitle",
    "atmosphereDayShort",
    "atmosphereNightShort",
    "heroCarouselPause",
    "heroCarouselPlay",
  ];
  for (const key of keys) {
    const map = ui[key];
    assert.ok(map, `ui.${key} must exist`);
    for (const locale of locales) {
      assert.equal(typeof map[locale], "string", `ui.${key}.${locale} must be a string`);
      assert.ok(map[locale].trim().length > 0, `ui.${key}.${locale} must not be empty`);
    }
  }
  // Locale-specific copy is the contract W1-C renders; keep it pinned.
  assert.equal(ui.defaultExhibitionTitle.en, "LIGHTING EXHIBITION");
  assert.equal(ui.defaultExhibitionTitle["zh-HK"], "建築燈飾典藏展");
  assert.equal(ui.atmosphereDayShort.en, "5500K");
  assert.equal(ui.atmosphereNightShort.en, "2700K");
  assert.equal(ui.heroCarouselPause["zh-HK"], "暫停輪播");
  assert.equal(ui.heroCarouselPlay["zh-HK"], "繼續輪播");
});

test("W1-B foundation: fluid Utopia tokens, touch hygiene and reel-x rail live in globals.css", () => {
  const css = fs.readFileSync(path.join(webRoot, "app/globals.css"), "utf8");
  for (const token of [
    "--space-page-x",
    "--space-section-y",
    "--text-hero",
    "--text-h1",
    "--text-h2",
  ]) {
    assert.match(
      css,
      new RegExp(`${token}:\\s*clamp\\(`),
      `${token} must be a viewport-fluid clamp()`,
    );
  }
  assert.match(css, /touch-action:\s*manipulation/);
  assert.match(css, /\.reel-x\s*\{[^}]*overflow-x:\s*auto/);
  assert.match(css, /\.reel-x\s*\{[^}]*scroll-snap-type:\s*x mandatory/);
  assert.match(css, /\.reel-x\s*>\s*\*\s*\{[^}]*scroll-snap-align:\s*center/);
  assert.match(css, /\.reel-x::-webkit-scrollbar\s*\{\s*display:\s*none/);
  // The dual-atmosphere palette and intrinsic primitives must survive.
  assert.match(css, /:root\s*\{[\s\S]*?--surface-page:/);
  assert.match(css, /html\[data-atmosphere="night"\]\s*\{[\s\S]*?--surface-page:/);
  assert.match(css, /\.intrinsic-stack\s*\{/);
  assert.match(css, /\.intrinsic-stack-start\s*\{/);
  assert.match(css, /\.intrinsic-ghost\s*\{/);
  assert.match(css, /\.scroll-contain\s*\{/);
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
    interpolateCopy("{pieces} pieces by {brand}", { brand: "KRONOS", pieces: 16 }),
    "16 pieces by KRONOS",
  );
  assert.equal(
    buildWhatsAppUrl("85291234567", "Hello KRONOS"),
    "https://wa.me/85291234567?text=Hello%20KRONOS",
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
    brandName: "KRONOS",
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
    brandName: "KRONOS",
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
  const brand = { brandName: "KRONOS", siteUrl: "https://shop.example.com" };
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
      brandName: "KRONOS",
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
      brandName: "KRONOS",
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

// ---------------------------------------------------------------------------
// Bounded Task 2A — brand logo slot, 1-Click CSV bulk sync contract, Admin
// Studio concurrency guards and inactive-SKU checkout resilience
// (DATA-01 / ADMIN-01 / CART-01 / ENV-01)
// ---------------------------------------------------------------------------

function readSource(relativePath) {
  return fs.readFileSync(path.join(webRoot, relativePath), "utf8");
}

function withEnv(key, value, fn) {
  const saved = process.env[key];
  if (value === undefined) delete process.env[key];
  else process.env[key] = value;
  try {
    return fn();
  } finally {
    if (saved === undefined) delete process.env[key];
    else process.env[key] = saved;
  }
}

test("resolveSiteConfig: brand.logo_url is an env-overridable slot, empty override means no logo (ENV-01)", () => {
  // Presence wins: a stored override beats the bootstrap env default.
  withEnv("NEXT_PUBLIC_BRAND_LOGO_URL", "https://cdn.example/env-logo.png", () => {
    assert.equal(
      resolveSiteConfig({ "brand.logo_url": "/uploads/admin-logo.png" }).brandLogoUrl,
      "/uploads/admin-logo.png",
    );
    // No override -> env default.
    assert.equal(
      resolveSiteConfig({}).brandLogoUrl,
      "https://cdn.example/env-logo.png",
    );
    // An EMPTY override is an explicit "no logo": the header falls back to the
    // wordmark instead of leaking the env default back in.
    assert.equal(resolveSiteConfig({ "brand.logo_url": "" }).brandLogoUrl, null);
    assert.equal(
      resolveSiteConfig({ "brand.logo_url": "   " }).brandLogoUrl,
      null,
    );
  });
  // Nothing configured at all -> null, never an inline literal.
  withEnv("NEXT_PUBLIC_BRAND_LOGO_URL", undefined, () => {
    assert.equal(resolveSiteConfig({}).brandLogoUrl, null);
  });
});

test("site layout passes the resolved logo into the header slot (ENV-01)", () => {
  const layout = readSource("app/[lang]/layout.tsx");
  assert.match(layout, /brandLogoUrl=\{config\.brandLogoUrl\}/);
});

test("settings validation: brand.logo_url accepts upload paths and http(s) URLs only (ENV-01/G23)", () => {
  assert.ok(SETTING_VALUE_RULES["brand.logo_url"]);
  assert.equal(typeof SETTING_VALUE_RULES["brand.logo_url"].hint, "string");

  // Uploaded assets under the site's own tree.
  assert.equal(validateSettingValue("brand.logo_url", "/uploads/logo.png").ok, true);
  assert.equal(
    validateSettingValue("brand.logo_url", "/uploads/2026/brand-logo.webp").ok,
    true,
  );
  assert.equal(validateSettingValue("brand.logo_url", "  /uploads/logo.png  ").ok, true);
  // Absolute http(s) URLs.
  assert.equal(
    validateSettingValue("brand.logo_url", "https://cdn.example.com/logo.svg").ok,
    true,
  );
  assert.equal(
    validateSettingValue("brand.logo_url", "http://localhost:3002/logo.png").ok,
    true,
  );
  // Empty clears the override.
  assert.equal(validateSettingValue("brand.logo_url", "").ok, true);
  assert.equal(validateSettingValue("brand.logo_url", "   ").ok, true);

  // Refused: foreign schemes, protocol-relative hosts, bare filenames, markup.
  for (const bad of [
    "javascript:alert(1)",
    "data:image/svg+xml;base64,AAAA",
    "//evil.example/logo.png",
    "http:///logo.png",
    "logo.png",
    "/uploads/logo.png onerror=alert(1)",
    '/uploads/logo"onerror="alert(1)',
    "ftp://cdn.example.com/logo.png",
  ]) {
    assert.equal(
      validateSettingValue("brand.logo_url", bad).ok,
      false,
      `${bad} must be rejected`,
    );
  }
});

test("admin settings page exposes the brand.logo_url field with its own env fallback (ENV-01)", () => {
  const settings = readSource("app/admin/(authed)/settings/page.tsx");
  assert.match(settings, /key: "brand\.logo_url"/);
  assert.match(settings, /envVar: "NEXT_PUBLIC_BRAND_LOGO_URL"/);
});

test("coerceArtVariant: only real silhouettes render, anything else falls back (MEDIA-01)", () => {
  for (const variant of ["dome", "shades", "shades-tall", "piano", "none"]) {
    assert.equal(coerceArtVariant(variant), variant);
  }
  assert.equal(coerceArtVariant("unknown-shape"), "dome");
  assert.equal(coerceArtVariant(""), "dome");
  assert.equal(coerceArtVariant(undefined), "dome");
  assert.equal(coerceArtVariant(null, "none"), "none");
});

test("catalogueProductFromOrderItem: deactivated SKU keeps its title/photo/stage on checkout (CART-01/DATA-01)", () => {
  const snapshot = {
    product_id: 7,
    product_slug: "ph-3-2-table-chrome",
    quantity: 2,
    unit_price_cents: 71400,
    line_total_cents: 142800,
    model: "PH 3/2 Table",
    brand: "Maker",
    image_day_url: "/uploads/ph-day.png",
    image_lit_url: "/uploads/ph-lit.png",
    art_variant: "shades",
    art_hue: 41,
    dimensions: "Ø 290 × H 320 mm",
  };
  const rebuilt = catalogueProductFromOrderItem(snapshot);
  assert.ok(rebuilt, "an enriched item must rebuild a renderable product");
  assert.equal(rebuilt.slug, "ph-3-2-table-chrome");
  assert.equal(rebuilt.model, "PH 3/2 Table");
  assert.equal(rebuilt.brand, "Maker");
  assert.equal(rebuilt.price_cents, 71400);
  assert.equal(rebuilt.image_day_url, "/uploads/ph-day.png");
  assert.equal(rebuilt.image_lit_url, "/uploads/ph-lit.png");
  assert.equal(rebuilt.art_variant, "shades");
  assert.equal(rebuilt.art_hue, 41);
  assert.equal(rebuilt.dimensions, "Ø 290 × H 320 mm");
  // The reconstructed snapshot is never presented as a buyable catalogue row.
  assert.equal(rebuilt.active, false);
  assert.deepEqual(rebuilt.pieces, []);

  // Bad/missing art config degrades to a safe render instead of undefined.
  const bare = catalogueProductFromOrderItem({
    ...snapshot,
    art_variant: "nonsense",
    art_hue: Number.NaN,
    image_day_url: undefined,
    image_lit_url: undefined,
    dimensions: undefined,
  });
  assert.equal(bare.art_variant, "dome");
  assert.equal(bare.art_hue, 38);
  assert.equal(bare.image_day_url, "");
  assert.equal(bare.dimensions, "");

  // Legacy orders written before item enrichment keep the slug fallback.
  assert.equal(
    catalogueProductFromOrderItem({
      product_id: 1,
      product_slug: "legacy",
      quantity: 1,
      unit_price_cents: 100,
      line_total_cents: 100,
    }),
    null,
  );
  assert.equal(catalogueProductFromOrderItem(null), null);
  assert.equal(catalogueProductFromOrderItem(undefined), null);
});

test("checkout surfaces render the enriched order snapshot, not a bare slug (CART-01)", () => {
  const page = readSource("app/[lang]/checkout/[code]/page.tsx");
  assert.match(page, /catalogueProductFromOrderItem/);
  const client = readSource("components/checkout-settlement-client.tsx");
  assert.match(client, /catalogueProductFromOrderItem/);
});

test("bag drawer sends the cart total the customer saw so a mid-checkout price change cannot charge a different amount (CART-01)", () => {
  const source = readSource("components/bag-drawer.tsx");
  assert.match(source, /expected_total_cents: subtotalCents/);
});

test("studio draft: four copy fields per locale are always sent so clearing a box deletes the DB row (ADMIN-01)", () => {
  const studio = stripComments(
    readSource("components/admin-product-studio-client.tsx"),
  );
  // No conditional trimming: every field is part of the payload, empty included.
  assert.doesNotMatch(studio, /if \(enModelAlt\.trim\(\)\)/);
  assert.doesNotMatch(studio, /if \(zhModelAlt\.trim\(\)\)/);
  for (const field of ["model_alt", "colourway", "heritage", "electrical"]) {
    assert.match(
      studio,
      new RegExp(`en: \\{[^}]*${field}: en`),
      `en ${field} must be sent unconditionally`,
    );
    assert.match(
      studio,
      new RegExp(`"zh-HK": \\{[^}]*${field}: zh`),
      `zh-HK ${field} must be sent unconditionally`,
    );
  }
});

test("studio stale-tab guard: version_hash is typed, drafted and sent as expected_version_hash (ADMIN-01)", () => {
  const studio = readSource("components/admin-product-studio-client.tsx");
  assert.match(studio, /version_hash\?: string/);
  assert.match(studio, /baseVersionHash/);
  assert.match(studio, /payload\.expected_version_hash = baseVersionHash/);
  // The saved row rebases the form so the next save is not a self-conflict.
  assert.match(studio, /setBaseVersionHash\(data\.version_hash \?\? ""\)/);
});

test("studio slug lock: read-only by default behind an explicit unlock toggle (ADMIN-01)", () => {
  const studio = readSource("components/admin-product-studio-client.tsx");
  assert.match(studio, /readOnly=\{!slugUnlocked\}/);
  assert.match(studio, /Unlock URL Slug/);
  assert.match(studio, /setSlugUnlocked/);
});

test("studio stage preview: art_variant = none renders an explicit CAD placeholder (MEDIA-01)", () => {
  const studio = readSource("components/admin-product-studio-client.tsx");
  assert.match(studio, /artVariant === "none"/);
  assert.match(studio, /1:10 CAD Schematic Disabled \(art_variant = none\)/);
});

test("CSV sync contract: endpoint paths per kind and phase (DATA-01)", () => {
  assert.equal(csvApiPath("products", "export"), "/api/admin/csv/products");
  assert.equal(
    csvApiPath("products", "preview"),
    "/api/admin/csv/products/preview",
  );
  assert.equal(csvApiPath("products", "commit"), "/api/admin/csv/products/commit");
  assert.equal(csvApiPath("site-content", "export"), "/api/admin/csv/site-content");
  assert.equal(
    csvApiPath("site-content", "commit"),
    "/api/admin/csv/site-content/commit",
  );
  assert.ok(CSV_SYNC_MAX_BYTES >= 1024 * 1024);
});

test("parseCsvSyncOptions: defaults are the safe ones and opt-ins are explicit (DATA-01)", () => {
  // `apply_delta` = the API's snapshot-aware mode: added stock applies, a
  // concurrent sale is never rewound.
  assert.equal(DEFAULT_CSV_STOCK_CONFLICT_MODE, "apply_delta");
  assert.deepEqual([...CSV_STOCK_CONFLICT_MODES], [
    "keep_db",
    "apply_delta",
    "force_override",
  ]);
  assert.equal(parseCsvSyncOptions(null).applyStock, false);
  assert.equal(parseCsvSyncOptions(undefined).allowSlugRename, false);
  assert.equal(parseCsvSyncOptions(null).stockConflictMode, "apply_delta");

  const untouched = new FormData();
  assert.deepEqual(parseCsvSyncOptions(untouched), {
    applyStock: false,
    stockConflictMode: "apply_delta",
    allowSlugRename: false,
  });

  // HTML checkboxes post "on"; JSON clients post booleans.
  const form = new FormData();
  form.append("apply_stock", "on");
  form.append("allow_slug_rename", "true");
  form.append("stock_conflict_mode", "force_override");
  assert.deepEqual(parseCsvSyncOptions(form), {
    applyStock: true,
    stockConflictMode: "force_override",
    allowSlugRename: true,
  });

  const jsonFlags = { apply_stock: true, allow_slug_rename: "1" };
  assert.deepEqual(
    parseCsvSyncOptions({ get: (name) => jsonFlags[name] }),
    {
      applyStock: true,
      stockConflictMode: "apply_delta",
      allowSlugRename: true,
    },
  );

  // Unchecked boxes and unknown modes never enable anything.
  const offFlags = {
    apply_stock: "off",
    allow_slug_rename: "no",
    stock_conflict_mode: "yolo",
  };
  assert.deepEqual(
    parseCsvSyncOptions({ get: (name) => offFlags[name] }),
    {
      applyStock: false,
      stockConflictMode: "apply_delta",
      allowSlugRename: false,
    },
  );
  for (const mode of CSV_STOCK_CONFLICT_MODES) {
    assert.equal(
      parseCsvSyncOptions({ get: () => mode }).stockConflictMode,
      mode,
    );
  }
});

test("buildCsvSyncPayload: strips only the leading BOM and forwards every option (DATA-01)", () => {
  const payload = buildCsvSyncPayload("\uFEFFslug,price_cents\nph-5,1615\n", {
    applyStock: false,
    stockConflictMode: "keep_db",
    allowSlugRename: false,
  });
  assert.equal(payload.csv, "slug,price_cents\nph-5,1615\n");
  assert.equal(payload.apply_stock, false);
  assert.equal(payload.stock_conflict_mode, "keep_db");
  assert.equal(payload.allow_slug_rename, false);
  // A BOM in the middle is data, not a header marker.
  assert.equal(stripUtf8Bom("a\uFEFFb"), "a\uFEFFb");
});

test("normalizeCsvSyncReport: renders the canonical diff and honours blocking errors (DATA-01)", () => {
  const report = normalizeCsvSyncReport({
    ok: true,
    dry_run: true,
    changes: [
      {
        row: "12",
        slug: "ph-5",
        field: "price_cents",
        before: "161500",
        after: "170000",
        action: "update",
      },
      {
        row: "13",
        slug: "ph-6",
        field: "model",
        before: "",
        after: "PH 6",
        action: "create",
      },
    ],
    errors: [],
    summary: { created: 1, updated: 1, unchanged: 0, errors: 0 },
  });
  assert.equal(report.ok, true);
  assert.equal(report.dryRun, true);
  assert.equal(report.changes.length, 2);
  assert.deepEqual(report.changes[0], {
    row: "12",
    slug: "ph-5",
    field: "price_cents",
    before: "161500",
    after: "170000",
    action: "update",
  });
  assert.deepEqual(report.summary, {
    created: 1,
    updated: 1,
    unchanged: 0,
    errors: 0,
  });

  // Any reported error makes the report NOT committable, even if the API said ok.
  const blocked = normalizeCsvSyncReport({
    ok: true,
    errors: [
      { row: 4, column: "price_cents", message: "price_cents must be an integer" },
      "unknown column 'foo'",
    ],
    changes: [
      { row: "4", slug: "ph-5", field: "price_cents", before: "1", after: "x" },
    ],
  });
  assert.equal(blocked.ok, false);
  assert.equal(blocked.errors.length, 2);
  assert.equal(blocked.errors[0].message, "price_cents must be an integer");
  assert.equal(blocked.errors[0].row, "4");
  assert.equal(blocked.errors[0].column, "price_cents");
  assert.equal(blocked.errors[1].message, "unknown column 'foo'");
  assert.equal(blocked.errors[1].column, "");
  // Action + summary are derived when the API omits them.
  assert.equal(blocked.changes[0].action, "update");
  assert.equal(blocked.summary.updated, 1);
  assert.equal(blocked.summary.errors, 2);

  // Aliased field names still render (rows/diffs, column, from/to); with no
  // line number the identifier doubles as the row label.
  const aliased = normalizeCsvSyncReport({
    ok: true,
    rows: [{ slug: "ph-5", column: "model", from: "PH5", to: "PH 5" }],
  });
  assert.deepEqual(aliased.changes[0], {
    row: "",
    slug: "ph-5",
    field: "model",
    before: "PH5",
    after: "PH 5",
    action: "update",
  });

  // Garbage in never throws and never claims success.
  const empty = normalizeCsvSyncReport(null);
  assert.equal(empty.ok, false);
  assert.deepEqual(empty.changes, []);
  assert.deepEqual(empty.errors, []);
  assert.equal(normalizeCsvSyncReport({ ok: false }).ok, false);
});

test("CSV sync client: 1-click export, dry-run diff table, apply_stock default OFF and commit gate (DATA-01)", () => {
  const client = readSource("components/admin-csv-sync-client.tsx");
  assert.match(client, /href=\{`\/admin\/api\/csv\/\$\{mode\}`\}/);
  assert.match(client, /download/);
  assert.match(client, /useState\(false\)/);
  assert.match(client, /CSV_STOCK_CONFLICT_MODES\.map/);
  assert.match(client, /allow_slug_rename|allowSlugRename/);
  assert.match(client, /Before \(database\)/);
  assert.match(client, /After \(spreadsheet\)/);
  // Nothing can be committed that was not previewed exactly as uploaded.
  assert.match(
    client,
    /const canCommit = canPreview && report !== null && report\.ok/,
  );
  assert.match(client, /function invalidatePreview\(\)/);
});

test("CSV proxy route: session cookie, same-origin guard, XRW header and commit-only revalidation (DATA-01/G17)", () => {
  const route = readSource("app/admin/api/csv/[...slug]/route.ts");
  assert.match(route, /verifySameOriginRequest\(request\)/);
  assert.match(route, /verifyAdminSession\(\)/);
  assert.match(route, /getAdminCookie\(\)/);
  assert.match(route, /ADMIN_COOKIE_NAME/);
  assert.match(route, /\[XRW_HEADER\]: XRW_VALUE/);
  assert.match(route, /csvApiPath\(route\.kind, route\.phase\)/);
  // Only a COMMIT may revalidate the storefront.
  assert.match(route, /if \(ok && route\.phase === "commit"\)/);
  assert.match(route, /revalidatePath\("\/", "layout"\)/);
  // Byte-for-byte pass-through keeps the UTF-8 BOM Excel needs.
  assert.match(route, /new Response\(upstream\.body/);
  assert.match(route, /text\/csv; charset=utf-8/);
});

test("admin pages mount the CSV sync client for both products and site copy (DATA-01)", () => {
  assert.match(
    readSource("app/admin/(authed)/products/page.tsx"),
    /<AdminCsvSyncClient mode="products" \/>/,
  );
  assert.match(
    readSource("app/admin/(authed)/site-content/page.tsx"),
    /<AdminCsvSyncClient mode="site-content" \/>/,
  );
});
