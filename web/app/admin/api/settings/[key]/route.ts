import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ key: string }> },
): Promise<Response> {
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
