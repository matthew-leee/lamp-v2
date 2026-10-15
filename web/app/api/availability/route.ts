import { fetchLiveStockMap } from "@/lib/api";

export const dynamic = "force-dynamic";

export async function GET(): Promise<Response> {
  const stock = await fetchLiveStockMap();
  if (!stock) {
    return Response.json(
      { ok: false, error: "Live stock unavailable" },
      { status: 503, headers: { "Cache-Control": "no-store" } },
    );
  }
  return Response.json(
    { ok: true, stock },
    { status: 200, headers: { "Cache-Control": "no-store" } },
  );
}
