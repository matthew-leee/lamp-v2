import { notFound } from "next/navigation";
import { loadStorefrontData, readSiteCopy } from "@/lib/api";
import { isLocale, t, ui, type Locale } from "@/lib/i18n";
import ExhibitionHomeClient from "@/components/exhibition-home-client";

export const dynamic = "force-dynamic";

export default async function ExhibitionHomePage({
  params,
}: {
  params: Promise<{ lang: string }>;
}) {
  const { lang } = await params;
  if (!isLocale(lang)) notFound();
  const locale: Locale = lang;

  const { catalogue, siteContent, settings, config } = await loadStorefrontData();
  const totalPieces = catalogue.total_pieces;
  const displayBrand =
    config.brandName.trim() !== ""
      ? config.brandName
      : t(ui.defaultExhibitionTitle, locale);
  const vars = { brand: displayBrand, pieces: totalPieces };

  // NAME-01 / DROP-01: the hero brand emblem uses the operator-supplied logo
  // when configured (`brand.logo_url` public setting, or the typed config
  // once the site config exposes it).
  const brandLogoUrl =
    (config as { brandLogoUrl?: string | null }).brandLogoUrl?.trim() ||
    settings["brand.logo_url"]?.trim() ||
    null;

  const dropSubject = readSiteCopy(
    siteContent,
    "enquiry.dropSubject",
    locale,
    vars,
    "Edition 01",
  );

  const copy = {
    heroEyebrow: readSiteCopy(siteContent, "hero.eyebrow", locale, vars),
    heroLead: readSiteCopy(siteContent, "hero.lead", locale, vars),
    dropFraming: readSiteCopy(siteContent, "drop.framing", locale, vars),
    piecesHeading: readSiteCopy(siteContent, "drop.piecesHeading", locale, vars),
    atmosphereCaption: readSiteCopy(
      siteContent,
      "atmosphere.caption",
      locale,
      vars,
    ),
    disclosure: readSiteCopy(siteContent, "trust.disclosure", locale, vars),
    warrantyPending: readSiteCopy(siteContent, "warranty.pending", locale, vars),
    enquiryIntro: readSiteCopy(siteContent, "enquiry.intro", locale, vars),
    enquiryMessage: readSiteCopy(
      siteContent,
      "enquiry.messageTemplate",
      locale,
      { ...vars, subject: dropSubject },
    ),
  };

  return (
    <ExhibitionHomeClient
      lang={locale}
      products={catalogue.products}
      totalPieces={totalPieces}
      currency={config.currency}
      brandName={config.brandName}
      brandNameNative={config.brandNameNative}
      brandLogoUrl={brandLogoUrl}
      whatsappNumber={config.whatsappNumber}
      copy={copy}
    />
  );
}
