import { revalidatePath } from "next/cache";
import { getApiBaseUrl } from "@/lib/api";

export const dynamic = "force-dynamic";

export async function POST(
  request: Request,
  { params }: { params: Promise<{ code: string }> },
): Promise<Response> {
  const { code } = await params;
  const body = await request.json().catch(() => ({}));
  const res = await fetch(
    `${getApiBaseUrl()}/api/orders/${encodeURIComponent(code)}/submit-fps`,
    {
      method: "POST",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
  );
  const data = await res.json().catch(() => ({}));
  if (res.ok) {
    revalidatePath("/", "layout");
  }
  return Response.json(data, { status: res.status });
}
