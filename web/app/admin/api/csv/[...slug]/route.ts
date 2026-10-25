import { revalidatePath } from "next/cache";
import {
  ADMIN_COOKIE_NAME,
  adminFetch,
  getAdminCookie,
  verifyAdminSession,
  verifySameOriginRequest,
  XRW_HEADER,
  XRW_VALUE,
} from "@/lib/admin-auth";
import {
  CSV_SYNC_MAX_BYTES,
  buildCsvSyncPayload,
  csvApiPath,
  getApiBaseUrl,
  parseCsvSyncOptions,
  type CsvSyncKind,
  type CsvSyncPhase,
} from "@/lib/api";

export const dynamic = "force-dynamic";

/**
 * 1-Click Excel/CSV Bulk Sync proxy (DATA-01 / ADMIN-01).
 *
 *   GET  /admin/api/csv/products              -> CSV attachment (UTF-8 BOM)
 *   GET  /admin/api/csv/site-content          -> CSV attachment (UTF-8 BOM)
 *   POST /admin/api/csv/products/preview      -> dry-run diff  (no writes)
 *   POST /admin/api/csv/products/commit       -> atomic commit
 *   POST /admin/api/csv/site-content/preview  -> dry-run diff  (no writes)
 *   POST /admin/api/csv/site-content/commit   -> atomic commit
 *
 * The browser never reaches the Rust API directly: this route attaches the
 * `admin_session` cookie (so the backend authorizes the CALLER, not the proxy),
 * runs the web tier's own same-origin CSRF guard on every mutation, and forces
 * `X-Requested-With: XMLHttpRequest` upstream. Only `/commit` revalidates the
 * storefront — a preview must never touch the live pages.
 */

const CSV_KINDS: readonly CsvSyncKind[] = ["products", "site-content"];
const CSV_PHASES: readonly CsvSyncPhase[] = ["preview", "commit"];

type CsvRoute = { kind: CsvSyncKind; phase: CsvSyncPhase | "export" };

function parseCsvRoute(slug: readonly string[]): CsvRoute | null {
  const [kind, phase] = slug;
  if (!kind || !(CSV_KINDS as readonly string[]).includes(kind)) return null;
  if (slug.length === 1) return { kind: kind as CsvSyncKind, phase: "export" };
  if (slug.length !== 2 || !phase) return null;
  if (!(CSV_PHASES as readonly string[]).includes(phase)) return null;
  return { kind: kind as CsvSyncKind, phase: phase as CsvSyncPhase };
}

function defaultFilename(kind: CsvSyncKind): string {
  const stamp = new Date().toISOString().slice(0, 10);
  return `${kind}-${stamp}.csv`;
}

/** Reads the spreadsheet text from a multipart upload or a JSON body. */
async function readCsvText(
  request: Request,
): Promise<
  | { ok: true; csvText: string; options: ReturnType<typeof parseCsvSyncOptions> }
  | { ok: false; error: string; status: number }
