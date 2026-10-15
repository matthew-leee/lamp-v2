import type { MetadataRoute } from "next";
import { loadStorefrontData } from "@/lib/api";
import { locales } from "@/lib/i18n";

export const dynamic = "force-dynamic";

export default async function sitemap(): Promise<MetadataRoute.Sitemap> {
  const { catalogue, config } = await loadStorefrontData();
  const entries: MetadataRoute.Sitemap = [];
  for (const locale of locales) {
    entries.push({ url: `${config.siteUrl}/${locale}` });
    for (const product of catalogue.products) {
      entries.push({
        url: `${config.siteUrl}/${locale}/products/${product.slug}`,
      });
    }
  }
  return entries;
}
