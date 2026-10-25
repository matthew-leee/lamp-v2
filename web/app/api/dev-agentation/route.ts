import { NextResponse } from "next/server";
import { promises as fs } from "node:fs";
import path from "node:path";

const DUMP_PATH = path.join(process.cwd(), "..", "artifacts", "agentation-live.json");

export async function GET() {
  try {
    const raw = await fs.readFile(DUMP_PATH, "utf8");
    return NextResponse.json(JSON.parse(raw));
  } catch {
    return NextResponse.json({ syncedAt: null, data: {} });
  }
}

export async function POST(req: Request) {
  if (process.env.NODE_ENV !== "development") {
    return NextResponse.json({ ok: false }, { status: 403 });
  }
  try {
    const body = await req.json();
    await fs.mkdir(path.dirname(DUMP_PATH), { recursive: true });
    await fs.writeFile(
      DUMP_PATH,
      JSON.stringify({ syncedAt: new Date().toISOString(), ...body }, null, 2),
      "utf8",
    );
    return NextResponse.json({ ok: true });
  } catch (err) {
    return NextResponse.json({ ok: false, error: String(err) }, { status: 500 });
  }
}
