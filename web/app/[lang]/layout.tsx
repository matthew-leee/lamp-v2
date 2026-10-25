import type { Metadata } from "next";
import type { ReactNode } from "react";
import Link from "next/link";
import { notFound } from "next/navigation";
import { loadStorefrontData, readSiteCopy } from "@/lib/api";
import { htmlLang, isLocale, t, ui, type Locale } from "@/lib/i18n";
import { StorefrontProvider } from "@/components/storefront-context";
import ExhibitionHeader from "@/components/exhibition-header";
import BagDrawer from "@/components/bag-drawer";
import DevAgentation from "@/components/dev-agentation";
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
  const displayBrand =
    config.brandName.trim() !== ""
      ? config.brandName
      : t(ui.defaultExhibitionTitle, locale);
  const vars = { brand: displayBrand, pieces: catalogue.total_pieces };

  const title = readSiteCopy(
    siteContent,
    "meta.home.title",
    locale,
    vars,
    locale === "zh-HK"
      ? `${displayBrand} | 典藏建築燈飾展`
      : `${displayBrand} | Curated Architectural Lighting`,
  );
  const description = readSiteCopy(
    siteContent,
    "meta.home.description",
    locale,
    vars,
    locale === "zh-HK"
      ? `${displayBrand} 香港限量典藏設計展。`
      : `${displayBrand} curated design exhibition, Hong Kong.`,
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

  const displayBrand =
    config.brandName.trim() !== ""
      ? config.brandName
      : t(ui.defaultExhibitionTitle, locale);
  const vars = { brand: displayBrand, pieces: catalogue.total_pieces };
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
      // Root-element ownership contract (React 19 hydration):
      //   - `lang` and `data-scroll-behavior` are rendered by React and never
      //     mutated by script, so React's hydration diff always matches.
      //   - `data-scroll-behavior="smooth"` is Next 15.5's required opt-in for
      //     the `scroll-behavior: smooth` declared on `html` in globals.css:
      //     without it Next logs a console warning on every route transition
      //     because it no longer force-disables smooth scrolling for us. It is
      //     static, so React's hydration diff can never disagree with the DOM.
      //   - `data-atmosphere` and the `--header-height` custom property are
      //     owned by client code (StorefrontProvider restores the visitor's
      //     persisted Daylight/Nocturne choice; ExhibitionHeader measures its
      //     own inner wrapper), so the server-rendered `data-atmosphere` is
      //     corrected by an effect after hydration. `suppressHydrationWarning`
      //     is required for exactly those two script-owned values — React 19
      //     must not try to "patch up" the visitor's lighting choice.
      data-atmosphere="night"
      data-scroll-behavior="smooth"
      suppressHydrationWarning
    >
      <body className="min-h-dvh font-sans antialiased" suppressHydrationWarning>
        {/* GA4 analytics (G23): only when a validated Measurement ID is
            configured. React 19 hoists the async loader into <head>. */}
        {config.gaMeasurementId && (
          <>
            <script
              async
              src={`https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(config.gaMeasurementId)}`}
            />
            <script
              dangerouslySetInnerHTML={{
                __html: `window.dataLayer = window.dataLayer || []; function gtag(){window.dataLayer.push(arguments);} gtag("js", new Date()); gtag("config", ${JSON.stringify(config.gaMeasurementId)});`,
              }}
            />
          </>
        )}
        <StorefrontProvider
          catalogueProducts={catalogue.products}
          currency={config.currency}
          brandName={config.brandName}
        >
          <ExhibitionHeader
            lang={locale}
            brandName={config.brandName}
            brandNameNative={config.brandNameNative}
            brandLogoUrl={config.brandLogoUrl}
          />
          <BagDrawer lang={locale} totalDropPieces={catalogue.total_pieces} />
          <main>{children}</main>
          <footer className="no-print border-t border-[var(--border-hairline)] bg-[var(--surface-card)]">
            <div className="mx-auto flex max-w-7xl flex-col justify-between gap-6 px-5 py-12 sm:px-10 lg:flex-row lg:items-end">
              <div className="max-w-3xl">
                <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--text-secondary)]">
                  {displayBrand} · {location}
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
          <DevAgentation />
        </StorefrontProvider>
      </body>
    </html>
  );
}
