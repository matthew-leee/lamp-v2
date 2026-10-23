import { cookies } from "next/headers";
import { getApiBaseUrl } from "@/lib/api";
import { ADMIN_COOKIE_NAME, XRW_HEADER, XRW_VALUE, verifySameOriginRequest } from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(request: Request): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json(
      { error: "Cross-origin request rejected" },
      { status: 403 },
    );
  }
  const form = await request.formData().catch(() => null);
  const username = String(form?.get("username") ?? "").trim();
  const password = String(form?.get("password") ?? "");

  if (!username || !password) {
    return new Response(null, {
      status: 303,
      headers: { Location: "/admin/login?error=Username+and+password+are+required" },
    });
  }

  try {
    const res = await fetch(`${getApiBaseUrl()}/api/admin/login`, {
      method: "POST",
      cache: "no-store",
      headers: {
        "Content-Type": "application/json",
        [XRW_HEADER]: XRW_VALUE,
      },
      body: JSON.stringify({ username, password }),
    });

    if (!res.ok) {
      const data = (await res.json().catch(() => ({}))) as { error?: string };
      const msg = data.error ?? "Invalid credentials";
      return new Response(null, {
        status: 303,
        headers: { Location: `/admin/login?error=${encodeURIComponent(msg)}` },
      });
    }

    const setCookieHeader =
      res.headers.getSetCookie?.()?.[0] ?? res.headers.get("set-cookie") ?? "";
    const match = setCookieHeader.match(new RegExp(`${ADMIN_COOKIE_NAME}=([^;]+)`));
    const token = match?.[1];

    if (!token) {
      return new Response(null, {
        status: 303,
        headers: { Location: "/admin/login?error=No+session+token+received" },
      });
    }

    const cookieStore = await cookies();
    cookieStore.set(ADMIN_COOKIE_NAME, token, {
      httpOnly: true,
      sameSite: "lax",
      path: "/",
      maxAge: 7 * 24 * 60 * 60,
      secure: setCookieHeader.toLowerCase().includes("secure"),
    });

    return new Response(null, {
      status: 303,
      headers: { Location: "/admin" },
    });
  } catch {
    return new Response(null, {
      status: 303,
      headers: { Location: "/admin/login?error=Admin+API+unreachable" },
    });
  }
}
