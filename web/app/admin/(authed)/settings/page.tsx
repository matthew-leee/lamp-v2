import type { Metadata } from "next";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";

export const metadata: Metadata = { title: "Company Settings" };
export const dynamic = "force-dynamic";

interface SettingDef {
  key: string;
  label: string;
  hint: string;
  envVar: string;
  required?: boolean;
  noOff?: boolean;
}

const SETTING_DEFS: readonly SettingDef[] = [
  {
    key: "brand.name",
    label: "Brand Name (Latin Wordmark)",
    hint: "Primary brand name displayed in header, titles, and legal copy. Required.",
    envVar: "NEXT_PUBLIC_BRAND_NAME",
    required: true,
    noOff: true,
  },
  {
    key: "brand.name_native",
    label: "Brand Name (Native Chinese Wordmark)",
    hint: "Optional Traditional Chinese brand name shown on /zh-HK routes.",
    envVar: "NEXT_PUBLIC_BRAND_NAME_NATIVE",
  },
  {
    key: "site.url",
    label: "Canonical Site URL",
    hint: "Absolute public URL without trailing slash (used for SEO canonicals & email receipts). Required.",
    envVar: "NEXT_PUBLIC_SITE_URL",
    required: true,
    noOff: true,
  },
  {
    key: "currency",
    label: "Storefront Currency (ISO 4217)",
    hint: "3-letter uppercase currency code (e.g. HKD, USD, EUR).",
    envVar: "NEXT_PUBLIC_CURRENCY",
    noOff: true,
  },
  {
    key: "contact.whatsapp",
    label: "WhatsApp Concierge Number",
    hint: "International digits (e.g. 85291234567). Toggle OFF to hide WhatsApp buttons.",
    envVar: "NEXT_PUBLIC_WHATSAPP_NUMBER",
  },
  {
    key: "analytics.ga_measurement_id",
    label: "Google Analytics 4 ID",
    hint: "GA4 Measurement ID (e.g. G-XXXXXXXXXX). Toggle OFF to disable analytics.",
    envVar: "NEXT_PUBLIC_GA_MEASUREMENT_ID",
  },
  {
    key: "analytics.gsc_verification",
    label: "Google Search Console Verification Token",
    hint: "Verification meta token. Toggle OFF to omit.",
    envVar: "NEXT_PUBLIC_GSC_VERIFICATION",
  },
];

export default async function AdminSettingsPage({
  searchParams,
}: {
  searchParams: Promise<{ saved?: string; error?: string }>;
}) {
  await requireAdmin();
  const { saved, error } = await searchParams;

  const { data } = await adminFetch("/api/settings");
  const overrides =
    typeof data === "object" && data !== null && !Array.isArray(data)
      ? (data as Record<string, string>)
      : {};

  return (
    <div className="max-w-4xl space-y-6">
      <div className="border-b border-[var(--border-hairline)] pb-5">
        <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
          LIVE WHITE-LABEL CONFIGURATION (0-SECOND STOREFRONT SYNC)
        </p>
        <h1 className="mt-1 font-serif text-3xl">Company Settings</h1>
        <p className="mt-2 text-xs text-[var(--text-secondary)]">
          Every save immediately revalidates the storefront layout and pages
          with zero wait time.
        </p>
      </div>

      {saved && (
        <div
          role="status"
          className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-card)] p-4 text-sm"
        >
          {saved}
        </div>
      )}

      {error && (
        <div
          role="alert"
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm text-[var(--color-error)] font-medium"
        >
          {error}
        </div>
      )}

      <div className="space-y-4">
        {SETTING_DEFS.map((def) => {
          const hasOverride = def.key in overrides;
          const overrideVal = overrides[def.key] ?? "";
          const envVal = process.env[def.envVar]?.trim() ?? "";
          const isExplicitOff = hasOverride && overrideVal === "" && !def.noOff;
          const currentEffective = hasOverride
            ? isExplicitOff
              ? "OFF (Explicitly Disabled)"
              : overrideVal
            : envVal || (def.key === "currency" ? "HKD" : "(unset)");

          return (
            <form
              key={def.key}
              method="post"
              action={`/admin/api/settings/${def.key}`}
              className="card-exhibition space-y-3 p-5"
            >
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <label
                  htmlFor={`setting-${def.key}`}
                  className="font-serif text-lg"
                >
                  {def.label}{" "}
                  <code className="ml-1 font-mono text-xs text-[var(--text-secondary)]">
                    ({def.key})
                  </code>
                </label>
                <span className="font-mono text-xs text-[var(--color-glow)]">
                  Effective: {currentEffective}
                </span>
              </div>

              <p className="text-xs text-[var(--text-secondary)]">{def.hint}</p>

              <input
                id={`setting-${def.key}`}
                name="value"
                type="text"
                defaultValue={
                  hasOverride
                    ? overrideVal
                    : envVal || (def.key === "currency" ? "HKD" : "")
                }
                className="w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2 text-sm"
              />

              <div className="flex flex-wrap items-center gap-3 pt-1">
                <button
                  type="submit"
                  name="action"
                  value="save"
                  className="bg-[var(--text-primary)] px-5 py-2 text-xs font-medium tracking-[0.16em] uppercase text-[var(--surface-page)]"
                >
                  Save Live Override
                </button>
                {!def.noOff && (
                  <button
                    type="submit"
                    name="action"
                    value="off"
                    className="border border-[var(--border-hairline)] px-4 py-2 text-xs uppercase tracking-[0.14em] hover:border-[var(--color-error)]"
                  >
                    Turn OFF
                  </button>
                )}
                {hasOverride && (
                  <button
                    type="submit"
                    name="action"
                    value="revert"
                    className="border border-[var(--border-hairline)] px-4 py-2 text-xs uppercase tracking-[0.14em] text-[var(--text-secondary)]"
                  >
                    Revert to Env Default ({envVal || "unset"})
                  </button>
                )}
              </div>
            </form>
          );
        })}
      </div>
    </div>
  );
}
