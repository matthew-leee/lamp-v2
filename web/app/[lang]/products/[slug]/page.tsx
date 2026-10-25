import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { loadStorefrontData, readSiteCopy } from "@/lib/api";
import { formatProductTitle } from "@/lib/format";
import { isLocale, t, ui, type Locale } from "@/lib/i18n";
import ProductStoryClient from "@/components/product-story-client";

export const dynamic = "force-dynamic";
export const dynamicParams = true;

export async function generateMetadata({
  params,
}: {
  params: Promise<{ lang: string; slug: string }>;
}): Promise<Metadata> {
  const { lang, slug } = await params;
  const locale: Locale = isLocale(lang) ? lang : "en";
  const { catalogue, siteContent, config } = await loadStorefrontData();
  const displayBrand =
    config.brandName.trim() !== ""
      ? config.brandName
      : t(ui.defaultExhibitionTitle, locale);
  const product = catalogue.products.find((p) => p.slug === slug);
  if (!product) {
    // LANG-01: the not-found title speaks the route's language too.
    return {
      title:
        locale === "zh-HK"
          ? `找不到作品 | ${displayBrand}`
          : `Not Found | ${displayBrand}`,
    };
  }

  // LANG-01: strictly locale-specific copy — `zh-HK` never falls back to
  // English product copy.
  const localeCopy = product.content[locale] ?? {};
  const displayTitle = formatProductTitle(
    product.model,
    localeCopy.model_alt,
    localeCopy.colourway,
    locale,
  );
  const vars = {
    brand: displayBrand,
    pieces: catalogue.total_pieces,
    subject: displayTitle,
  };
  const description = readSiteCopy(
    siteContent,
    "meta.product.descriptionTemplate",
    locale,
    vars,
    `${displayTitle} · ${displayBrand}`,
  );

  return {
    title: `${displayTitle} | ${displayBrand}`,
    description,
    openGraph: {
      title: `${displayTitle} | ${displayBrand}`,
      description,
      images: product.image_lit_url
        ? [product.image_lit_url]
        : product.image_day_url
          ? [product.image_day_url]
          : undefined,
    },
    alternates: {
      canonical: `${config.siteUrl}/${locale}/products/${product.slug}`,
      languages: {
        en: `${config.siteUrl}/en/products/${product.slug}`,
        "zh-Hant-HK": `${config.siteUrl}/zh-HK/products/${product.slug}`,
        "x-default": `${config.siteUrl}/en/products/${product.slug}`,
      },
    },
  };
}

export default async function ProductStoryPage({
  params,
}: {
  params: Promise<{ lang: string; slug: string }>;
}) {
  const { lang, slug } = await params;
  if (!isLocale(lang)) notFound();
  const locale: Locale = lang;

  const { catalogue, siteContent, config } = await loadStorefrontData();
  const product = catalogue.products.find((p) => p.slug === slug);
  if (!product) {
    notFound();
  }

  // Group siblings by family (if non-empty) or by model
  const siblings = catalogue.products.filter((p) =>
    product.family.trim() !== ""
      ? p.family.trim() === product.family.trim()
      : p.model === product.model,
  );

  // LANG-01: strictly locale-specific copy — `zh-HK` never falls back to
  // English product copy.
  const localeCopy = product.content[locale] ?? {};
  const displayTitle = formatProductTitle(
    product.model,
    localeCopy.model_alt,
    localeCopy.colourway,
    locale,
  );

  const displayBrand =
    config.brandName.trim() !== ""
      ? config.brandName
      : t(ui.defaultExhibitionTitle, locale);

  const vars = {
    brand: displayBrand,
    pieces: catalogue.total_pieces,
    subject: displayTitle,
  };

  const copy = {
    disclosure: readSiteCopy(siteContent, "trust.disclosure", locale, vars),
    warrantyPending: readSiteCopy(siteContent, "warranty.pending", locale, vars),
    enquiryMessage: readSiteCopy(
      siteContent,
      "enquiry.messageTemplate",
      locale,
      vars,
      // LANG-01: the fallback enquiry line speaks the route's language.
      locale === "zh-HK"
        ? `你好 ${displayBrand}，我想查詢 ${displayTitle}。`
        : `Hello ${displayBrand}, I would like to enquire about ${displayTitle}.`,
    ),
  };

  // Safe Product JSON-LD
  const whole = Math.floor(product.price_cents / 100);
  const minor = product.price_cents % 100;
  const jsonLd = {
    "@context": "https://schema.org",
    "@type": "Product",
    name: displayTitle,
    sku: product.slug,
    brand: {
      "@type": "Brand",
      name: product.brand,
    },
    offers: {
      "@type": "Offer",
      url: `${config.siteUrl}/${locale}/products/${product.slug}`,
      priceCurrency: config.currency,
      price: `${whole}.${String(minor).padStart(2, "0")}`,
    },
  };

  const safeJsonLd = JSON.stringify(jsonLd)
    .replace(/</g, "\\u003c")
    .replace(/>/g, "\\u003e")
    .replace(/&/g, "\\u0026");

  return (
    <>
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: safeJsonLd }}
      />
      <ProductStoryClient
        lang={locale}
        product={product}
        siblings={siblings}
        totalPieces={catalogue.total_pieces}
        currency={config.currency}
        whatsappNumber={config.whatsappNumber}
        copy={copy}
      />
    </>
  );
}
