import fs from "node:fs/promises";
import path from "node:path";

export const dynamic = "force-dynamic";

/**
 * Read-only serving for operator photo uploads (findings G18).
 *
 * `/admin/api/upload` stores files under `public/uploads/`, but `public/` is
 * copied at `next build` time — under `next start` a runtime upload would
 * 404 on the static path. This route serves them IMMEDIATELY, with the
 * safety rules a static mount cannot express:
 *
 * - `filename` must match the exact upload naming contract
 *   (`{uuid36}.{jpg|png|webp|avif}`): no traversal, no user-named files,
 *   no extensions outside the media allowlist;
 * - the served `Content-Type` is derived from the ALLOWED extension (never
 *   from the file bytes or any client claim) plus `X-Content-Type-Options:
 *   nosniff`, so an uploaded payload can never be reinterpreted as HTML or
 *   script by the browser;
 * - immutable caching: content-addressed UUID names never change content.
 */

const UPLOAD_FILENAME_PATTERN = /^[0-9a-f-]{36}\.(jpg|png|webp|avif)$/;

const UPLOAD_CONTENT_TYPES: Record<string, string> = {
  jpg: "image/jpeg",
  png: "image/png",
  webp: "image/webp",
  avif: "image/avif",
};

const IMMUTABLE_CACHE = "public, max-age=31536000, immutable";

function notFound(): Response {
  return Response.json(
    { error: "file not found" },
    { status: 404, headers: { "Cache-Control": "no-store" } },
  );
}

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ filename: string }> },
): Promise<Response> {
  const { filename } = await params;
  if (!UPLOAD_FILENAME_PATTERN.test(filename)) {
    return notFound();
  }
  const ext = filename.slice(filename.lastIndexOf(".") + 1);
  const contentType = UPLOAD_CONTENT_TYPES[ext];
  if (!contentType) {
    return notFound();
  }

  // The pattern above already forbids separators/dots beyond the extension,
  // and `path.join` stays inside `public/uploads` by construction.
  const filePath = path.join(process.cwd(), "public", "uploads", filename);
  let bytes: Buffer;
  try {
    bytes = await fs.readFile(filePath);
  } catch {
    return notFound();
  }

  return new Response(new Uint8Array(bytes), {
    status: 200,
    headers: {
      "Content-Type": contentType,
      "X-Content-Type-Options": "nosniff",
      "Cache-Control": IMMUTABLE_CACHE,
    },
  });
}
