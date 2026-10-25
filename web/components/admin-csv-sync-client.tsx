"use client";

import { useRef, useState, type ChangeEvent } from "react";
import { useRouter } from "next/navigation";
import {
  CSV_STOCK_CONFLICT_MODES,
  DEFAULT_CSV_STOCK_CONFLICT_MODE,
  normalizeCsvSyncReport,
  type CsvStockConflictMode,
  type CsvSyncKind,
  type CsvSyncReport,
} from "@/lib/api";

const STOCK_CONFLICT_LABELS: Record<CsvStockConflictMode, string> = {
  keep_db: "Keep live stock — ignore the CSV stock columns entirely",
  apply_delta:
    "Apply the added delta to live stock (default) — a sale made while you were in Excel is never rewound",
  force_override:
    "Force the CSV total onto live stock — only when the spreadsheet is definitely correct",
};

type BusyState = "preview" | "commit" | null;

/**
 * 1-Click Excel/CSV Bulk Sync (DATA-01 / ADMIN-01).
 *
 * Operator flow: (1) export the current spreadsheet, (2) edit it in Excel,
 * (3) upload it for a DRY-RUN before→after diff, (4) commit atomically in one
 * click. Nothing reaches the database before step 4, and any change to the file
 * or the options invalidates the preview — the operator can never commit a
 * commit that was not previewed exactly as uploaded.
 */
