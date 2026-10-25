"use client";

import { useEffect, useState } from "react";
import { Agentation } from "agentation";

function collectAllAgentationStorage(): Record<string, unknown> {
  if (typeof window === "undefined") return {};
  const out: Record<string, unknown> = {};
  try {
    for (let i = 0; i < window.localStorage.length; i++) {
      const k = window.localStorage.key(i);
      if (!k) continue;
      if (
        k.startsWith("feedback-") ||
        k.startsWith("agentation-") ||
        k.includes("annotation")
      ) {
        const raw = window.localStorage.getItem(k);
        if (raw) {
          try {
            out[k] = JSON.parse(raw);
          } catch {
            out[k] = raw;
          }
        }
      }
    }
  } catch {
    // ignore
  }
  return out;
}

async function syncToDevServer(trigger: string, extra?: unknown) {
  try {
    const storage = collectAllAgentationStorage();
    await fetch("/api/dev-agentation", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        trigger,
        pathname: typeof window !== "undefined" ? window.location.pathname : "",
        storage,
        extra,
      }),
    });
  } catch {
    // ignore
  }
}

/**
 * How long the toolbar waits for an idle window before mounting anyway, so a
 * permanently busy main thread can never swallow the dev tool.
 */
const IDLE_TIMEOUT_MS = 2000;

/**
 * Dev-only annotation toolbar (agentation).
 *
 * WHY IT IS MOUNTED LATE (hydration safety):
 * `Agentation` does not render inside React's tree — it appends its portal host
 * (`<div data-agentation-portal>`), its shadow `<style data-agentation-styles>`
 * and a helper `<textarea>` straight into `document.body` from mount effects.
 * React 19 diffs the *whole* document on hydration, and Next streams route
 * segments (`/products/[slug]`, `/orders`, ...) as separate Suspense
 * boundaries that hydrate AFTER the layout shell commits. A dev toolbar that
 * inserts nodes into `<body>` while a streamed segment is still hydrating makes
 * React meet DOM/attributes it did not render and emit a hydration-mismatch
 * diagnostic against the `<html>` chain ("A tree hydrated but some attributes
 * of the server rendered HTML didn't match the client properties").
 *
 * So the toolbar is only mounted once the document has finished loading AND the
 * main thread has gone idle: by then every segment's hydration has committed,
 * and the toolbar's DOM writes look like any other userland mutation. Two
 * animation frames after `load` also guarantee the commit has been painted.
 *
 * Production never renders this component (the early return below), so the
 * deferral costs production nothing.
 */
export default function DevAgentation() {
  const [mounted, setMounted] = useState(false);

  useEffect(() => {
    if (process.env.NODE_ENV !== "development") return;

    let cancelled = false;
    let frame: number | null = null;
    let idle: number | null = null;
    let timeout: ReturnType<typeof setTimeout> | null = null;

    const mount = () => {
      if (cancelled) return;
      setMounted(true);
      void syncToDevServer("mount");
    };

    const afterPaint = () => {
      frame = null;
      if (cancelled) return;
      // Second frame: the hydration commit has been painted, so nothing React
      // owns is pending a DOM write any more.
      frame = window.requestAnimationFrame(() => {
        frame = null;
        if (cancelled) return;
        if (typeof window.requestIdleCallback === "function") {
          idle = window.requestIdleCallback(mount, { timeout: IDLE_TIMEOUT_MS });
        } else {
          timeout = setTimeout(mount, IDLE_TIMEOUT_MS);
        }
      });
    };

    const onLoad = () => {
      frame = window.requestAnimationFrame(afterPaint);
    };

    if (document.readyState === "complete") {
      onLoad();
    } else {
      window.addEventListener("load", onLoad, { once: true });
    }

    return () => {
      cancelled = true;
      window.removeEventListener("load", onLoad);
      if (frame !== null) window.cancelAnimationFrame(frame);
      if (idle !== null && typeof window.cancelIdleCallback === "function") {
        window.cancelIdleCallback(idle);
      }
      if (timeout !== null) clearTimeout(timeout);
    };
  }, []);

  if (process.env.NODE_ENV !== "development" || !mounted) {
    return null;
  }

  return (
    <Agentation
      onAnnotationAdd={(a) => void syncToDevServer("add", a)}
      onAnnotationUpdate={(a) => void syncToDevServer("update", a)}
      onAnnotationDelete={(a) => void syncToDevServer("delete", a)}
      onAnnotationsClear={(a) => void syncToDevServer("clear", a)}
      onCopy={(md) => void syncToDevServer("copy", md)}
      onSubmit={(md, list) => void syncToDevServer("submit", { md, list })}
    />
  );
}
