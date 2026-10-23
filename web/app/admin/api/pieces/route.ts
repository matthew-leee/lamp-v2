import { revalidatePath } from "next/cache";
import {
  adminFetch,
  verifyAdminSession,
  verifySameOriginRequest,
} from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(request: Request): Promise<Response> {
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
  const form = await request.formData();
  const pieceNo = Number.parseInt(String(form.get("piece_no") ?? ""), 10);
  const productId = Number.parseInt(String(form.get("product_id") ?? ""), 10);

  const { ok, data } = await adminFetch("/api/admin/pieces", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ piece_no: pieceNo, product_id: productId }),
  });

  revalidatePath("/", "layout");
  const backUrl = `/admin/products/${productId}`;
  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Could not add piece";
    return new Response(null, {
      status: 303,
      headers: { Location: `${backUrl}?error=${encodeURIComponent(err)}` },
    });
  }
  return new Response(null, {
    status: 303,
    headers: { Location: `${backUrl}?saved=${encodeURIComponent(`Piece #${pieceNo} added`)}` },
  });
}
