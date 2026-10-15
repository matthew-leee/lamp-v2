import { revalidatePath } from "next/cache";
import { getApiBaseUrl } from "@/lib/api";

export const dynamic = "force-dynamic";

export async function POST(
  _request: Request,
  { params }: { params: Promise<{ code: string }> },
): Promise<Response> {
  const { code } = await params;
  const res = await fetch(
    `${getApiBaseUrl()}/api/orders/${encodeURIComponent(code)}/cancel`,
    { method: "POST", cache: "no-store" },
  );
  const data = await res.json().catch(() => ({}));
  if (res.ok) {
    revalidatePath("/", "layout");
  }
  return Response.json(data, { status: res.status });
}
