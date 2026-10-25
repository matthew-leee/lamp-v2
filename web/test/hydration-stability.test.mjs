/**
 * W2-C — Hydration stability contracts.
 *
 * React 19 diffs the WHOLE document on hydration, so anything that writes to the
 * root (`<html data-atmosphere>`, `--header-height`) or appends nodes to
 * `<body>` while Next is still hydrating a streamed route segment can surface as
 * a hydration-mismatch diagnostic against the `<html>` chain:
 *
 *   "A tree hydrated but some attributes of the server rendered HTML didn't
 *    match the client properties. This won't be patched up."
 *
 * The pixel/console-level proof is the real-browser Stage 3 audit
 * (`run-stage3.mjs`, 25/25 with zero console errors). The invariants here are
 * the ones that must never silently regress:
 *   1. `<html>` keeps the React-owned attributes deterministic
 *      (`lang`, `data-scroll-behavior`) and declares
 *      `suppressHydrationWarning` for the two values that client code owns.
 *   2. Exactly one code path owns each non-react root value:
 *      `StorefrontProvider` -> `data-atmosphere`, `ExhibitionHeader` ->
 *      `--header-height` measured from its INNER wrapper (never the header box,
 *      which would ratchet through `min-block-size`).
 *   3. The server-rendered atmosphere default matches the client's first render,
 *      so the only thing an effect can do is flip the visitor's persisted choice
 *      — never a server/client branch.
 *   4. The dev-only annotation toolbar (which appends a portal host, shadow
 *      styles and a textarea straight into `document.body`) is mounted only
 *      after `load` + an idle window, so its DOM writes can never interleave
 *      with hydration.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readSource(relativePath) {
  return fs.readFileSync(path.join(webRoot, relativePath), "utf8");
}

/** Locate the `<html ...>` opening tag of the locale layout. */
function localeLayoutHtmlTag() {
  const layout = readSource("app/[lang]/layout.tsx");
  const start = layout.indexOf("<html");
  assert.notEqual(start, -1, "locale layout must render the <html> element");
  const end = layout.indexOf(">", layout.indexOf('data-scroll-behavior', start));
  assert.notEqual(end, -1, "locale layout <html> tag must be closed");
  return { layout, tag: layout.slice(start, end + 1) };
}

test("locale layout: <html> declares React-owned attributes and the hydration opt-out (W2-C)", () => {
  const { tag } = localeLayoutHtmlTag();

  assert.match(
    tag,
    /lang=\{htmlLang\[locale\]\}/,
    "<html lang> must come from the locale map so SSR and client agree",
  );
  assert.match(
    tag,
    /data-scroll-behavior="smooth"/,
    "Next 15.5 requires data-scroll-behavior=smooth on <html> to own the " +
      "stylesheet's smooth scrolling (otherwise every route transition logs a " +
      "console warning and Next stops force-disabling it for us)",
  );
  assert.match(
    tag,
    /suppressHydrationWarning/,
    "<html> must keep suppressHydrationWarning for the two script-owned values " +
      "(data-atmosphere, --header-height)",
  );
  assert.match(
    tag,
    /data-atmosphere="night"/,
    "SSR atmosphere default must be the same value StorefrontProvider starts " +
      "with (night), so hydration never sees a server/client branch",
  );
});

test("locale layout: no browser branch in the document shell (W2-C)", () => {
  const layout = readSource("app/[lang]/layout.tsx");
  assert.doesNotMatch(
    layout,
    /typeof\s+window/,
    "the document shell must not branch on `typeof window` — that is the " +
      "classic source of a server/client attribute mismatch",
  );
  assert.doesNotMatch(
    layout,
    /Math\.random|Date\.now\(\)/,
    "the document shell must not render non-deterministic values",
  );
});

test("storefront provider: atmosphere state starts at the SSR default and is restored from one key (W2-C)", () => {
  const provider = readSource("components/storefront-context.tsx");

  assert.match(
    provider,
    /useState<boolean>\(true\)/,
    "`lit` must start `true` (night = the SSR attribute) so the first client " +
      "render matches the server HTML",
  );
  assert.match(
    provider,
    /const ATMOSPHERE_STORAGE_KEY = "lamp_atmosphere_lit_v2";/,
    "the persisted atmosphere key must stay single-sourced",
  );
  assert.match(
    provider,
    /document\.documentElement\.dataset\.atmosphere/,
    "the provider is the owner of data-atmosphere",
  );

  // Only effects/handlers may write to the root: a module-scope write would run
  // before hydration everywhere it is imported.
  const moduleScope = provider.slice(0, provider.indexOf("export function StorefrontProvider"));
  assert.doesNotMatch(
    moduleScope,
    /document\./,
    "no module-scope DOM writes — root attributes are set from effects/handlers",
  );
});

test("exhibition header: --header-height is measured from the inner wrapper, never the header box (W2-C)", () => {
  const header = readSource("components/exhibition-header.tsx");

  assert.match(
    header,
    /document\.documentElement\.style\.setProperty\(\s*"--header-height"/,
    "--header-height must be published on the document root",
  );
  assert.match(
    header,
    /inner\.getBoundingClientRect\(\)\.height/,
    "measurement must come from the inner content wrapper + border chrome",
  );
  assert.doesNotMatch(
    header,
    /header\.getBoundingClientRect\(/,
    "measuring the <header> itself feeds min-block-size: var(--header-height) " +
      "back into its own input and ratchets the header taller on every pass",
  );
  assert.match(
    header,
    /ResizeObserver/,
    "the measurement must be observer-driven (tiers, wrapping, locale, fonts)",
  );
  assert.match(
    header,
    /requestAnimationFrame/,
    "observers must coalesce bursts into one measurement per frame",
  );
});

test("dev annotation toolbar mounts only after load + idle, never during hydration (W2-C)", () => {
  const source = readSource("components/dev-agentation.tsx");

  // The toolbar renders only behind the `mounted` flag ...
  assert.match(
    source,
    /if \(process\.env\.NODE_ENV !== "development" \|\| !mounted\) \{\s*return null;\s*\}/,
    "Agentation must never render before the deferred mount flag flips",
  );
  // ... which is flipped after the window `load` event ...
  assert.match(
    source,
    /addEventListener\("load", onLoad, \{ once: true \}\)/,
    "the toolbar must wait for the window load event",
  );
  assert.match(
    source,
    /readyState === "complete"/,
    "an already-loaded document must still take the deferred path",
  );
  // ... and after an idle window on the main thread ...
  assert.match(
    source,
    /requestIdleCallback\(mount, \{ timeout: IDLE_TIMEOUT_MS \}\)/,
    "the toolbar must mount from a requestIdleCallback window",
  );
  assert.match(
    source,
    /setTimeout\(mount, IDLE_TIMEOUT_MS\)/,
    "engines without requestIdleCallback must fall back to a timeout",
  );
  // ... after the hydration commit has been painted.
  assert.match(
    source,
    /requestAnimationFrame/,
    "two animation frames guarantee the hydration commit is painted first",
  );
  assert.match(
    source,
    /cancelAnimationFrame|cancelIdleCallback|clearTimeout/,
    "deferred handles must be cleaned up on unmount",
  );

  // The <Agentation> element itself must appear after the mount gate.
  const gate = source.indexOf("!mounted");
  const render = source.indexOf("<Agentation");
  assert.ok(gate !== -1 && render > gate, "the toolbar JSX must render after the mount gate");
  assert.match(
    source,
    /data-agentation-portal/,
    "the deferral exists because the toolbar appends a portal host to <body> — " +
      "keep the reason documented with the component",
  );
});
