import { revalidatePath } from "next/cache";
import {
  adminFetch,
  verifyAdminSession,
  verifySameOriginRequest,
} from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function PUT(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
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
    return Response.json({ error: "Session expired. Please sign in again." }, { status: 401 });
  }
  const { id } = await params;
  const body = await request.json().catch(() => null);
  if (!body) {
    return Response.json({ error: "Invalid JSON payload" }, { status: 400 });
  }

  const { status, ok, data } = await adminFetch(`/api/admin/products/${id}/full`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });

  if (ok) {
    revalidatePath("/", "layout");
  }
  return Response.json(data, { status });
}
