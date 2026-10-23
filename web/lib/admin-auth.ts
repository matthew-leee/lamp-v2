import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { getApiBaseUrl } from "./api";

export const ADMIN_COOKIE_NAME = "admin_session";
export const XRW_HEADER = "X-Requested-With";
export const XRW_VALUE = "XMLHttpRequest";

/**
 * Second-layer CSRF guard for the web tier's own mutation endpoints
 * (findings G17): the backend's `X-Requested-With` guard is satisfied by
 * `adminFetch` on the way IN, so the web layer must carry its own
 * request-origin protection. Browsers attach `Origin` (or, for a few
 * same-origin form navigations, only `Referer`) to every state-changing
 * request; when such a header is present it MUST resolve to this request's
 * own public host (`x-forwarded-host` behind Caddy/Cloudflare, else `host`).
 * Non-browser clients (curl, server-to-server) send neither header and pass.
 */
export function verifySameOriginRequest(request: Request): boolean {
  const origin = request.headers.get("origin")?.trim() ?? "";
  const referer = request.headers.get("referer")?.trim() ?? "";
  const source = origin || referer;
  if (!source) return true;
  let sourceHost: string;
  try {
    sourceHost = new URL(source).host.toLowerCase();
  } catch {
    return false; // e.g. sandboxed `Origin: null` or a malformed header
  }
  if (!sourceHost) return false;
  const targetHosts = [
    request.headers.get("x-forwarded-host")?.split(",")[0]?.trim(),
    request.headers.get("host")?.split(",")[0]?.trim(),
  ]
    .filter((h): h is string => typeof h === "string" && h !== "")
    .map((h) => h.toLowerCase());
  if (targetHosts.length === 0) return false;
  return targetHosts.includes(sourceHost);
}

export async function getAdminCookie(): Promise<string | null> {
  const store = await cookies();
  return store.get(ADMIN_COOKIE_NAME)?.value ?? null;
}

export async function verifyAdminSession(): Promise<{ username: string } | null> {
  const token = await getAdminCookie();
  if (!token) return null;
  try {
    const res = await fetch(`${getApiBaseUrl()}/api/admin/whoami`, {
      cache: "no-store",
      headers: {
        Cookie: `${ADMIN_COOKIE_NAME}=${token}`,
      },
    });
    if (!res.ok) return null;
    const data = (await res.json()) as { username?: string };
    return typeof data.username === "string" ? { username: data.username } : null;
  } catch {
    return null;
  }
}

export async function requireAdmin(): Promise<{ username: string; token: string }> {
  const token = await getAdminCookie();
  if (!token) {
    redirect("/admin/login");
  }
  const session = await verifyAdminSession();
  if (!session) {
    redirect("/admin/login");
  }
  return { username: session.username, token };
}

export async function adminFetch(
  path: string,
  init: RequestInit = {},
): Promise<{ status: number; ok: boolean; data: unknown }> {
  const token = await getAdminCookie();
  const headers = new Headers(init.headers);
  if (token) {
    headers.set("Cookie", `${ADMIN_COOKIE_NAME}=${token}`);
  }
  if (init.method && !["GET", "HEAD", "OPTIONS"].includes(init.method.toUpperCase())) {
    headers.set(XRW_HEADER, XRW_VALUE);
  }
  try {
    const res = await fetch(`${getApiBaseUrl()}${path}`, {
      ...init,
      cache: "no-store",
      headers,
    });
    const data = await res.json().catch(() => null);
    return { status: res.status, ok: res.ok, data };
  } catch (err) {
    return {
      status: 503,
      ok: false,
      data: { error: err instanceof Error ? err.message : "Network error" },
    };
  }
}
