"use client";

import {
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import LampStage from "./lamp-stage";
import type { ArtVariant } from "@/lib/api";
import {
  formatCentsToDollarsInput,
  formatMoney,
  parseDollarsToCents,
} from "@/lib/format";

export interface AdminProductFullData {
  id: number;
  slug: string;
  brand: string;
  model: string;
  price_cents: number;
  family: string;
  art_variant: ArtVariant;
  art_hue: number;
  active: boolean;
  stock_total: number;
  stock_reserved: number;
  stock_quarantined: number;
  image_day_url: string;
  image_lit_url: string;
  designer: string;
  design_year: string;
  dimensions: string;
  light_source: string;
  created_at: string;
  content: Record<string, Record<string, string>>;
  reference_price: {
    product_id: number;
    min_price_cents: number;
    max_price_cents: number;
    source_name: string;
    source_url: string;
    checked_on: string;
  } | null;
  pieces: number[];
}

export interface AdminProductSummary {
  id: number;
  slug: string;
  brand: string;
  model: string;
}

/** Everything the studio form can edit — one sessionStorage draft snapshot. */
interface StudioDraft {
  slug: string;
  brand: string;
  model: string;
  family: string;
  active: boolean;
  priceDollars: string;
  stockTotalInput: string;
  stockQuarantinedInput: string;
  imageDayUrl: string;
  imageLitUrl: string;
  artVariant: ArtVariant;
  artHue: number;
  designer: string;
  designYear: string;
  dimensions: string;
  lightSource: string;
  enModelAlt: string;
  zhModelAlt: string;
  enColourway: string;
  zhColourway: string;
  enHeritage: string;
  zhHeritage: string;
  enElectrical: string;
  zhElectrical: string;
  refClear: boolean;
  refMinDollars: string;
  refMaxDollars: string;
  refSourceName: string;
  refSourceUrl: string;
  refCheckedOn: string;
}

const ART_VARIANTS: readonly ArtVariant[] = [
  "dome",
  "shades",
  "shades-tall",
  "piano",
  "none",
] as const;

export default function AdminProductStudioClient({
  initialProduct,
  allProducts,
  currency,
  initialBanner,
  initialError,
}: {
  initialProduct: AdminProductFullData;
  allProducts: AdminProductSummary[];
  currency: string;
  initialBanner: string | null;
  initialError: string | null;
}) {
  const router = useRouter();
  const [row, setRow] = useState<AdminProductFullData>(initialProduct);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [cloning, setCloning] = useState(false);
  const [uploadingField, setUploadingField] = useState<"day" | "lit" | null>(null);
  const [banner, setBanner] = useState<string | null>(initialBanner);
  const [errorMsg, setErrorMsg] = useState<string | null>(initialError);

  // Editable form state (human dollars for prices!)
  const [slug, setSlug] = useState(row.slug);
  const [brand, setBrand] = useState(row.brand);
  const [model, setModel] = useState(row.model);
  const [family, setFamily] = useState(row.family);
  const [active, setActive] = useState(row.active);
  const [priceDollars, setPriceDollars] = useState(
    formatCentsToDollarsInput(row.price_cents),
  );
  const [stockTotalInput, setStockTotalInput] = useState("");
  const [stockQuarantinedInput, setStockQuarantinedInput] = useState("");

  // Media & Stage
  const [imageDayUrl, setImageDayUrl] = useState(row.image_day_url);
  const [imageLitUrl, setImageLitUrl] = useState(row.image_lit_url);
  const [artVariant, setArtVariant] = useState<ArtVariant>(row.art_variant);
  const [artHue, setArtHue] = useState<number>(row.art_hue);

  // Architectural specs
  const [designer, setDesigner] = useState(row.designer);
  const [designYear, setDesignYear] = useState(row.design_year);
  const [dimensions, setDimensions] = useState(row.dimensions);
  const [lightSource, setLightSource] = useState(row.light_source);

  // Bilingual copy
  const [enModelAlt, setEnModelAlt] = useState(row.content.en?.model_alt ?? "");
  const [zhModelAlt, setZhModelAlt] = useState(row.content["zh-HK"]?.model_alt ?? "");
  const [enColourway, setEnColourway] = useState(row.content.en?.colourway ?? "");
  const [zhColourway, setZhColourway] = useState(row.content["zh-HK"]?.colourway ?? "");
  const [enHeritage, setEnHeritage] = useState(row.content.en?.heritage ?? "");
  const [zhHeritage, setZhHeritage] = useState(row.content["zh-HK"]?.heritage ?? "");
  const [enElectrical, setEnElectrical] = useState(row.content.en?.electrical ?? "");
  const [zhElectrical, setZhElectrical] = useState(row.content["zh-HK"]?.electrical ?? "");

  // Reference price (human dollars)
  const [refClear, setRefClear] = useState(false);
  const [refMinDollars, setRefMinDollars] = useState(
    row.reference_price
      ? formatCentsToDollarsInput(row.reference_price.min_price_cents)
      : "",
  );
  const [refMaxDollars, setRefMaxDollars] = useState(
    row.reference_price
      ? formatCentsToDollarsInput(row.reference_price.max_price_cents)
      : "",
  );
  const [refSourceName, setRefSourceName] = useState(
    row.reference_price?.source_name ?? "",
  );
  const [refSourceUrl, setRefSourceUrl] = useState(
    row.reference_price?.source_url ?? "",
  );
  const [refCheckedOn, setRefCheckedOn] = useState(
    row.reference_price?.checked_on ?? "",
  );

  // ------------------------------------------------------------------
  // G23: session-expiry work protection. Every edit is mirrored into
  // sessionStorage under `lamp_studio_draft_${product.id}` — a save bounced
  // by an expired admin session (or any remount) restores the draft verbatim
  // instead of losing operator work.
  // ------------------------------------------------------------------
  const draftKey = `lamp_studio_draft_${initialProduct.id}`;

  /**
   * Counts every edit so save COMPLETION can tell whether newer edits arrived
   * while the request was in flight — those drafts must survive (never be
   * cleared by a save that no longer covers the form).
   */
  const editVersionRef = useRef(0);

  /** Marks the form dirty AND counts the edit (see [`editVersionRef`]). */
  function markDirty() {
    editVersionRef.current += 1;
    setDirty(true);
  }

  /** Drops the persisted draft — only ever called once it is fully saved. */
  function clearStudioDraft() {
    try {
      window.sessionStorage.removeItem(draftKey);
    } catch {
      // storage unavailable — persistence is best effort
    }
  }

  function collectDraft(): StudioDraft {
    return {
      slug,
      brand,
      model,
      family,
      active,
      priceDollars,
      stockTotalInput,
      stockQuarantinedInput,
      imageDayUrl,
      imageLitUrl,
      artVariant,
      artHue,
      designer,
      designYear,
      dimensions,
      lightSource,
      enModelAlt,
      zhModelAlt,
      enColourway,
      zhColourway,
      enHeritage,
      zhHeritage,
      enElectrical,
      zhElectrical,
      refClear,
      refMinDollars,
      refMaxDollars,
      refSourceName,
      refSourceUrl,
      refCheckedOn,
    };
  }

  const draftJson = JSON.stringify(collectDraft());

  useEffect(() => {
    if (!dirty) return;
    try {
      window.sessionStorage.setItem(draftKey, draftJson);
    } catch {
      // storage unavailable — persistence is best effort
    }
  }, [dirty, draftKey, draftJson]);

  useEffect(() => {
    let draft: Partial<StudioDraft>;
    try {
      const raw = window.sessionStorage.getItem(draftKey);
      if (!raw) return;
      draft = JSON.parse(raw) as Partial<StudioDraft>;
    } catch {
      return; // corrupted or unavailable draft — ignore
    }
    const applyStr = (v: unknown, set: (s: string) => void) => {
      if (typeof v === "string") set(v);
    };
    applyStr(draft.slug, setSlug);
    applyStr(draft.brand, setBrand);
    applyStr(draft.model, setModel);
    applyStr(draft.family, setFamily);
    applyStr(draft.priceDollars, setPriceDollars);
    applyStr(draft.stockTotalInput, setStockTotalInput);
    applyStr(draft.stockQuarantinedInput, setStockQuarantinedInput);
    applyStr(draft.imageDayUrl, setImageDayUrl);
    applyStr(draft.imageLitUrl, setImageLitUrl);
    applyStr(draft.designer, setDesigner);
    applyStr(draft.designYear, setDesignYear);
    applyStr(draft.dimensions, setDimensions);
    applyStr(draft.lightSource, setLightSource);
    applyStr(draft.enModelAlt, setEnModelAlt);
    applyStr(draft.zhModelAlt, setZhModelAlt);
    applyStr(draft.enColourway, setEnColourway);
    applyStr(draft.zhColourway, setZhColourway);
    applyStr(draft.enHeritage, setEnHeritage);
    applyStr(draft.zhHeritage, setZhHeritage);
    applyStr(draft.enElectrical, setEnElectrical);
    applyStr(draft.zhElectrical, setZhElectrical);
    applyStr(draft.refMinDollars, setRefMinDollars);
    applyStr(draft.refMaxDollars, setRefMaxDollars);
    applyStr(draft.refSourceName, setRefSourceName);
    applyStr(draft.refSourceUrl, setRefSourceUrl);
    applyStr(draft.refCheckedOn, setRefCheckedOn);
    if (typeof draft.active === "boolean") setActive(draft.active);
    if (typeof draft.refClear === "boolean") setRefClear(draft.refClear);
    if (typeof draft.artHue === "number" && Number.isFinite(draft.artHue)) {
      setArtHue(draft.artHue);
    }
    if (
      typeof draft.artVariant === "string" &&
      (ART_VARIANTS as readonly string[]).includes(draft.artVariant)
    ) {
      setArtVariant(draft.artVariant as ArtVariant);
    }
    editVersionRef.current += 1;
    setDirty(true);
    setBanner(
      "Restored unsaved draft from an interrupted session — review the fields and Save All Changes to persist.",
    );
  }, [draftKey]);

  const previewCents = parseDollarsToCents(priceDollars) ?? row.price_cents;
  const availableNow = Math.max(
    0,
    row.stock_total - row.stock_reserved - row.stock_quarantined,
  );

  async function handleFileUpload(
    field: "day" | "lit",
    e: ChangeEvent<HTMLInputElement>,
  ) {
    const file = e.target.files?.[0];
    if (!file) return;
    setUploadingField(field);
    setErrorMsg(null);
    try {
      const formData = new FormData();
      formData.append("file", file);
      const res = await fetch("/admin/api/upload", {
        method: "POST",
        body: formData,
      });
      const data = (await res.json().catch(() => ({}))) as {
        url?: string;
        error?: string;
      };
      if (!res.ok || !data.url) {
        setErrorMsg(data.error ?? "Upload failed");
        return;
      }
      if (field === "day") {
        setImageDayUrl(data.url);
      } else {
        setImageLitUrl(data.url);
      }
      markDirty();
      setBanner(`Uploaded ${file.name} -> ${data.url} (click Save All Changes to persist)`);
    } finally {
      setUploadingField(null);
    }
  }

  async function handleSaveAll(e: FormEvent) {
    e.preventDefault();
    if (saving) return;
    setSaving(true);
    setErrorMsg(null);
    setBanner(null);

    const parsedPriceCents = parseDollarsToCents(priceDollars);
    if (parsedPriceCents === null) {
      setErrorMsg("Price must be a valid non-negative dollar amount (e.g. 1615 or 1615.50).");
      setSaving(false);
      return;
    }

    const contentPayload: Record<string, Record<string, string>> = {};
    const enFields: Record<string, string> = {};
    if (enModelAlt.trim()) enFields.model_alt = enModelAlt.trim();
    if (enColourway.trim()) enFields.colourway = enColourway.trim();
    if (enHeritage.trim()) enFields.heritage = enHeritage.trim();
    if (enElectrical.trim()) enFields.electrical = enElectrical.trim();
    if (Object.keys(enFields).length > 0) contentPayload.en = enFields;

    const zhFields: Record<string, string> = {};
    if (zhModelAlt.trim()) zhFields.model_alt = zhModelAlt.trim();
    if (zhColourway.trim()) zhFields.colourway = zhColourway.trim();
    if (zhHeritage.trim()) zhFields.heritage = zhHeritage.trim();
    if (zhElectrical.trim()) zhFields.electrical = zhElectrical.trim();
    if (Object.keys(zhFields).length > 0) contentPayload["zh-HK"] = zhFields;

    let referencePayload: {
      min_price_cents: number;
      max_price_cents: number;
      source_name: string;
      source_url: string;
      checked_on: string;
    } | null = null;

    const anyRefTyped =
      refMinDollars.trim() !== "" ||
      refMaxDollars.trim() !== "" ||
      refSourceName.trim() !== "" ||
      refSourceUrl.trim() !== "" ||
      refCheckedOn.trim() !== "";

    if (!refClear && anyRefTyped) {
      const minCents = parseDollarsToCents(refMinDollars);
      const maxCents = parseDollarsToCents(refMaxDollars);
      if (minCents === null || maxCents === null) {
        setErrorMsg("Reference min and max prices must be valid dollar numbers.");
        setSaving(false);
        return;
      }
      referencePayload = {
        min_price_cents: minCents,
        max_price_cents: maxCents,
        source_name: refSourceName.trim(),
        source_url: refSourceUrl.trim(),
        checked_on: refCheckedOn.trim(),
      };
    }

    const payload: Record<string, unknown> = {
      slug: slug.trim(),
      brand: brand.trim(),
      model: model.trim(),
      price_cents: parsedPriceCents,
      family: family.trim(),
      art_variant: artVariant,
      art_hue: Number(artHue),
      active,
      image_day_url: imageDayUrl.trim(),
      image_lit_url: imageLitUrl.trim(),
      designer: designer.trim(),
      design_year: designYear.trim(),
      dimensions: dimensions.trim(),
      light_source: lightSource.trim(),
      content: contentPayload,
      reference_price: referencePayload,
    };

    if (stockTotalInput.trim() !== "") {
      payload.stock_total = Number.parseInt(stockTotalInput.trim(), 10);
    }
    if (stockQuarantinedInput.trim() !== "") {
      payload.stock_quarantined = Number.parseInt(stockQuarantinedInput.trim(), 10);
    }

    // G23: snapshot the draft immediately before the save call so a bounced
    // save (expired session, redirect to login) can be restored verbatim.
    try {
      window.sessionStorage.setItem(draftKey, JSON.stringify(collectDraft()));
    } catch {
      // best effort
    }
    const editVersionAtSaveStart = editVersionRef.current;

    try {
      const res = await fetch(`/admin/api/products/${row.id}/full`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      const data = (await res.json().catch(() => ({}))) as AdminProductFullData & {
        error?: string;
      };
      if (!res.ok) {
        setErrorMsg(data.error ?? `Save rejected (HTTP ${res.status})`);
        return;
      }
      setRow(data);
      if (editVersionRef.current === editVersionAtSaveStart) {
        // Nothing was edited while the save was in flight: the whole draft is
        // persisted now and can go. (Newer edits keep the dirty flag AND the
        // draft — they are still unsaved work.)
        setStockTotalInput("");
        setStockQuarantinedInput("");
        setDirty(false);
        clearStudioDraft();
      }
      setBanner(`Saved ${data.slug} — live on storefront immediately (0s sync).`);
      router.refresh();
    } catch {
      setErrorMsg("Network error while saving product.");
    } finally {
      setSaving(false);
    }
  }

  async function handleCloneProduct() {
    if (cloning) return;
    setCloning(true);
    setErrorMsg(null);
    try {
      const res = await fetch(`/admin/api/products/${row.id}/clone`, {
        method: "POST",
      });
      const data = (await res.json().catch(() => ({}))) as {
        id?: number;
        slug?: string;
        error?: string;
      };
      if (!res.ok || typeof data.id !== "number") {
        setErrorMsg(data.error ?? "Clone failed");
        return;
      }
      router.push(
        `/admin/products/${data.id}?saved=${encodeURIComponent(`Cloned from #${row.id} into ${data.slug}`)}`,
      );
    } finally {
      setCloning(false);
    }
  }

  async function handleQuarantineAction(action: "restock" | "write_off") {
    setErrorMsg(null);
    const res = await fetch(
      `/admin/api/products/${row.id}/inspect-quarantine`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action, quantity: 1 }),
      },
    );
    const data = (await res.json().catch(() => ({}))) as AdminProductFullData & {
      error?: string;
    };
    if (!res.ok) {
      setErrorMsg(data.error ?? "Quarantine action failed");
      return;
    }
    setRow(data);
    setBanner(
      action === "restock"
        ? "1 unit passed inspection and returned to saleable vault stock."
        : "1 damaged unit permanently written off from total stock.",
    );
    router.refresh();
  }

  const inputClass =
    "w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2 text-sm text-[var(--text-primary)] focus:border-[var(--color-glow)] focus:outline-none";

  return (
    <div className="space-y-8">
      {/* Studio Top Bar */}
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-[var(--border-hairline)] pb-5">
        <div>
          <div className="flex items-center gap-3">
            <Link
              href="/admin/products"
              className="text-xs tracking-[0.14em] uppercase text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              ← All Products
            </Link>
            <span className="text-xs text-[var(--text-secondary)]">·</span>
            <span className="font-mono text-xs text-[var(--text-secondary)]">
              SKU #{row.id} ({row.slug})
            </span>
            {dirty && (
              <span className="border border-[var(--color-glow)] px-2 py-0.5 text-[10px] tracking-[0.14em] uppercase text-[var(--color-glow)]">
                Unsaved Edits
              </span>
            )}
          </div>
          <h1 className="mt-1 font-display text-3xl">
            {brand} · {model}
          </h1>
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <button
            type="button"
            disabled={cloning}
            onClick={handleCloneProduct}
            className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-4 py-2 text-xs tracking-[0.14em] uppercase hover:border-[var(--color-glow)]"
          >
            {cloning ? "Cloning..." : "⧉ Duplicate / Clone SKU"}
          </button>
          {row.active && (
            <Link
              href={`/en/products/${row.slug}`}
              target="_blank"
              className="border border-[var(--border-hairline)] bg-[var(--surface-card)] px-4 py-2 text-xs tracking-[0.14em] uppercase hover:border-[var(--color-glow)]"
            >
              View on Storefront ↗
            </Link>
          )}
        </div>
      </div>

      {banner && (
        <div
          role="status"
          className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-card)] p-4 text-sm"
        >
          {banner}
        </div>
      )}

      {errorMsg && (
        <div
          role="alert"
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm text-[var(--color-error)] font-medium"
        >
          {errorMsg}
        </div>
      )}

      {/* Real-Time As-You-Type Stage Preview (Daylight + Nocturne 2700K + Scale CAD) */}
      <section className="card-exhibition p-6">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-[var(--border-hairline)] pb-4">
          <h2 className="font-display text-xl">
            Live Stage Preview (Updates Instantaneously Before Saving)
          </h2>
          <span className="font-mono text-xs text-[var(--text-secondary)]">
            Silhouette: {artVariant} · Hue: {artHue}° · Price:{" "}
            {formatMoney(previewCents, currency)}
          </span>
        </div>
        <div className="mt-5 grid gap-6 sm:grid-cols-3">
          <div>
            <LampStage
              hue={artHue}
              variant={artVariant}
              lit={false}
              imageDayUrl={imageDayUrl}
              imageLitUrl={imageLitUrl}
              aspectClass="aspect-square"
            />
            <p className="mt-2 text-center text-xs tracking-[0.14em] uppercase text-[var(--text-secondary)]">
              Daylight 5500K Cutout
            </p>
          </div>
          <div>
            <LampStage
              hue={artHue}
              variant={artVariant}
              lit={true}
              imageDayUrl={imageDayUrl}
              imageLitUrl={imageLitUrl}
              aspectClass="aspect-square"
            />
            <p className="mt-2 text-center text-xs tracking-[0.14em] uppercase text-[var(--color-glow)]">
              Nocturne 2700K Lit
            </p>
          </div>
          <div>
            <LampStage
              hue={artHue}
              variant={artVariant}
              lit={true}
              dimensionsText={dimensions}
              mode="scale"
              aspectClass="aspect-square"
            />
            <p className="mt-2 text-center text-xs tracking-[0.14em] uppercase text-[var(--text-secondary)]">
              1:10 CAD Scale Blueprint
            </p>
          </div>
        </div>
      </section>

      {/* Unified Single-Save Form */}
      <form
        onSubmit={handleSaveAll}
        onChange={markDirty}
        className="space-y-8"
      >
        {/* Section 1: Identity, Visibility, Human Dollar Pricing & Stock */}
        <section className="card-exhibition space-y-5 p-6">
          <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
            1. Identity, Storefront Visibility, Pricing & Vault Inventory
          </h2>

          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <label htmlFor="st-slug" className="text-xs text-[var(--text-secondary)]">
                Slug (URL Path) *
              </label>
              <input
                id="st-slug"
                type="text"
                required
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-brand" className="text-xs text-[var(--text-secondary)]">
                Maker / Brand *
              </label>
              <input
                id="st-brand"
                type="text"
                required
                value={brand}
                onChange={(e) => setBrand(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-model" className="text-xs text-[var(--text-secondary)]">
                Model Name (Latin) *
              </label>
              <input
                id="st-model"
                type="text"
                required
                value={model}
                onChange={(e) => setModel(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-family" className="text-xs text-[var(--text-secondary)]">
                Sibling Family Group
              </label>
              <input
                id="st-family"
                type="text"
                placeholder="e.g. panthella-160-portable"
                value={family}
                onChange={(e) => setFamily(e.target.value)}
                className={inputClass}
              />
            </div>
          </div>

          <div className="grid items-end gap-4 pt-2 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <label htmlFor="st-price-dollars" className="text-xs text-[var(--text-secondary)]">
                House Price in {currency} (Dollars, e.g. 1615) *
              </label>
              <input
                id="st-price-dollars"
                type="text"
                required
                value={priceDollars}
                onChange={(e) => setPriceDollars(e.target.value)}
                className={`${inputClass} font-mono font-semibold`}
              />
              <p className="mt-1 text-[11px] text-[var(--text-secondary)]">
                Displays as: <strong>{formatMoney(previewCents, currency)}</strong> (
                {previewCents} cents)
              </p>
            </div>

            <div>
              <label htmlFor="st-stock-total" className="text-xs text-[var(--text-secondary)]">
                Stock Total (Current: {row.stock_total})
              </label>
              <input
                id="st-stock-total"
                type="number"
                min={0}
                step={1}
                placeholder={`Leave blank to keep ${row.stock_total}`}
                value={stockTotalInput}
                onChange={(e) => setStockTotalInput(e.target.value)}
                className={inputClass}
              />
            </div>

            <div>
              <label htmlFor="st-stock-quar" className="text-xs text-[var(--text-secondary)]">
                Quarantined (Current: {row.stock_quarantined})
              </label>
              <input
                id="st-stock-quar"
                type="number"
                min={0}
                step={1}
                placeholder={`Leave blank to keep ${row.stock_quarantined}`}
                value={stockQuarantinedInput}
                onChange={(e) => setStockQuarantinedInput(e.target.value)}
                className={inputClass}
              />
            </div>

            <div className="border border-[var(--border-hairline)] bg-[var(--surface-page)] p-3 text-xs">
              <div className="flex justify-between">
                <span className="text-[var(--text-secondary)]">Reserved:</span>
                <strong className="font-mono">{row.stock_reserved}</strong>
              </div>
              <div className="mt-1 flex justify-between">
                <span className="text-[var(--text-secondary)]">Saleable Now:</span>
                <strong className="font-mono text-[var(--color-glow)]">
                  {availableNow}
                </strong>
              </div>
            </div>
          </div>

          {/* Quarantine Quick-Inspection Bar when Quarantined > 0 */}
          {row.stock_quarantined > 0 && (
            <div className="flex flex-wrap items-center justify-between gap-4 border border-[var(--color-glow)] bg-[var(--surface-elevated)] p-4 text-xs">
              <span>
                <strong>{row.stock_quarantined} unit(s)</strong> currently held in
                quarantine inspection.
              </span>
              <div className="flex gap-3">
                <button
                  type="button"
                  onClick={() => handleQuarantineAction("restock")}
                  className="bg-[var(--color-success)] px-3.5 py-1.5 text-white uppercase tracking-[0.12em]"
                >
                  ✓ Pass Inspection (Restock +1)
                </button>
                <button
                  type="button"
                  onClick={() => handleQuarantineAction("write_off")}
                  className="bg-[var(--color-error)] px-3.5 py-1.5 text-white uppercase tracking-[0.12em]"
                >
                  ✕ Write Off Damaged (−1)
                </button>
              </div>
            </div>
          )}

          <label className="flex cursor-pointer items-center gap-3 pt-2 text-sm">
            <input
              type="checkbox"
              checked={active}
              onChange={(e) => setActive(e.target.checked)}
              className="size-4 accent-[var(--color-glow)]"
            />
            <span>
              <strong>Active on Public Storefront</strong> (uncheck to hide SKU
              from public catalogue while keeping all data editable here)
            </span>
          </label>
        </section>

        {/* Section 2: Media Upload / URLs, Silhouette & Glow Hue */}
        <section className="card-exhibition space-y-5 p-6">
          <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
            2. Photography Upload / URLs, Architectural Silhouette & 2700K Hue
          </h2>

          <div className="grid gap-6 sm:grid-cols-2">
            <div className="space-y-2">
              <label htmlFor="st-img-day" className="text-xs text-[var(--text-secondary)]">
                Daylight 5500K Cutout Image URL (or Upload File)
              </label>
              <input
                id="st-img-day"
                type="text"
                placeholder="https://... or /uploads/..."
                value={imageDayUrl}
                onChange={(e) => setImageDayUrl(e.target.value)}
                className={inputClass}
              />
              <div className="flex items-center gap-3 text-xs">
                <label className="cursor-pointer border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-1.5 hover:border-[var(--color-glow)]">
                  <span>
                    {uploadingField === "day"
                      ? "Uploading..."
                      : "↑ Upload Daylight Photo"}
                  </span>
                  <input
                    type="file"
                    accept="image/*"
                    onChange={(e) => handleFileUpload("day", e)}
                    className="hidden"
                  />
                </label>
                {imageDayUrl && (
                  <button
                    type="button"
                    onClick={() => {
                      setImageDayUrl("");
                      markDirty();
                    }}
                    className="text-[var(--text-secondary)] underline"
                  >
                    Clear URL
                  </button>
                )}
              </div>
            </div>

            <div className="space-y-2">
              <label htmlFor="st-img-lit" className="text-xs text-[var(--text-secondary)]">
                Nocturne 2700K Lit Image URL (or Upload File)
              </label>
              <input
                id="st-img-lit"
                type="text"
                placeholder="https://... or /uploads/..."
                value={imageLitUrl}
                onChange={(e) => setImageLitUrl(e.target.value)}
                className={inputClass}
              />
              <div className="flex items-center gap-3 text-xs">
                <label className="cursor-pointer border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-1.5 hover:border-[var(--color-glow)]">
                  <span>
                    {uploadingField === "lit"
                      ? "Uploading..."
                      : "↑ Upload 2700K Lit Photo"}
                  </span>
                  <input
                    type="file"
                    accept="image/*"
                    onChange={(e) => handleFileUpload("lit", e)}
                    className="hidden"
                  />
                </label>
                {imageLitUrl && (
                  <button
                    type="button"
                    onClick={() => {
                      setImageLitUrl("");
                      markDirty();
                    }}
                    className="text-[var(--text-secondary)] underline"
                  >
                    Clear URL
                  </button>
                )}
              </div>
            </div>
          </div>

          <div className="grid gap-6 pt-2 sm:grid-cols-2">
            <div>
              <label htmlFor="st-variant" className="text-xs text-[var(--text-secondary)]">
                Architectural Blueprint Silhouette
              </label>
              <select
                id="st-variant"
                value={artVariant}
                onChange={(e) => setArtVariant(e.target.value as ArtVariant)}
                className={inputClass}
              >
                {ART_VARIANTS.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            </div>

            <div>
              <label htmlFor="st-hue" className="text-xs text-[var(--text-secondary)]">
                Ambient Glow Hue ({artHue}° · 0–359)
              </label>
              <div className="mt-1 flex items-center gap-4">
                <input
                  id="st-hue"
                  type="range"
                  min={0}
                  max={359}
                  value={artHue}
                  onChange={(e) => setArtHue(Number(e.target.value))}
                  className="flex-1 accent-[var(--color-glow)]"
                />
                <input
                  type="number"
                  min={0}
                  max={359}
                  value={artHue}
                  onChange={(e) => setArtHue(Number(e.target.value))}
                  className="w-20 border border-[var(--border-hairline)] bg-[var(--surface-page)] px-2.5 py-1.5 font-mono text-sm"
                />
              </div>
            </div>
          </div>
        </section>

        {/* Section 3: Architectural Specifications */}
        <section className="card-exhibition space-y-4 p-6">
          <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
            3. Architectural & Optical Specifications
          </h2>
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <div>
              <label htmlFor="st-designer" className="text-xs text-[var(--text-secondary)]">
                Designer
              </label>
              <input
                id="st-designer"
                type="text"
                value={designer}
                onChange={(e) => setDesigner(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-year" className="text-xs text-[var(--text-secondary)]">
                Design Year
              </label>
              <input
                id="st-year"
                type="text"
                value={designYear}
                onChange={(e) => setDesignYear(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-dims" className="text-xs text-[var(--text-secondary)]">
                Dimensions (mm)
              </label>
              <input
                id="st-dims"
                type="text"
                placeholder="Ø 160 × H 238 mm"
                value={dimensions}
                onChange={(e) => setDimensions(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label htmlFor="st-light" className="text-xs text-[var(--text-secondary)]">
                Light Source
              </label>
              <input
                id="st-light"
                type="text"
                value={lightSource}
                onChange={(e) => setLightSource(e.target.value)}
                className={inputClass}
              />
            </div>
          </div>
        </section>

        {/* Section 4: Side-by-Side Bilingual Copy (English | 繁體中文) */}
        <section className="card-exhibition space-y-5 p-6">
          <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
            4. Side-by-Side Bilingual Editorial Copy (English | 繁體中文)
          </h2>

          <div className="grid gap-6 sm:grid-cols-2">
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Colourway (English)
              </label>
              <input
                type="text"
                value={enColourway}
                onChange={(e) => setEnColourway(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                配色名稱 (繁體中文)
              </label>
              <input
                type="text"
                value={zhColourway}
                onChange={(e) => setZhColourway(e.target.value)}
                className={inputClass}
              />
            </div>

            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Localized Model Name Override (English, optional)
              </label>
              <input
                type="text"
                value={enModelAlt}
                onChange={(e) => setEnModelAlt(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                型號顯示名稱 (繁體中文)
              </label>
              <input
                type="text"
                value={zhModelAlt}
                onChange={(e) => setZhModelAlt(e.target.value)}
                className={inputClass}
              />
            </div>

            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Architectural Heritage Story (English)
              </label>
              <textarea
                rows={4}
                value={enHeritage}
                onChange={(e) => setEnHeritage(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                建築與光學傳承故事 (繁體中文)
              </label>
              <textarea
                rows={4}
                value={zhHeritage}
                onChange={(e) => setZhHeritage(e.target.value)}
                className={inputClass}
              />
            </div>

            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Electrical & HK Compatibility (English)
              </label>
              <textarea
                rows={3}
                value={enElectrical}
                onChange={(e) => setEnElectrical(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                香港電壓與插頭規格 (繁體中文)
              </label>
              <textarea
                rows={3}
                value={zhElectrical}
                onChange={(e) => setZhElectrical(e.target.value)}
                className={inputClass}
              />
            </div>
          </div>
        </section>

        {/* Section 5: Verified Reference Price (in Human Dollars!) */}
        <section className="card-exhibition space-y-4 p-6">
          <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
            5. Verified HK Showroom Reference Pricing (Market Transparency)
          </h2>

          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Min Price ({currency} Dollars)
              </label>
              <input
                type="text"
                placeholder="e.g. 1900"
                value={refMinDollars}
                onChange={(e) => setRefMinDollars(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Max Price ({currency} Dollars)
              </label>
              <input
                type="text"
                placeholder="e.g. 2800"
                value={refMaxDollars}
                onChange={(e) => setRefMaxDollars(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Showroom Source Name
              </label>
              <input
                type="text"
                placeholder="MyConcept Hong Kong"
                value={refSourceName}
                onChange={(e) => setRefSourceName(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Source URL (https://...)
              </label>
              <input
                type="url"
                placeholder="https://..."
                value={refSourceUrl}
                onChange={(e) => setRefSourceUrl(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-[var(--text-secondary)]">
                Checked Date (YYYY-MM-DD)
              </label>
              <input
                type="text"
                placeholder="2026-10-12"
                value={refCheckedOn}
                onChange={(e) => setRefCheckedOn(e.target.value)}
                className={inputClass}
              />
            </div>
          </div>

          <label className="flex cursor-pointer items-center gap-2.5 pt-2 text-xs text-[var(--text-secondary)]">
            <input
              type="checkbox"
              checked={refClear}
              onChange={(e) => setRefClear(e.target.checked)}
            />
            <span>
              Clear reference price on save (sets storefront back to
              &ldquo;Comparison pricing being verified&rdquo;)
            </span>
          </label>
        </section>

        {/* Save Footer */}
        <div className="flex flex-wrap items-center justify-between gap-4 border border-[var(--border-strong)] bg-[var(--surface-card)] px-6 py-4">
          <span className="text-xs text-[var(--text-secondary)]">
            Saving commits all 5 sections in one database transaction and
            updates the live storefront in 0 seconds.
          </span>
          <button
            type="submit"
            disabled={saving}
            className="bg-[var(--color-glow)] px-8 py-3 text-xs font-semibold tracking-[0.2em] uppercase text-[#11100e] hover:opacity-90 disabled:opacity-50"
          >
            {saving ? "Saving Changes..." : "Save All Changes"}
          </button>
        </div>
      </form>

      {/* Section 6: Numbered Edition Pieces (01..16) Assignment */}
      <section className="card-exhibition space-y-5 p-6">
        <h2 className="border-b border-[var(--border-hairline)] pb-3 font-display text-xl">
          6. Numbered Drop Pieces Assigned to This SKU ({row.pieces.length})
        </h2>

        {row.pieces.length > 0 ? (
          <table className="w-full border-collapse text-left text-sm">
            <thead>
              <tr className="border-b border-[var(--border-hairline)] text-xs text-[var(--text-secondary)]">
                <th className="py-2">Piece #</th>
                <th className="py-2">Reassign to Another SKU</th>
                <th className="py-2 text-right">Remove</th>
              </tr>
            </thead>
            <tbody>
              {row.pieces.map((pieceNo) => (
                <tr
                  key={pieceNo}
                  className="border-b border-[var(--border-hairline)]"
                >
                  <td className="py-3 font-mono font-semibold">
                    #{String(pieceNo).padStart(2, "0")}
                  </td>
                  <td className="py-3">
                    <form
                      method="post"
                      action={`/admin/api/pieces/${pieceNo}`}
                      className="flex flex-wrap items-center gap-2"
                    >
                      <input
                        type="hidden"
                        name="return_product_id"
                        value={row.id}
                      />
                      <select
                        name="product_id"
                        defaultValue={row.id}
                        className="border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-1.5 text-xs"
                      >
                        {allProducts.map((p) => (
                          <option key={p.id} value={p.id}>
                            #{p.id} · {p.brand} {p.model} ({p.slug})
                          </option>
                        ))}
                      </select>
                      <button
                        type="submit"
                        name="action"
                        value="reassign"
                        className="border border-[var(--border-hairline)] px-3 py-1.5 text-xs uppercase hover:border-[var(--color-glow)]"
                      >
                        Reassign
                      </button>
                    </form>
                  </td>
                  <td className="py-3 text-right">
                    <form method="post" action={`/admin/api/pieces/${pieceNo}`}>
                      <input
                        type="hidden"
                        name="return_product_id"
                        value={row.id}
                      />
                      <button
                        type="submit"
                        name="action"
                        value="delete"
                        className="border border-[var(--border-hairline)] px-3 py-1.5 text-xs text-[var(--color-error)] uppercase hover:border-[var(--color-error)]"
                      >
                        Delete
                      </button>
                    </form>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="text-xs text-[var(--text-secondary)]">
            No numbered drop pieces assigned yet — this SKU currently renders on
            the storefront as an <strong>Open Edition</strong>.
          </p>
        )}

        <form
          method="post"
          action="/admin/api/pieces"
          className="flex flex-wrap items-end gap-3 border-t border-[var(--border-hairline)] pt-4"
        >
          <input type="hidden" name="product_id" value={row.id} />
          <div>
            <label
              htmlFor="new-piece-input"
              className="block text-xs text-[var(--text-secondary)]"
            >
              Assign New Piece Number (≥ 1)
            </label>
            <input
              id="new-piece-input"
              name="piece_no"
              type="number"
              min={1}
              step={1}
              required
              placeholder="e.g. 17"
              className="mt-1 w-36 border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 font-mono text-sm"
            />
          </div>
          <button
            type="submit"
            className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-5 py-2 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)]"
          >
            + Assign Piece
          </button>
        </form>
      </section>
    </div>
  );
}
