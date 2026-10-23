import { revalidatePath } from "next/cache";
import {
  adminFetch,
  verifyAdminSession,
  verifySameOriginRequest,
} from "@/lib/admin-auth";
import { validateSettingValue } from "@/lib/settings-validation";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ key: string }> },
): Promise<Response> {
  // G17: second-layer CSRF guard for the web tier's own mutation endpoints.
  if (!verifySameOriginRequest(request)) {
    return Response.json(
      { error: "Cross-origin request rejected" },
      { status: 403 },
    );
  }
  const session = await verifyAdminSession();
  if (!session) {
    return new Response(null, { status: 303, headers: { Location: "/admin/login" } });
  }
  const { key } = await params;
  const form = await request.formData();
  const action = String(form.get("action") ?? "save");
  const value = String(form.get("value") ?? "");

  if (action === "revert") {
    const { ok, status, data } = await adminFetch(
      `/api/admin/settings/${encodeURIComponent(key)}`,
      { method: "DELETE" },
    );
    revalidatePath("/", "layout");
    if (!ok && status !== 404) {
      const err = (data as { error?: string } | null)?.error ?? "Revert failed";
      return new Response(null, {
        status: 303,
        headers: { Location: `/admin/settings?error=${encodeURIComponent(err)}` },
      });
    }
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/settings?saved=${encodeURIComponent(`Reverted ${key} to default`)}`,
      },
    });
  }

  const effectiveVal = action === "off" ? "" : value;

  // G23: save-time format validation — a malformed GA Measurement ID,
  // WhatsApp number or site URL is rejected with an actionable error instead
  // of silently reaching the live storefront configuration.
  const verdict = validateSettingValue(key, effectiveVal);
  if (!verdict.ok) {
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/settings?error=${encodeURIComponent(verdict.error)}`,
      },
    });
  }

  const { ok, data } = await adminFetch(
    `/api/admin/settings/${encodeURIComponent(key)}`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ value: effectiveVal }),
    },
  );
  revalidatePath("/", "layout");

  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Could not save setting";
    return new Response(null, {
      status: 303,
      headers: { Location: `/admin/settings?error=${encodeURIComponent(err)}` },
    });
  }

  return new Response(null, {
    status: 303,
    headers: {
      Location: `/admin/settings?saved=${encodeURIComponent(`${key} updated live`)}`,
    },
  });
}
