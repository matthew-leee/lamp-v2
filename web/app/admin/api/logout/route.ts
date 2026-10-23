import { cookies } from "next/headers";
import {
  ADMIN_COOKIE_NAME,
  adminFetch,
  verifySameOriginRequest,
} from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

/**
 * Fail-closed logout destination (G17): server-side revocation could not be
 * confirmed (5xx / unreachable API), so the session must NOT be presented as
 * ended. The em-dash is percent-encoded (`%E2%80%94`) because a `Location`
 * header must be a ByteString — the admin console decodes it back to
 * "Logout failed—please retry".
 */
const LOGOUT_FAILED_LOCATION = "/admin?error=Logout+failed%E2%80%94please+retry";

export async function POST(request: Request): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json(
      { error: "Cross-origin request rejected" },
      { status: 403 },
    );
  }
  // G17: deleting the cookie alone is NOT logout — the bearer token stays
  // valid until expiry. Revoke it server-side FIRST (the Rust API deny-lists
  // the token hash), then clear the browser half.
  const revoke = await adminFetch("/api/admin/logout", { method: "POST" });
  // Fail closed: when the server-side revocation FAILED (5xx / unreachable),
  // clearing the cookie would leave a still-valid token in the wild while
  // the operator believes they logged out. Keep the session and say so.
  if (!revoke.ok && revoke.status >= 500) {
    return new Response(null, {
      status: 303,
      headers: { Location: LOGOUT_FAILED_LOCATION },
    });
  }
  const cookieStore = await cookies();
  cookieStore.set(ADMIN_COOKIE_NAME, "", {
    httpOnly: true,
    sameSite: "lax",
    path: "/",
    maxAge: 0,
  });
  return new Response(null, {
    status: 303,
    headers: { Location: "/admin/login" },
  });
}
