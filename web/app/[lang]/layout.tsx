import type { Metadata } from "next";
import type { ReactNode } from "react";
import Link from "next/link";
import { notFound } from "next/navigation";
import { loadStorefrontData, readSiteCopy } from "@/lib/api";
import { htmlLang, isLocale, type Locale } from "@/lib/i18n";
import { StorefrontProvider } from "@/components/storefront-context";
import ExhibitionHeader from "@/components/exhibition-header";
import BagDrawer from "@/components/bag-drawer";
import "../globals.css";

export const dynamic = "force-dynamic";

export async function generateMetadata({
  params,
}: {
  params: Promise<{ lang: string }>;
}): Promise<Metadata> {
  const { lang } = await params;
  const locale: Locale = isLocale(lang) ? lang : "en";
  const { catalogue, siteContent, config } = await loadStorefrontData();
  const vars = { brand: config.brandName, pieces: catalogue.total_pieces };

  const title = readSiteCopy(
    siteContent,
    "meta.home.title",
    locale,
    vars,
    `${config.brandName} | Curated Architectural Lighting`,
  );
  const description = readSiteCopy(
    siteContent,
    "meta.home.description",
    locale,
    vars,
    `${config.brandName} curated design exhibition, Hong Kong.`,
  );

  return {
    title,
    description,
    alternates: {
      canonical: `${config.siteUrl}/${locale}`,
      languages: {
        en: `${config.siteUrl}/en`,
        "zh-Hant-HK": `${config.siteUrl}/zh-HK`,
        "x-default": `${config.siteUrl}/en`,
      },
    },
    verification: config.gscVerification
      ? { google: config.gscVerification }
      : undefined,
  };
}

export default async function LocaleLayout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  if (!isLocale(lang)) {
    notFound();
  }
  const locale: Locale = lang;
  const { catalogue, siteContent, config } = await loadStorefrontData();

  const vars = { brand: config.brandName, pieces: catalogue.total_pieces };
  const disclosure = readSiteCopy(siteContent, "trust.disclosure", locale, vars);
  const location = readSiteCopy(
    siteContent,
    "footer.location",
    locale,
    vars,
    locale === "zh-HK" ? "香港" : "Hong Kong",
  );

  return (
    <html
      lang={htmlLang[locale]}
      data-atmosphere="night"
      suppressHydrationWarning
    >
      <body className="min-h-dvh font-sans antialiased">
        <StorefrontProvider
          catalogueProducts={catalogue.products}
          currency={config.currency}
          brandName={config.brandName}
        >
          <ExhibitionHeader
            lang={locale}
            brandName={config.brandName}
            brandNameNative={config.brandNameNative}
          />
          <BagDrawer lang={locale} totalDropPieces={catalogue.total_pieces} />
          <main>{children}</main>
          <footer className="no-print border-t border-[var(--border-hairline)] bg-[var(--surface-card)]">
            <div className="mx-auto flex max-w-7xl flex-col justify-between gap-6 px-5 py-12 sm:px-10 lg:flex-row lg:items-end">
              <div className="max-w-3xl">
                <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
                  {config.brandName} · {location}
                </p>
                <p
                  lang={htmlLang[locale]}
                  className="mt-3 text-xs leading-relaxed text-[var(--text-secondary)]"
                >
                  {disclosure}
                </p>
              </div>
              <div className="flex flex-wrap gap-6 text-xs tracking-[0.16em] uppercase text-[var(--text-secondary)]">
                <Link href={`/${locale}/orders`} className="hover:text-[var(--text-primary)]">
                  {locale === "zh-HK" ? "訂單查詢" : "Order Lookup"}
                </Link>
                {locale === "zh-HK" ? (
                  <Link href="/admin" className="hover:text-[var(--text-primary)]">
                    後台管理
                  </Link>
                ) : (
                  <Link href="/admin" className="hover:text-[var(--text-primary)]">
                    Studio Admin
                  </Link>
                )}
              </div>
            </div>
          </footer>
        </StorefrontProvider>
      </body>
    </html>
  );
}
