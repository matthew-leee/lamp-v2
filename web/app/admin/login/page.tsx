import type { Metadata } from "next";
import Link from "next/link";

export const metadata: Metadata = { title: "Sign In" };
export const dynamic = "force-dynamic";

export default async function AdminLoginPage({
  searchParams,
}: {
  searchParams: Promise<{ error?: string }>;
}) {
  const { error } = await searchParams;

  return (
    <main className="mx-auto flex min-h-dvh max-w-md flex-col justify-center px-6 py-12">
      <div className="card-exhibition p-8 sm:p-10">
        <p className="text-[11px] tracking-[0.24em] uppercase text-[var(--color-glow)]">
          OPERATOR STUDIO
        </p>
        <h1 className="mt-2 font-serif text-3xl">Admin Sign-In</h1>
        <p className="mt-2 text-xs text-[var(--text-secondary)]">
          Sign in to manage catalogue SKUs, numbered editions, orders, and live
          storefront settings.
        </p>

        {error && (
          <div
            role="alert"
            className="mt-5 border-l-4 border-[var(--color-error)] bg-[var(--surface-elevated)] p-3 text-xs text-[var(--color-error)] font-medium"
          >
            {error}
          </div>
        )}

        <form
          method="post"
          action="/admin/api/login"
          className="mt-6 flex flex-col gap-4"
        >
          <div>
            <label
              htmlFor="username"
              className="block text-xs text-[var(--text-secondary)]"
            >
              Username
            </label>
            <input
              id="username"
              name="username"
              type="text"
              autoComplete="username"
              required
              className="mt-1 w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 text-base sm:text-sm"
            />
          </div>
          <div>
            <label
              htmlFor="password"
              className="block text-xs text-[var(--text-secondary)]"
            >
              Password
            </label>
            <input
              id="password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              className="mt-1 w-full border border-[var(--border-hairline)] bg-[var(--surface-page)] px-3.5 py-2.5 text-base sm:text-sm"
            />
          </div>
          <button
            type="submit"
            className="mt-2 min-h-11 w-full bg-[var(--text-primary)] py-3 text-xs font-medium tracking-[0.2em] uppercase text-[var(--surface-page)]"
          >
            Sign In
          </button>
        </form>

        <div className="mt-6 border-t border-[var(--border-hairline)] pt-4 text-xs text-[var(--text-secondary)]">
          <Link href="/en" className="underline">
            ← Return to Public Storefront
          </Link>
        </div>
      </div>
    </main>
  );
}
