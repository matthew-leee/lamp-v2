/**
 * Media-only upload policy for operator photo uploads (findings G18).
 *
 * Pure, dependency-free rules so both the `/admin/api/upload` route and the
 * unit test suite share ONE definition of "a real photo":
 * - declared MIME type must be in the image allowlist (SVG/HTML/etc. never),
 * - the file's MAGIC BYTES must prove the same image format (a `.png` name or
 *   `image/png` header is not proof — anything can claim that),
 * - canonical extension comes from the sniffed format, never from user input.
 */

export const MAX_UPLOAD_BYTES = 16 * 1024 * 1024;

export const ALLOWED_IMAGE_MIME_TYPES = [
  "image/jpeg",
  "image/png",
  "image/webp",
  "image/avif",
] as const;

export type UploadImageMime = (typeof ALLOWED_IMAGE_MIME_TYPES)[number];

export const CANONICAL_IMAGE_EXTENSIONS: Record<UploadImageMime, string> = {
  "image/jpeg": ".jpg",
  "image/png": ".png",
  "image/webp": ".webp",
  "image/avif": ".avif",
};

export type UploadImageVerdict =
  | { ok: true; mime: UploadImageMime; ext: string }
  | { ok: false; error: string };

function asciiAt(bytes: Uint8Array, offset: number, length: number): string {
  if (bytes.length < offset + length) return "";
  let out = "";
  for (let i = offset; i < offset + length; i += 1) {
    out += String.fromCharCode(bytes[i] as number);
  }
  return out;
}

/**
 * Identifies the image format from magic bytes only:
 * - JPEG: `FF D8 FF`
 * - PNG:  `89 50 4E 47 0D 0A 1A 0A` (the FULL 8-byte signature)
 * - WebP: `RIFF....WEBP`
 * - AVIF: `ftypavif` / `ftypavis` at offset 4 (generic `mif1`/`miaf` boxes
 *   are ISO-BMFF containers that may hold non-AVIF media — refused)
 */
export function sniffImageMime(bytes: Uint8Array): UploadImageMime | null {
  if (
    bytes.length >= 3 &&
    bytes[0] === 0xff &&
    bytes[1] === 0xd8 &&
    bytes[2] === 0xff
  ) {
    return "image/jpeg";
  }
  if (
    bytes.length >= 8 &&
    bytes[0] === 0x89 &&
    bytes[1] === 0x50 &&
    bytes[2] === 0x4e &&
    bytes[3] === 0x47 &&
    bytes[4] === 0x0d &&
    bytes[5] === 0x0a &&
    bytes[6] === 0x1a &&
    bytes[7] === 0x0a
  ) {
    return "image/png";
  }
  if (
    bytes.length >= 12 &&
    asciiAt(bytes, 0, 4) === "RIFF" &&
    asciiAt(bytes, 8, 4) === "WEBP"
  ) {
    return "image/webp";
  }
  if (bytes.length >= 12) {
    const box = asciiAt(bytes, 4, 8);
    if (box === "ftypavif" || box === "ftypavis") {
      return "image/avif";
    }
  }
  return null;
}

/**
 * Full upload verdict: allowlist + magic bytes + size. Never throws.
 */
export function validateUploadImage(input: {
  declaredType: string;
  bytes: Uint8Array;
}): UploadImageVerdict {
  const declared = input.declaredType.trim().toLowerCase();
  if (!(ALLOWED_IMAGE_MIME_TYPES as readonly string[]).includes(declared)) {
    return {
      ok: false,
      error:
        `Unsupported file type "${input.declaredType.trim() || "unknown"}" — only JPEG, PNG, WebP and AVIF photos are accepted`,
    };
  }
  if (input.bytes.length === 0) {
    return { ok: false, error: "Image file is empty" };
  }
  if (input.bytes.length > MAX_UPLOAD_BYTES) {
    return { ok: false, error: "Image file must be at most 16MB" };
  }
  const sniffed = sniffImageMime(input.bytes);
  if (!sniffed) {
    return {
      ok: false,
      error:
        "File content is not a real JPEG, PNG, WebP or AVIF image (magic bytes mismatch) — SVG, HTML and other payloads are rejected",
    };
  }
  if (sniffed !== declared) {
    return {
      ok: false,
      error: `File content (${sniffed}) does not match the declared type (${declared})`,
    };
  }
  return { ok: true, mime: sniffed, ext: CANONICAL_IMAGE_EXTENSIONS[sniffed] };
}
