import fs from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { verifyAdminSession, verifySameOriginRequest } from "@/lib/admin-auth";
import {
  MAX_UPLOAD_BYTES,
  validateUploadImage,
} from "@/lib/upload-validation";

export const dynamic = "force-dynamic";

/**
 * Reads the raw request body with a HARD byte cap while STREAMING: the
 * `content-length` header is a claim, not a guarantee — a hostile chunked
 * body may declare anything (or nothing) and must be cut off mid-stream,
 * never buffered first. `null` means the cap was exceeded.
 */
async function readBodyWithCap(
  request: Request,
  cap: number,
): Promise<Buffer | null> {
  const stream = request.body;
  if (!stream) return Buffer.alloc(0);
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > cap) {
      await reader.cancel().catch(() => undefined);
      return null;
    }
    chunks.push(value);
  }
  return Buffer.concat(chunks);
}

/**
 * Operator photo upload (findings G18 hardening):
 * 1. same-origin + authenticated only,
 * 2. declared `content-length` rejected BEFORE the body is read — and the
 *    body itself streamed under a hard 16 MB byte counter, so neither a
 *    lying header nor a chunked upload can spike memory,
 * 3. media-only policy: MIME allowlist AND magic-byte proof of the SAME
 *    image format — SVG/HTML/polyglot payloads can never be served from our
 *    origin,
 * 4. collision-safe storage: `crypto.randomUUID()` + the canonical extension
 *    of the sniffed format (user-controlled filenames never reach the disk),
 * 5. served via the read-only `/api/uploads/{filename}` route (strict
 *    filename allowlist, `nosniff`, immutable caching) — `public/` is frozen
 *    at `next build`, so runtime uploads must NOT rely on static serving.
 */
export async function POST(request: Request): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json(
      { error: "Cross-origin request rejected" },
      { status: 403 },
    );
  }
  const session = await verifyAdminSession();
  if (!session) {
    return Response.json({ error: "Not authenticated" }, { status: 401 });
  }

  const declaredLength = Number.parseInt(
    request.headers.get("content-length") ?? "",
    10,
  );
  if (Number.isFinite(declaredLength) && declaredLength > MAX_UPLOAD_BYTES) {
    return Response.json(
      { error: "Image file must be at most 16MB" },
      { status: 400 },
    );
  }

  const rawBody = await readBodyWithCap(request, MAX_UPLOAD_BYTES);
  if (rawBody === null) {
    return Response.json(
      { error: "Image file must be at most 16MB" },
      { status: 400 },
    );
  }

  const form = await new Response(new Uint8Array(rawBody), {
    headers: { "content-type": request.headers.get("content-type") ?? "" },
  })
    .formData()
    .catch(() => null);
  const file = form?.get("file");
  if (!file || !(file instanceof File)) {
    return Response.json({ error: "No image file provided" }, { status: 400 });
  }

  const bytes = Buffer.from(await file.arrayBuffer());
  const verdict = validateUploadImage({ declaredType: file.type, bytes });
  if (!verdict.ok) {
    return Response.json({ error: verdict.error }, { status: 400 });
  }

  const filename = `${randomUUID()}${verdict.ext}`;
  const uploadDir = path.join(process.cwd(), "public", "uploads");
  await fs.mkdir(uploadDir, { recursive: true });
  await fs.writeFile(path.join(uploadDir, filename), bytes);

  return Response.json({ ok: true, url: `/api/uploads/${filename}` });
}
