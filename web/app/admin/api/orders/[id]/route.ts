import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
): Promise<Response> {
  const session = await verifyAdminSession();
  if (!session) {
    return new Response(null, { status: 303, headers: { Location: "/admin/login" } });
  }
  const { id } = await params;
  const form = await request.formData();
  const action = String(form.get("action") ?? "");

  let endpoint = "";
  let body = "{}";

  if (action === "confirm_fps") {
    endpoint = `/api/admin/orders/${id}/confirm-fps`;
    body = JSON.stringify({
      fps_reference: String(form.get("fps_reference") ?? "").trim(),
    });
  } else if (action === "fulfil") {
    endpoint = `/api/admin/orders/${id}/fulfil`;
  } else if (action === "release") {
    endpoint = `/api/admin/orders/${id}/release`;
  } else if (action === "quarantine_return") {
    endpoint = `/api/admin/orders/${id}/quarantine-return`;
  } else {
    return new Response(null, {
      status: 303,
      headers: { Location: "/admin/orders?error=Unknown+order+action" },
    });
  }

  const { ok, data } = await adminFetch(endpoint, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });

  revalidatePath("/", "layout");

  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Order action failed";
    return new Response(null, {
      status: 303,
      headers: { Location: `/admin/orders?error=${encodeURIComponent(err)}` },
    });
  }

  return new Response(null, {
    status: 303,
    headers: {
      Location: `/admin/orders?saved=${encodeURIComponent(`Order #${id} (${action})`)}`,
    },
  });
}
