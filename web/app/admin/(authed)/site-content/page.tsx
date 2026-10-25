import type { Metadata } from "next";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";
import AdminCsvSyncClient from "@/components/admin-csv-sync-client";

export const metadata: Metadata = { title: "Site Copy Studio" };
export const dynamic = "force-dynamic";

const HUMAN_LABELS: Record<string, { section: string; title: string }> = {
  "hero.eyebrow": { section: "Hero Stage", title: "Top Exhibition Eyebrow" },
  "hero.lead": { section: "Hero Stage", title: "Primary Exhibition Headline ({pieces} supported)" },
  "hero.leadPaired": { section: "Hero Stage", title: "Secondary Brand Positioning Line ({brand} supported)" },
  "drop.framing": { section: "Collection Framing", title: "Drop Introduction Paragraph ({pieces} supported)" },
  "drop.piecesHeading": { section: "Collection Framing", title: "Collection Grid Section Heading ({pieces} supported)" },
  "atmosphere.dayLabel": { section: "Atmosphere Dimmer", title: "Daylight 5500K Mode Label" },
  "atmosphere.nightLabel": { section: "Atmosphere Dimmer", title: "Nocturne 2700K Mode Label" },
  "atmosphere.caption": { section: "Atmosphere Dimmer", title: "Hero Lighting Physics Caption" },
  "trust.disclosure": { section: "Provenance & Trust", title: "Parallel-Import Independent Seller Disclosure ({brand} supported)" },
  "warranty.pending": { section: "Provenance & Trust", title: "House Warranty Statement ({brand} supported)" },
  "reference.pending": { section: "Provenance & Trust", title: "Unquoted Showroom Reference Price Fallback" },
  "enquiry.intro": { section: "Concierge", title: "WhatsApp Concierge Invitation" },
  "enquiry.messageTemplate": { section: "Concierge", title: "Prefilled WhatsApp Message Template ({brand}, {subject})" },
  "enquiry.dropSubject": { section: "Concierge", title: "Homepage Concierge Subject Name" },
  "footer.location": { section: "Footer & SEO", title: "Footer City / Location Label" },
  "meta.home.title": { section: "Footer & SEO", title: "Homepage Browser Title ({brand} supported)" },
  "meta.home.description": { section: "Footer & SEO", title: "Homepage Meta Description ({brand}, {pieces})" },
  "meta.product.descriptionTemplate": { section: "Footer & SEO", title: "Product Page Meta Description ({brand}, {subject})" },
};

export default async function AdminSiteContentPage({
  searchParams,
}: {
  searchParams: Promise<{ saved?: string; error?: string }>;
}) {
  await requireAdmin();
  const { saved, error } = await searchParams;

  const { data } = await adminFetch("/api/site-content");
  const content =
    (data as { content?: Record<string, Record<string, string>> } | null)
      ?.content ?? {};
  const keys = Object.keys(content);

  return (
    <div className="space-y-8">
      <div className="border-b border-[var(--border-hairline)] pb-5">
        <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
          SIDE-BY-SIDE BILINGUAL COPY STUDIO
        </p>
        <h1 className="mt-1 font-serif text-3xl">Site Copy ({keys.length} Keys)</h1>
        <p className="mt-2 text-xs text-[var(--text-secondary)]">
          Edit English and 繁體中文 side-by-side and save both in one click.
          Variables <code>{"{brand}"}</code>, <code>{"{pieces}"}</code>, and{" "}
          <code>{"{subject}"}</code> are interpolated automatically.
        </p>
      </div>

      {saved && (
        <div
          role="status"
          className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-card)] p-4 text-sm"
        >
          Saved: {saved}
        </div>
      )}

      {error && (
        <div
          role="alert"
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm text-[var(--color-error)]"
        >
          {error}
        </div>
      )}

      {/* 1-Click Excel/CSV Bulk Sync for bilingual site copy (DATA-01) */}
      <AdminCsvSyncClient mode="site-content" />

      <div className="space-y-5">
        {keys.map((key) => {
          const meta = HUMAN_LABELS[key];
          const enVal = content[key]?.en ?? "";
          const zhVal = content[key]?.["zh-HK"] ?? "";
          return (
            <form
              key={key}
              method="post"
              action="/admin/api/site-content"
              className="card-exhibition space-y-4 p-5"
            >
              <input type="hidden" name="key" value={key} />
              <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-[var(--border-hairline)] pb-3">
                <div>
                  {meta && (
                    <span className="mr-2 text-[10px] tracking-[0.18em] uppercase text-[var(--color-glow)]">
                      {meta.section} ·
                    </span>
                  )}
                  <span className="font-serif text-lg">
                    {meta?.title ?? key}
                  </span>
                </div>
                <code className="font-mono text-xs text-[var(--text-secondary)]">
                  {key}
                </code>
              </div>

              <div className="grid gap-4 sm:grid-cols-2">
                <div>
                  <label className="block text-xs text-[var(--text-secondary)]">
                    English (en)
                  </label>
                  <textarea
                    name="value_en"
                    rows={3}
                    defaultValue={enVal}
                    className="mt-1 w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-base sm:text-sm"
                  />
                </div>
                <div>
                  <label className="block text-xs text-[var(--text-secondary)]">
                    繁體中文 (zh-HK)
                  </label>
                  <textarea
                    name="value_zh"
                    rows={3}
                    defaultValue={zhVal}
                    className="mt-1 w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-base sm:text-sm"
                  />
                </div>
              </div>

              <div className="flex justify-end">
                <button
                  type="submit"
                  name="action"
                  value="save_pair"
                  className="bg-[var(--text-primary)] px-5 py-2 text-xs font-medium tracking-[0.16em] uppercase text-[var(--surface-page)]"
                >
                  Save EN + 繁體中文
                </button>
              </div>
            </form>
          );
        })}
      </div>
    </div>
  );
}
