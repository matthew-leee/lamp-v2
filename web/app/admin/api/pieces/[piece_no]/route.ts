import { revalidatePath } from "next/cache";
import {
  adminFetch,
  verifyAdminSession,
  verifySameOriginRequest,
} from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ piece_no: string }> },
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
  const { piece_no } = await params;
  const form = await request.formData();
  const action = String(form.get("action") ?? "reassign");
  const productId = Number.parseInt(String(form.get("product_id") ?? ""), 10);
  const returnProductId =
    Number.parseInt(String(form.get("return_product_id") ?? String(productId)), 10) || 1;

  if (action === "delete") {
    const { ok, data } = await adminFetch(`/api/admin/pieces/${piece_no}`, {
      method: "DELETE",
    });
    revalidatePath("/", "layout");
    if (!ok) {
      const err = (data as { error?: string } | null)?.error ?? "Delete failed";
      return new Response(null, {
        status: 303,
        headers: {
          Location: `/admin/products/${returnProductId}?error=${encodeURIComponent(err)}`,
        },
      });
    }
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/products/${returnProductId}?saved=${encodeURIComponent(`Deleted piece #${piece_no}`)}`,
      },
    });
  }

  const { ok, data } = await adminFetch(`/api/admin/pieces/${piece_no}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ product_id: productId }),
  });
  revalidatePath("/", "layout");
  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Reassign failed";
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/products/${returnProductId}?error=${encodeURIComponent(err)}`,
      },
    });
  }
  return new Response(null, {
    status: 303,
    headers: {
      Location: `/admin/products/${productId}?saved=${encodeURIComponent(`Reassigned piece #${piece_no}`)}`,
    },
  });
}
