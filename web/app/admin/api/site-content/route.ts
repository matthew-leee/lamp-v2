import { revalidatePath } from "next/cache";
import { adminFetch, verifyAdminSession } from "@/lib/admin-auth";

export const dynamic = "force-dynamic";

export async function POST(request: Request): Promise<Response> {
  const session = await verifyAdminSession();
  if (!session) {
    return new Response(null, { status: 303, headers: { Location: "/admin/login" } });
  }
  const form = await request.formData();
  const action = String(form.get("action") ?? "save");
  const key = String(form.get("key") ?? "").trim();

  if (action === "save_pair") {
    const valEn = String(form.get("value_en") ?? "").trim();
    const valZh = String(form.get("value_zh") ?? "").trim();
    for (const [locale, value] of [
      ["en", valEn],
      ["zh-HK", valZh],
    ] as const) {
      if (value !== "") {
        const { ok, data } = await adminFetch("/api/admin/site-content", {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ key, locale, value }),
        });
        if (!ok) {
          const err = (data as { error?: string } | null)?.error ?? "Save failed";
          return new Response(null, {
            status: 303,
            headers: {
              Location: `/admin/site-content?error=${encodeURIComponent(err)}`,
            },
          });
        }
      }
    }
    revalidatePath("/", "layout");
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/site-content?saved=${encodeURIComponent(`${key} (EN + 繁體中文)`)}`,
      },
    });
  }

  const locale = String(form.get("locale") ?? "en").trim();
  const value = String(form.get("value") ?? "").trim();

  if (action === "delete") {
    const { ok, data } = await adminFetch(
      `/api/admin/site-content/${encodeURIComponent(key)}/${encodeURIComponent(locale)}`,
      { method: "DELETE" },
    );
    revalidatePath("/", "layout");
    if (!ok) {
      const err = (data as { error?: string } | null)?.error ?? "Delete failed";
      return new Response(null, {
        status: 303,
        headers: {
          Location: `/admin/site-content?error=${encodeURIComponent(err)}`,
        },
      });
    }
    return new Response(null, {
      status: 303,
      headers: {
        Location: `/admin/site-content?saved=${encodeURIComponent(`Deleted ${key} (${locale})`)}`,
      },
    });
  }

  const { ok, data } = await adminFetch("/api/admin/site-content", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ key, locale, value }),
  });
  revalidatePath("/", "layout");
  if (!ok) {
    const err = (data as { error?: string } | null)?.error ?? "Save failed";
    return new Response(null, {
      status: 303,
      headers: { Location: `/admin/site-content?error=${encodeURIComponent(err)}` },
    });
  }
  return new Response(null, {
    status: 303,
    headers: {
      Location: `/admin/site-content?saved=${encodeURIComponent(`${key} (${locale})`)}`,
    },
  });
}