> {
  const contentType = request.headers.get("content-type") ?? "";

  if (contentType.includes("multipart/form-data")) {
    const form = await request.formData().catch(() => null);
    if (!form) {
      return { ok: false, error: "Unreadable form upload", status: 400 };
    }
    const file = form.get("file");
    const options = parseCsvSyncOptions(form);
    if (typeof file === "string") {
      return { ok: true, csvText: file, options };
    }
    if (file && typeof file.text === "function") {
      if (file.size > CSV_SYNC_MAX_BYTES) {
        return {
          ok: false,
          error: `CSV is larger than the ${Math.floor(CSV_SYNC_MAX_BYTES / (1024 * 1024))}MB limit`,
          status: 413,
        };
      }
      return { ok: true, csvText: await file.text(), options };
    }
    // No file part: fall back to an inline `csv` textarea field.
    const inline = form.get("csv");
    if (typeof inline === "string" && inline.trim() !== "") {
      return { ok: true, csvText: inline, options };
    }
    return { ok: false, error: "No CSV file was uploaded", status: 400 };
  }

  const body = (await request.json().catch(() => null)) as
    | Record<string, unknown>
    | null;
  if (!body) {
    return { ok: false, error: "Invalid JSON payload", status: 400 };
  }
  const options = parseCsvSyncOptions({
    get: (name: string) => body[name],
  });
  const raw = body.csv ?? body.csv_text;
  if (typeof raw !== "string" || raw.trim() === "") {
    return { ok: false, error: "No CSV content in the request", status: 400 };
  }
  return { ok: true, csvText: raw, options };
}

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ slug: string[] }> },
): Promise<Response> {
  const { slug } = await params;
  const route = parseCsvRoute(slug);
  if (!route || route.phase !== "export") {
    return Response.json({ error: "Unknown CSV export path" }, { status: 404 });
  }

  const token = await getAdminCookie();
  if (!token) {
    // A download link is a top-level navigation: send the operator to sign in
    // rather than dumping a JSON 401 into the browser tab.
    return new Response(null, {
      status: 303,
      headers: { Location: "/admin/login" },
    });
  }

  let upstream: Response;
  try {
    upstream = await fetch(`${getApiBaseUrl()}${csvApiPath(route.kind, "export")}`, {
      cache: "no-store",
      headers: {
        Cookie: `${ADMIN_COOKIE_NAME}=${token}`,
        Accept: "text/csv",
      },
    });
  } catch (err) {
    return Response.json(
      {
        error:
          err instanceof Error
            ? `CSV export failed: ${err.message}`
            : "CSV export failed",
      },
      { status: 503 },
    );
  }

  if (!upstream.ok || !upstream.body) {
    const detail = await upstream.text().catch(() => "");
    return Response.json(
      {
        error:
          detail.trim() !== ""
            ? detail.trim()
            : `CSV export rejected (HTTP ${upstream.status})`,
      },
      { status: upstream.status === 200 ? 502 : upstream.status },
    );
  }

  const headers = new Headers({
    // The upstream body is passed through BYTE-FOR-BYTE so its UTF-8 BOM
    // survives — that BOM is what makes Excel open the file as UTF-8 instead
    // of mangling 繁體中文 into mojibake.
    "Content-Type": "text/csv; charset=utf-8",
    "Content-Disposition":
      upstream.headers.get("content-disposition") ??
      `attachment; filename="${defaultFilename(route.kind)}"`,
    "Cache-Control": "no-store",
  });

  return new Response(upstream.body, { status: 200, headers });
}

export async function POST(
  request: Request,
  { params }: { params: Promise<{ slug: string[] }> },
): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json({ error: "Cross-origin request rejected" }, { status: 403 });
  }
  const session = await verifyAdminSession();
  if (!session) {
    return Response.json(
      { error: "Session expired. Please sign in again." },
      { status: 401 },
    );
  }

  const { slug } = await params;
  const route = parseCsvRoute(slug);
  if (!route || route.phase === "export") {
    return Response.json({ error: "Unknown CSV sync path" }, { status: 404 });
  }

  const parsed = await readCsvText(request);
  if (!parsed.ok) {
    return Response.json({ error: parsed.error }, { status: parsed.status });
  }
  const csvText = parsed.csvText;
  if (new TextEncoder().encode(csvText).byteLength > CSV_SYNC_MAX_BYTES) {
    return Response.json(
      { error: "CSV is larger than the 8MB import limit" },
      { status: 413 },
    );
  }
  if (csvText.trim() === "") {
    return Response.json({ error: "The uploaded CSV is empty" }, { status: 400 });
  }

  const { status, ok, data } = await adminFetch(csvApiPath(route.kind, route.phase), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      [XRW_HEADER]: XRW_VALUE,
    },
    body: JSON.stringify(buildCsvSyncPayload(csvText, parsed.options)),
  });

  // Only a committed import changes the storefront. A dry-run (or a rejected
  // commit) leaves every cached page exactly as it was.
  if (ok && route.phase === "commit") {
    revalidatePath("/", "layout");
  }

  return Response.json(data ?? {}, { status });
}
