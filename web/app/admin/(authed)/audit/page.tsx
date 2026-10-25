import type { Metadata } from "next";
import { adminFetch, requireAdmin } from "@/lib/admin-auth";

export const metadata: Metadata = { title: "Audit Log" };
export const dynamic = "force-dynamic";

interface AuditRow {
  id: number;
  actor: string;
  action: string;
  entity: string;
  entity_id: string | null;
  detail: unknown;
  created_at: string;
}

export default async function AdminAuditPage() {
  await requireAdmin();
  const { data } = await adminFetch("/api/admin/audit-log");
  const rows = Array.isArray(data) ? (data as AuditRow[]) : [];

  return (
    <div className="space-y-6">
      <div className="border-b border-[var(--border-hairline)] pb-5">
        <p className="text-[11px] tracking-[0.22em] uppercase text-[var(--color-glow)]">
          TRANSACTIONAL ACCOUNTABILITY LEDGER
        </p>
        <h1 className="mt-1 font-serif text-3xl">
          Audit Trail (Last {rows.length} Mutations)
        </h1>
      </div>

      <div className="scroll-contain overflow-x-auto border border-[var(--border-hairline)] bg-[var(--surface-card)]">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr className="border-b border-[var(--border-hairline)] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
              <th className="px-4 py-3">Timestamp (UTC)</th>
              <th className="px-4 py-3">Actor</th>
              <th className="px-4 py-3">Action</th>
              <th className="px-4 py-3">Entity</th>
              <th className="px-4 py-3">Detail Payload</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr
                key={row.id}
                className="border-b border-[var(--border-hairline)] align-top"
              >
                <td className="px-4 py-2.5 font-mono whitespace-nowrap">
                  {row.created_at}
                </td>
                <td className="px-4 py-2.5 font-medium">{row.actor}</td>
                <td className="px-4 py-2.5 font-mono text-[var(--color-glow)]">
                  {row.action}
                </td>
                <td className="px-4 py-2.5 font-mono">
                  {row.entity}
                  {row.entity_id ? ` #${row.entity_id}` : ""}
                </td>
                <td className="px-4 py-2.5 font-mono text-[11px] text-[var(--text-secondary)] break-all">
                  {row.detail ? JSON.stringify(row.detail) : "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
