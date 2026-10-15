import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
): Promise<Response> {
  const session = await verifyAdminSession();
  if (!session) {
    return Response.json({ error: "Not authenticated" }, { status: 401 });
  }
  const { id } = await params;
  const body = await request.json().catch(() => ({}));
  const { status, ok, data } = await adminFetch(
    `/api/admin/products/${id}/inspect-quarantine`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
  );
  if (ok) {
    revalidatePath("/", "layout");
  }
  return Response.json(data, { status });
}
