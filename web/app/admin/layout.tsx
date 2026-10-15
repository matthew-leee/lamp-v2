import type { Metadata } from "next";
import type { ReactNode } from "react";
import "../globals.css";

export const metadata: Metadata = {
  title: { default: "Operator Studio", template: "%s · Operator Studio" },
  robots: { index: false, follow: false },
};

export default function AdminRootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" data-atmosphere="day">
      <body className="min-h-dvh bg-[var(--surface-page)] font-sans text-[var(--text-primary)] antialiased">
        {children}
      </body>
    </html>
  );
}
