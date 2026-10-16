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
import { buildOrderEmailHtml, escapeHtml } from "../lib/email-receipt.ts";

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
