import type { MetadataRoute } from "next";
import { loadStorefrontData } from "@/lib/api";

export const dynamic = "force-dynamic";

export default async function robots(): Promise<MetadataRoute.Robots> {
  const { config } = await loadStorefrontData();
  return {
    rules: {
      userAgent: "*",
      allow: "/",
      disallow: ["/api/", "/admin/"],
    },
    sitemap: `${config.siteUrl}/sitemap.xml`,
  };
}