export default function AdminCsvSyncClient({ mode }: { mode: CsvSyncKind }) {
  const router = useRouter();
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const [file, setFile] = useState<File | null>(null);
  const [applyStock, setApplyStock] = useState(false);
  const [stockConflictMode, setStockConflictMode] =
    useState<CsvStockConflictMode>(DEFAULT_CSV_STOCK_CONFLICT_MODE);
  const [allowSlugRename, setAllowSlugRename] = useState(false);

  const [report, setReport] = useState<CsvSyncReport | null>(null);
  const [busy, setBusy] = useState<BusyState>(null);
  const [banner, setBanner] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const isProducts = mode === "products";
  const noun = isProducts ? "Products & Stock" : "Site Copy";

  /** Any edit to the input or the options voids the dry-run result. */
  function invalidatePreview() {
    setReport(null);
    setBanner(null);
    setErrorMsg(null);
  }

  function handleFileChange(e: ChangeEvent<HTMLInputElement>) {
    const picked = e.target.files?.[0] ?? null;
    setFile(picked);
    invalidatePreview();
  }

  function buildForm(): FormData {
    const form = new FormData();
    if (file) form.append("file", file);
    form.append("apply_stock", String(applyStock));
    form.append("stock_conflict_mode", stockConflictMode);
    form.append("allow_slug_rename", String(allowSlugRename));
    return form;
  }

  async function runPhase(phase: "preview" | "commit") {
    if (!file || busy) return;
    setBusy(phase);
    setErrorMsg(null);
    setBanner(null);
    try {
      const res = await fetch(`/admin/api/csv/${mode}/${phase}`, {
        method: "POST",
        body: buildForm(),
      });
      const payload = (await res.json().catch(() => null)) as unknown;
      if (!res.ok) {
        const message =
          typeof payload === "object" && payload !== null
            ? ((payload as { error?: string }).error ?? null)
            : null;
        setErrorMsg(message ?? `Import ${phase} rejected (HTTP ${res.status})`);
        return;
      }
      const normalized = normalizeCsvSyncReport(payload);
      if (phase === "preview") {
        setReport(normalized);
        if (!normalized.ok) {
          setErrorMsg(
            normalized.errors.length > 0
              ? "Dry run found problems. Fix them in Excel and upload again — nothing was written."
              : "Dry run was rejected by the API — nothing was written.",
          );
        }
        return;
      }
      const { created, updated, unchanged } = normalized.summary;
      setBanner(
        `Committed ${noun}: ${created} created, ${updated} updated, ${unchanged} unchanged. Storefront revalidated (0s sync).`,
      );
      setReport(null);
      setFile(null);
      if (fileInputRef.current) fileInputRef.current.value = "";
      router.refresh();
    } catch {
      setErrorMsg(
        `Network error while running the CSV ${phase}. Nothing was changed.`,
      );
    } finally {
      setBusy(null);
    }
  }

  const canPreview = file !== null && busy === null;
  const canCommit = canPreview && report !== null && report.ok;
  const blockingErrors = report?.errors ?? [];

  return (
    <section className="card-exhibition space-y-6 p-6">
      <div className="flex flex-wrap items-start justify-between gap-4 border-b border-[var(--border-hairline)] pb-4">
        <div>
          <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
            1-CLICK EXCEL / CSV BULK SYNC
          </p>
          <h2 className="mt-1 font-display text-xl">
            {noun} Spreadsheet Import & Export
          </h2>
          <p className="mt-1 max-w-2xl text-xs text-[var(--text-secondary)]">
            Export a UTF-8 spreadsheet, edit it in Excel, then upload it for a
            dry-run diff before committing. The dry run writes nothing.
          </p>
        </div>

        {/* 1-click export: a plain link keeps the browser's download handling. */}
        <a
          href={`/admin/api/csv/${mode}`}
          download
          className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-5 py-2.5 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)] hover:opacity-90"
        >
          ↓ Export {noun} CSV
        </a>
      </div>

      {banner && (
        <div
          role="status"
          aria-live="polite"
          className="border-l-4 border-[var(--color-glow)] bg-[var(--surface-card)] p-4 text-sm"
        >
          {banner}
        </div>
      )}

      {errorMsg && (
        <div
          role="alert"
          className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-sm font-medium text-[var(--color-error)]"
        >
          {errorMsg}
        </div>
      )}

      {/* Upload + options */}
      <div className="grid gap-6 lg:grid-cols-2">
        <div className="space-y-2">
          <label
            htmlFor={`csv-file-${mode}`}
            className="text-xs text-[var(--text-secondary)]"
          >
            Upload the edited CSV (dry run first) *
          </label>
          <input
            ref={fileInputRef}
            id={`csv-file-${mode}`}
            type="file"
            accept=".csv,text/csv"
            onChange={handleFileChange}
            className="w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-base file:mr-3 file:border-0 file:bg-[var(--surface-elevated)] file:px-3 file:py-1 file:text-xs file:uppercase sm:text-sm"
          />
          <p className="text-[11px] text-[var(--text-secondary)]">
            {file
              ? `Selected: ${file.name} (${Math.max(1, Math.round(file.size / 1024))} KB)`
              : "Excel must save as “CSV UTF-8” so 繁體中文 survives the round trip."}
          </p>
        </div>

        {isProducts ? (
          <div className="space-y-3">
            <label className="flex cursor-pointer items-start gap-3 text-sm">
              <input
                type="checkbox"
                checked={applyStock}
                onChange={(e) => {
                  setApplyStock(e.target.checked);
                  invalidatePreview();
                }}
                className="mt-1 size-4 accent-[var(--color-glow)]"
              />
              <span>
                <strong>Apply stock changes from the spreadsheet</strong>
                <span className="mt-0.5 block text-[11px] text-[var(--text-secondary)]">
                  Off by default: a catalogue-only edit can never silently rewrite live inventory.
                </span>
              </span>
            </label>

            <div>
              <label
                htmlFor={`csv-stock-mode-${mode}`}
                className="text-xs text-[var(--text-secondary)]"
              >
                When the CSV stock number disagrees with live stock
              </label>
              <select
                id={`csv-stock-mode-${mode}`}
                value={stockConflictMode}
                onChange={(e) => {
                  setStockConflictMode(e.target.value as CsvStockConflictMode);
                  invalidatePreview();
                }}
                className="mt-1 w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3 py-2 text-base sm:text-sm"
              >
                {CSV_STOCK_CONFLICT_MODES.map((value) => (
                  <option key={value} value={value}>
                    {STOCK_CONFLICT_LABELS[value]}
                  </option>
                ))}
              </select>
            </div>

            <label className="flex cursor-pointer items-start gap-3 text-sm">
              <input
                type="checkbox"
                checked={allowSlugRename}
                onChange={(e) => {
                  setAllowSlugRename(e.target.checked);
                  invalidatePreview();
                }}
                className="mt-1 size-4 accent-[var(--color-glow)]"
              />
              <span>
                <strong>Allow slug renames</strong>
                <span className="mt-0.5 block text-[11px] text-[var(--text-secondary)]">
                  Off by default: a renamed slug invalidates every existing
                  storefront link, so it must be intentional.
                </span>
              </span>
            </label>
          </div>
        ) : (
          <div className="flex flex-col justify-center border border-[var(--border-hairline)] bg-[var(--surface-page)] p-4 text-xs text-[var(--text-secondary)]">
            <p className="font-medium text-[var(--text-primary)]">
              Bilingual Site Copy CSV Format (<code>key,en,zh-HK</code>)
            </p>
            <p className="mt-1 leading-relaxed">
              Edit English and 繁體中文 columns side-by-side in Excel. Template
              placeholders <code>{"{brand}"}</code>, <code>{"{pieces}"}</code>,
              and <code>{"{subject}"}</code> are preserved automatically.
            </p>
          </div>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <button
          type="button"
          disabled={!canPreview}
          onClick={() => runPhase("preview")}
          className="border border-[var(--border-strong)] bg-[var(--text-primary)] px-6 py-2.5 text-xs tracking-[0.16em] uppercase text-[var(--surface-page)] disabled:opacity-50"
        >
          {busy === "preview" ? "Running dry run..." : "Run Dry-Run Preview"}
        </button>
        <button
          type="button"
          disabled={!canCommit}
          onClick={() => runPhase("commit")}
          className="bg-[var(--color-glow)] px-6 py-2.5 text-xs font-semibold tracking-[0.16em] uppercase text-[var(--text-on-glow)] disabled:opacity-40"
        >
          {busy === "commit" ? "Committing..." : "Commit Changes (1 click)"}
        </button>
        {report && report.ok && (
          <span className="text-[11px] text-[var(--text-secondary)]">
            Dry run passed: {report.summary.created} create ·{" "}
            {report.summary.updated} update · {report.summary.unchanged} unchanged
            · {report.summary.errors} error
          </span>
        )}
      </div>

      {/* Dry-run diff */}
      {report && (
        <div className="space-y-4">
          <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-[var(--border-hairline)] pb-2">
            <h3 className="font-display text-lg">
              Dry-Run Diff ({report.changes.length} field
              {report.changes.length === 1 ? "" : "s"})
            </h3>
            <span className="font-mono text-xs text-[var(--text-secondary)]">
              {report.dryRun ? "nothing written" : "committed"} ·{" "}
              {report.summary.created} create · {report.summary.updated} update ·{" "}
              {report.summary.unchanged} unchanged
            </span>
          </div>

          {blockingErrors.length > 0 && (
            <div
              role="alert"
              className="border-l-4 border-[var(--color-error)] bg-[var(--surface-card)] p-4 text-xs text-[var(--color-error)]"
            >
              <p className="font-semibold">
                {blockingErrors.length} problem
                {blockingErrors.length === 1 ? "" : "s"} found — the import will
                not run until these are fixed:
              </p>
              <ul className="mt-2 list-disc space-y-1 pl-5">
                {blockingErrors.map((issue, index) => (
                  <li key={`${issue.row}-${issue.column}-${index}`}>
                    {issue.row !== "" && (
                      <strong className="font-mono">line {issue.row}: </strong>
                    )}
                    {issue.column !== "" && (
                      <span className="font-mono">{issue.column}: </span>
                    )}
                    {issue.message}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {report.changes.length === 0 ? (
            <p className="text-xs text-[var(--text-secondary)]">
              No differences between the spreadsheet and the database — nothing
              to commit.
            </p>
          ) : (
            <div className="scroll-contain overflow-x-auto border border-[var(--border-hairline)]">
              <table className="w-full border-collapse text-left text-xs">
                <thead>
                  <tr className="border-b border-[var(--border-hairline)] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                    <th className="px-3 py-2">Row</th>
                    <th className="px-3 py-2">Line</th>
                    <th className="px-3 py-2">Field</th>
                    <th className="px-3 py-2">Before (database)</th>
                    <th className="px-3 py-2">After (spreadsheet)</th>
                    <th className="px-3 py-2">Change</th>
                  </tr>
                </thead>
                <tbody>
                  {report.changes.map((change, index) => (
                    <tr
                      key={`${change.slug}-${change.field}-${index}`}
                      className="border-b border-[var(--border-hairline)] align-top"
                    >
                      <td className="px-3 py-2 font-mono">{change.slug}</td>
                      <td className="px-3 py-2 font-mono text-[var(--text-secondary)]">
                        {change.row === "" ? "—" : `#${change.row}`}
                      </td>
                      <td className="px-3 py-2 font-mono text-[var(--text-secondary)]">
                        {change.field}
                      </td>
                      <td className="px-3 py-2 text-[var(--text-secondary)] line-through decoration-[var(--color-error)]/60">
                        {change.before === "" ? "—" : change.before}
                      </td>
                      <td className="px-3 py-2 font-medium">
                        {change.after === "" ? "—" : change.after}
                      </td>
                      <td className="px-3 py-2 uppercase tracking-[0.12em]">
                        <span
                          className={
                            change.action === "create"
                              ? "text-[var(--color-success)]"
                              : change.action === "unchanged"
                                ? "text-[var(--text-secondary)]"
                                : "text-[var(--color-glow)]"
                          }
                        >
                          {change.action}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
