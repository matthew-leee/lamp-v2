/**
 * W1-D / W4-B — mobile, responsive & accessibility ergonomics contracts.
 *
 * These surfaces are pure Tailwind markup, so the automatable half of the
 * contract is the INVARIANT, not a frozen class string. Asserting a full
 * `className` copy would make the suite fail on every legitimate restyle while
 * still missing the regressions that actually hurt a phone user, so each check
 * below resolves a number or a semantic relationship instead:
 *   - iOS Safari only auto-zooms a `focus`ed field whose font-size is < 16px,
 *     so every text field is `text-base sm:text-sm` (16px phone / 14px desktop).
 *   - Touch targets declare a minimum BLOCK size (`min-h-*`) rather than relying
 *     on font size + padding, which collapses on a 320px viewport: >= 36px for
 *     compact controls, >= 44px for primary actions. Colours, wording and
 *     padding may change freely; shrinking a tap target may not.
 *   - Slide-over / modal surfaces close on Escape and clean the listener up.
 *   - Long ORD-… / FPS codes wrap (`break-all` + `sm:break-normal`) instead of
 *     forcing horizontal page overflow.
 *   - Reusable purchase / line-item panels size their internals from their own
 *     inline width (`@container`) instead of the viewport, and a
 *     `grid-rows-subgrid` card is NEVER a query container — `container-type`
 *     inline-size containment disqualifies an element from participating in a
 *     parent subgrid, so hosting both silently collapses cross-card alignment.
 *   - Wide operator tables scroll inside their own track, never the page.
 * The pixel-level result is verified in the real-browser UI audit pipeline; the
 * assertions here are the ones that must never silently regress.
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

const REM_PX = 16;
const SPACE_UNIT_PX = 4; // Tailwind's 0.25rem spacing scale

function classTokens(className) {
  return (className ?? "").split(/\s+/).filter(Boolean);
}

/** Pixels for one Tailwind spacing token (`9` -> 36, `3.5` -> 14, `[2.75rem]` -> 44). */
function spaceTokenPx(token) {
  const scale = /^(\d+(?:\.\d+)?)$/.exec(token);
  if (scale) return Number(scale[1]) * SPACE_UNIT_PX;
  const arbitrary = /^\[(\d+(?:\.\d+)?)(px|rem)\]$/.exec(token);
  if (!arbitrary) return null;
  return arbitrary[2] === "rem"
    ? Number(arbitrary[1]) * REM_PX
    : Number(arbitrary[1]);
}

/**
 * Effective `min-h-*` / `min-w-*` constraint of a class string, in pixels.
 * `min-h-full` / `min-h-fit` declare no pixel floor and are ignored; the
 * largest declared floor wins. `null` means the control declares none at all.
 */
function sizeFloorPx(className, axis = "block") {
  const prefix = axis === "inline" ? "min-w" : "min-h";
  let floor = null;
  for (const token of classTokens(className)) {
    if (!token.startsWith(`${prefix}-`)) continue;
    const px = spaceTokenPx(token.slice(prefix.length + 1));
    if (px === null) continue;
    floor = Math.max(floor ?? 0, px);
  }
  return floor;
}

/** Class tokens that apply at one breakpoint (`base` = no variant prefix). */
function tokensAtBreakpoint(className, breakpoint) {
  const prefix = breakpoint === "base" ? "" : `${breakpoint}:`;
  return classTokens(className)
    .filter((token) => (prefix ? token.startsWith(prefix) : !token.includes(":")))
    .map((token) => (prefix ? token.slice(prefix.length) : token));
}

/** Effective inline (horizontal) padding of a box at one breakpoint, in pixels. */
function inlinePaddingPx(className, breakpoint = "base") {
  const tokens = tokensAtBreakpoint(className, breakpoint);
  const lastPx = (pattern) => {
    const hit = tokens.filter((token) => pattern.test(token)).at(-1);
    return hit ? spaceTokenPx(hit.replace(pattern, "$1")) : null;
  };
  // `px-*` overrides `p-*` on the inline axis.
  return lastPx(/^px-(.+)$/) ?? lastPx(/^p-(.+)$/);
}

/**
 * Read one JSX opening tag starting at `start`. A naive `[^>]*>` scan breaks on
 * arrow-function props (`onChange={(e) => ...}`), so track JSX expression depth
 * and string state and stop at the first `>` outside both.
 */
function readOpeningTag(text, start) {
  let depth = 0;
  let quote = null;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (quote) {
      if (ch === quote && text[i - 1] !== "\\") quote = null;
      continue;
    }
    if (ch === '"' || ch === "'") {
      // Only a quote that opens a JSX attribute value matters here; inside a
      // `{...}` expression the quote handling above still applies.
      quote = ch;
      continue;
    }
    if (ch === "{") depth += 1;
    else if (ch === "}") depth = Math.max(0, depth - 1);
    else if (ch === ">" && depth === 0) return text.slice(start, i + 1);
  }
  throw new Error(`unterminated JSX tag at offset ${start}`);
}

/** Class string of an opening tag, literal or template-literal alike. */
function classNameOf(tag) {
  const doubleQuoted = tag.match(/className="([^"]*)"/);
  if (doubleQuoted) return doubleQuoted[1];
  const template = tag.match(/className=\{`([^`]*)`/);
  if (template) return template[1];
  const singleQuoted = tag.match(/className='([^']*)'/);
  if (singleQuoted) return singleQuoted[1];
  const expression = tag.match(/className=\{([^}]*)\}/);
  return expression ? expression[1] : "";
}

/**
 * Every element in a source file that carries a `className`, resolved to
 * `{ className, line, tag }`. Anchoring on `className=` avoids mistaking a
 * comparison (`a < b`) inside an expression for an element boundary.
 */
function elementTags(source) {
  const elements = [];
  const scan = /className=/g;
  let match;
  while ((match = scan.exec(source))) {
    const start = source.lastIndexOf("<", match.index);
    if (start === -1) continue;
    const tag = readOpeningTag(source, start);
    elements.push({
      tag,
      className: classNameOf(tag),
      line: source.slice(0, start).split("\n").length,
    });
  }
  return elements;
}

/** The element that owns the `className` immediately preceding `index`. */
function tagEndingBefore(source, index) {
  const classIndex = source.lastIndexOf("className=", index);
  assert.notEqual(
    classIndex,
    -1,
    "expected an enclosing element with an explicit className",
  );
  const start = source.lastIndexOf("<", classIndex);
  const tag = readOpeningTag(source, start);
  return {
    tag,
    className: classNameOf(tag),
    line: source.slice(0, start).split("\n").length,
  };
}

/** The element whose opening tag is the last one before `token`. */
function tagAtToken(source, token) {
  const at = source.indexOf(token);
  assert.notEqual(at, -1, `expected to find ${token} in the source`);
  return tagEndingBefore(source, at);
}

/** The element that starts with `tagToken` (e.g. `"<header"`). */
function tagAtStart(source, tagToken) {
  const at = source.indexOf(tagToken);
  assert.notEqual(at, -1, `expected to find ${tagToken} in the source`);
  const tag = readOpeningTag(source, at);
  return {
    tag,
    className: classNameOf(tag),
    line: source.slice(0, at).split("\n").length,
  };
}

/**
 * Every `<button>` opening tag with its class string, `aria-label` and raw JSX
 * body — so a control is identified by the accessible name / copy it renders
 * (`aria-label="Increase quantity"`, `t(ui.bagProceedCheckout, lang)`) rather
 * than by a frozen class list.
 */
function buttonTags(source) {
  const buttons = [];
  const scan = /<button\b/g;
  let match;
  while ((match = scan.exec(source))) {
    const tag = readOpeningTag(source, match.index);
    const bodyStart = match.index + tag.length;
    const bodyEnd = source.indexOf("</button>", bodyStart);
    buttons.push({
      tag,
      line: source.slice(0, match.index).split("\n").length,
      className: classNameOf(tag),
      ariaLabel: (tag.match(/aria-label="([^"]*)"/) ?? [])[1] ?? "",
      body: source.slice(bodyStart, bodyEnd === -1 ? bodyStart : bodyEnd),
    });
    scan.lastIndex = bodyStart;
  }
  return buttons;
}

/** A boxed control (bordered / filled / explicitly sized) as opposed to a bare text link. */
function isBoxedControl(className) {
  return classTokens(className).some(
    (token) =>
      token === "inline-flex" ||
      token.startsWith("border") ||
      token.startsWith("bg-[") ||
      token.startsWith("min-w-"),
  );
}

function assertBlockFloor(file, control, minPx, describe) {
  const floor = sizeFloorPx(control.className, "block");
  assert.ok(
    floor !== null,
    `${file}:${control.line} ${describe} must declare an explicit min-h-* block floor`,
  );
  assert.ok(
    floor >= minPx,
    `${file}:${control.line} ${describe} has a ${floor}px block floor (< ${minPx}px target)`,
  );
}

function findButtons(source, file, describe, predicate) {
  const found = buttonTags(source).filter(predicate);
  assert.ok(found.length > 0, `${file}: expected a ${describe} button`);
  return found;
}

function findButton(source, file, describe, predicate) {
  const found = findButtons(source, file, describe, predicate);
  assert.equal(
    found.length,
    1,
    `${file}: expected exactly one ${describe} button, found ${found.length}`,
  );
  return found[0];
}

/** Files that render customer / operator form fields on mobile. */
const FIELDSURFACES = [
  "components/product-story-client.tsx",
  "components/bag-drawer.tsx",
  "components/checkout-settlement-client.tsx",
  "app/[lang]/orders/page.tsx",
  "app/admin/login/page.tsx",
  "app/admin/(authed)/settings/page.tsx",
  "app/admin/(authed)/site-content/page.tsx",
  "app/admin/(authed)/products/page.tsx",
  "components/admin-product-studio-client.tsx",
  "components/admin-csv-sync-client.tsx",
];

/**
 * Pull every `<input>`, `<select>` and `<textarea>` opening tag, resolving the
 * studio's shared `inputClass` constant so a single class string is checked
 * once instead of being re-inlined at 30 call sites.
 */
function formFieldTags(source, { resolveInputClass = false } = {}) {
  let text = source;
  if (resolveInputClass) {
    const literal = text.match(/const inputClass =\s*\n?\s*"([^"]+)"/);
    assert.ok(literal, "admin studio must declare an inputClass literal");
    text = text
      .replace(/className=\{inputClass\}/g, `className="${literal[1]}"`)
      .replace(/\$\{inputClass\}/g, literal[1]);
  }
  const tags = [];
  const tagStart = /<(input|select|textarea)\b/g;
  let match;
  while ((match = tagStart.exec(text))) {
    const tag = readOpeningTag(text, match.index);
    tags.push({
      kind: match[1],
      tag,
      line: text.slice(0, match.index).split("\n").length,
      className: classNameOf(tag),
    });
    tagStart.lastIndex = match.index + tag.length;
  }
  return tags;
}

/** `text-xs` / `text-sm` tokens minus the `file:` pseudo-element sizing. */
function fontSizeTokens(className) {
  return className
    .split(/\s+/)
    .filter(Boolean)
    .filter((token) => !token.startsWith("file:"))
    .filter((token) => /^(sm:)?text-(xs|sm|base|lg)/.test(token));
}

test("iOS Safari 16px rule: no mobile form field renders below 16px on focus (W1-D)", () => {
  for (const file of FIELDSURFACES) {
    const tags = formFieldTags(readSource(file), {
      resolveInputClass:
        file.includes("admin-product-studio-client") ||
        file.includes("admin/(authed)/products/page.tsx"),
    });
    assert.ok(tags.length > 0, `${file} must render form fields`);
    for (const { kind, className, line, tag } of tags) {
      const sizes = fontSizeTokens(className);
      const isTextLike = !/type="(checkbox|radio|range|hidden|file)"/.test(tag);
      if (!isTextLike) continue;
      // A text field with no size class inherits the parent font size, which is
      // never the intent for an input — require the explicit pairing.
      assert.ok(
        sizes.includes("text-base"),
        `${file}:${line} <${kind}> must be text-base on phones (found: ${sizes.join(", ") || "none"})`,
      );
      assert.ok(
        sizes.includes("sm:text-sm"),
        `${file}:${line} <${kind}> must settle at text-sm from sm: up (found: ${sizes.join(", ") || "none"})`,
      );
      assert.ok(
        !sizes.includes("text-xs") && !sizes.includes("text-sm"),
        `${file}:${line} <${kind}> must not use a bare text-xs/text-sm (iOS auto-zoom)`,
      );
    }
  }
});

test("PDP: the purchase panel measures itself and its controls are 44px targets (W4-B)", () => {
  const file = "components/product-story-client.tsx";
  const source = readSource(file);

  // The panel is a query container so the stepper + Add to Cart row switches
  // from stacked to inline from ITS OWN inline width, not a viewport breakpoint.
  const panelMarker = source.indexOf("{/* Purchase & Instant Checkout Controls */}");
  assert.notEqual(panelMarker, -1, "the PDP must keep its purchase control panel");
  const panelStart = source.indexOf("<div", panelMarker);
  const panel = classNameOf(readOpeningTag(source, panelStart));
  assert.ok(
    classTokens(panel).includes("@container"),
    `${file}: the purchase panel must declare @container`,
  );
  const panelBody = source.slice(
    panelMarker,
    source.indexOf("Inline Express Checkout Panel", panelMarker),
  );
  assert.match(
    panelBody,
    /@\[[\d.]+rem\]:flex-row/,
    "the quantity + Add to Cart row must switch to a row from its own container width",
  );

  // 44px floors for every purchase control (steppers on both axes).
  for (const label of ["Decrease quantity", "Increase quantity"]) {
    const stepper = findButton(source, file, `"${label}" stepper`, (b) => b.ariaLabel === label);
    assertBlockFloor(file, stepper, 44, `"${label}" stepper`);
    assert.ok(
      sizeFloorPx(stepper.className, "inline") >= 44,
      `${file}:${stepper.line} "${label}" stepper must also be >= 44px wide`,
    );
  }
  const addToBag = findButton(source, file, "Add to Cart", (b) => b.body.includes("ui.addToBag"));
  assertBlockFloor(file, addToBag, 44, "Add to Cart");
  const buyNow = findButton(source, file, "Buy Now", (b) =>
    b.body.includes("formatMoney(product.price_cents * quantity"),
  );
  assertBlockFloor(file, buyNow, 44, "Buy Now");
});

test("PDP: the light-studio switcher stacks on tiny phones and keeps 40px rows (W1-D)", () => {
  const file = "components/product-story-client.tsx";
  const source = readSource(file);
  const switcher = source.slice(
    source.indexOf("Light Studio Switcher"),
    source.indexOf("{/* Purchase & Instant Checkout Controls */}"),
  );

  // Both layouts (3-mode / 2-mode) start as ONE column and may only upgrade at
  // an explicit sub-`sm` pixel breakpoint, so a 320px phone never shows a
  // squeezed 3-up strip.
  const branches = [...switcher.matchAll(/"grid-cols-1 ([^"]+)"/g)].map((m) => m[1]);
  assert.equal(branches.length, 2, "both switcher layouts must start stacked");
  for (const branch of branches) {
    assert.match(
      branch,
      /^min-\[\d+px\]:grid-cols-[23]$/,
      `switcher may only upgrade to 2/3 columns at a min-[..px] breakpoint (found: ${branch})`,
    );
  }

  const modes = findButtons(switcher, file, "light-studio mode", (b) =>
    b.body.includes("IntrinsicBilingualSpan"),
  );
  assert.equal(modes.length, 3, "all three stage-mode buttons must exist in source");
  for (const mode of modes) assertBlockFloor(file, mode, 40, "light-studio mode button");
});

test("bag drawer: Escape closes the slide-over and hook order survives the early return (W1-D)", () => {
  const source = readSource("components/bag-drawer.tsx");
  const escapeHandler = source.match(
    /useEffect\(\(\) => \{\s*\n\s*if \(!bagOpen\) return;\s*\n\s*const handleKeyDown = \(event: KeyboardEvent\) => \{\s*\n\s*if \(event\.key === "Escape"\) setBagOpen\(false\);/,
  );
  assert.ok(escapeHandler, "bag drawer must close on Escape while open");
  assert.match(
    source,
    /return \(\) => window\.removeEventListener\("keydown", handleKeyDown\);/,
    "the Escape listener must be removed on cleanup",
  );
  const earlyReturn = source.indexOf("if (!bagOpen) return null;");
  const hook = source.indexOf('if (event.key === "Escape") setBagOpen(false);');
  assert.ok(
    earlyReturn > hook,
    "the escape effect must be registered before the `!bagOpen` early return (rules of hooks)",
  );
});

test("bag drawer: no boxed control is smaller than 36px and drawer CTAs are 44px (W4-B)", () => {
  const file = "components/bag-drawer.tsx";
  const source = readSource(file);

  // Sweep: nothing bordered / filled / explicitly sized in the drawer — close,
  // discovery CTA, Remove, both steppers, both footer CTAs — may fall below the
  // 36px compact-control minimum.
  const boxed = buttonTags(source).filter((b) => isBoxedControl(b.className));
  assert.ok(boxed.length >= 7, `expected every drawer control, found ${boxed.length}`);
  for (const control of boxed) {
    assertBlockFloor(file, control, 36, "boxed drawer control");
  }

  // The line-item Remove control is compact (36px) but must never drop back to
  // the 32px it was regressed to, and needs inline padding for its label.
  const remove = findButton(source, file, "line-item Remove", (b) => /"(?:Remove|移除)"/.test(b.body));
  assertBlockFloor(file, remove, 36, "Remove");
  const removeTokens = classTokens(remove.className);
  assert.ok(
    removeTokens.some((token) => /^px-\d/.test(token)),
    `${file}:${remove.line} Remove needs horizontal padding so its label is not flush`,
  );

  // Cart quantity steppers are 36x36 targets.
  for (const label of ["Decrease quantity", "Increase quantity"]) {
    const stepper = findButton(source, file, `cart "${label}" stepper`, (b) => b.ariaLabel === label);
    assertBlockFloor(file, stepper, 36, `cart "${label}" stepper`);
    assert.ok(
      sizeFloorPx(stepper.className, "inline") >= 36,
      `${file}:${stepper.line} cart "${label}" stepper must also be >= 36px wide`,
    );
  }

  // Primary actions are 44px: the empty-bag discovery CTA, Proceed to checkout
  // and Lock Piece & Continue.
  const discovery = findButton(source, file, "empty-bag discovery CTA", (b) =>
    b.body.includes("ui.addToBag"),
  );
  assertBlockFloor(file, discovery, 44, "empty-bag discovery CTA");
  const proceed = findButton(source, file, "Proceed to checkout", (b) =>
    b.body.includes("ui.bagProceedCheckout"),
  );
  assertBlockFloor(file, proceed, 44, "Proceed to checkout");
  const lock = findButton(source, file, "Lock Piece & Continue", (b) =>
    b.body.includes("Lock Piece & Continue to Payment"),
  );
  assertBlockFloor(file, lock, 44, "Lock Piece & Continue");

  // The close button is a compact chrome control (40px), never a bare glyph.
  const close = findButton(source, file, "drawer Close", (b) => /"(?:Close|關閉)"/.test(b.body));
  assertBlockFloor(file, close, 40, "drawer Close");
});

test("bag drawer: line items are self-measuring containers, subgrid cards are not (W4-B)", () => {
  const source = readSource("components/bag-drawer.tsx");

  // The drawer panel is the container for the empty-bag discovery grid.
  const panel = elementTags(source).find(({ className }) => {
    const tokens = classTokens(className);
    return tokens.includes("@container") && tokens.includes("max-w-lg") && tokens.includes("h-full");
  });
  assert.ok(
    panel,
    "the drawer panel must stay a query container (safe-area + @[22rem] grid)",
  );

  // Each populated cart line item is its own container.
  const lineItems = elementTags(source).filter(({ className }) => {
    const tokens = classTokens(className);
    return tokens.includes("@container") && tokens.includes("gap-4") && tokens.includes("p-4");
  });
  assert.equal(lineItems.length, 1, "each populated cart line item must be an @container");
  const lineItem = lineItems[0];
  assert.ok(
    !classTokens(lineItem.className).some((token) => token.endsWith("grid-rows-subgrid")),
    "a line item may not be an @container AND participate in a parent subgrid",
  );

  // The container must actually be queried — an `@container` with no
  // `@[..]` variant inside it is dead configuration.
  const populated = source.slice(
    source.indexOf("populatedItems.map"),
    source.indexOf("Step 2 inside Drawer"),
  );
  assert.match(
    populated,
    /@\[\d+(?:\.\d+)?rem\]:/,
    "the line-item card must query its own width (stepper + line total row)",
  );

  // Empty-bag discovery cards align their rows across the grid via `subgrid`,
  // which inline-size containment would silently break.
  const subgridCards = elementTags(source).filter(({ className }) =>
    classTokens(className).some((token) => token.endsWith("grid-rows-subgrid")),
  );
  assert.ok(subgridCards.length > 0, "empty-bag cards must align rows via subgrid");
  for (const card of subgridCards) {
    assert.ok(
      !card.className.includes("@container") && !card.className.includes("container-type"),
      `bag-drawer.tsx:${card.line} subgrid card must not be a query container`,
    );
  }
});

test("@container and CSS subgrid never share an element (W4-B)", () => {
  // `container-type: inline-size` makes the element its own layout constraint
  // box, which disqualifies it from a parent `grid-template-rows: subgrid`.
  const files = [
    "components/bag-drawer.tsx",
    "components/product-story-client.tsx",
    "components/checkout-settlement-client.tsx",
    "components/exhibition-home-client.tsx",
  ];
  for (const file of files) {
    for (const { className, line } of elementTags(readSource(file))) {
      const isSubgrid = classTokens(className).some((token) =>
        token.endsWith("grid-rows-subgrid"),
      );
      if (!isSubgrid) continue;
      assert.ok(
        !className.includes("@container") && !className.includes("container-type"),
        `${file}:${line} cannot host @container and grid-rows-subgrid at once`,
      );
    }
  }
});

test("checkout: the eyebrow falls back to the localized title, the FPS payee never does (W4-B/NAME-01)", () => {
  const file = "components/checkout-settlement-client.tsx";
  const source = readSource(file);

  // The payee resolution stays contract-honest: `fpsAccountName ?? brandName`,
  // with no display placeholder ever back-filling the raw brand.
  assert.match(
    source,
    /const displayFpsAccount =\s*\n?\s*fpsAccountName \?\? brandName;/,
    "the FPS payee must stay `fpsAccountName ?? brandName` (NAME-01)",
  );
  assert.match(
    source,
    /const displayBrandTitle =\s*\n?\s*brandName\.trim\(\) !== "" \? brandName : t\(ui\.defaultExhibitionTitle, lang\);/,
    "an unconfigured brand must resolve the localized exhibition title at render time",
  );

  // The visual eyebrow renders the fallback, never the raw (possibly empty) brand.
  const at = source.indexOf("HONG KONG VAULT");
  assert.notEqual(at, -1, "the settlement eyebrow must keep its vault suffix");
  const open = source.lastIndexOf("<p", at);
  const content = source.slice(source.indexOf(">", open) + 1, source.indexOf("</p>", open));
  assert.ok(
    !content.includes("{brandName}"),
    "the eyebrow must not interpolate the raw brand (renders \" · HONG KONG VAULT\")",
  );
  assert.ok(
    content.includes("{displayBrandTitle}"),
    "the eyebrow must interpolate the display fallback title",
  );
});

test("long order / FPS codes wrap at 320px instead of widening the page (W1-D)", () => {
  const files = [
    "components/checkout-settlement-client.tsx",
    "app/[lang]/orders/page.tsx",
  ];
  for (const file of files) {
    const wrappers = elementTags(readSource(file)).filter(({ className }) =>
      classTokens(className).includes("break-all"),
    );
    assert.ok(wrappers.length > 0, `${file} must wrap at least one long identifier`);
    for (const { className, line } of wrappers) {
      assert.ok(
        classTokens(className).includes("sm:break-normal"),
        `${file}:${line} break-all must pair with sm:break-normal so desktop codes stay unbroken`,
      );
    }
  }
});

test("checkout settlement: primary actions are 44px and the email modal closes on Escape (W1-D)", () => {
  const file = "components/checkout-settlement-client.tsx";
  const source = readSource(file);

  const primaries = findButtons(source, file, "settlement primary", (b) =>
    classTokens(b.className).includes("bg-[var(--color-glow)]"),
  );
  assert.ok(primaries.length >= 3, `expected the settlement CTAs, found ${primaries.length}`);
  for (const primary of primaries) {
    assertBlockFloor(file, primary, 44, "settlement primary action");
  }

  // The expired/cancelled recovery action is the same 44px target class as a CTA.
  const recovery = tagAtToken(source, "Reserve Piece Again");
  assertBlockFloor(file, recovery, 44, "Reserve Piece Again recovery action");

  // Escape closes the email-receipt modal.
  assert.match(
    source,
    /if \(!showEmailModal\) return;\s*\n\s*const handleKeyDown = \(event: KeyboardEvent\) => \{\s*\n\s*if \(event\.key === "Escape"\) setShowEmailModal\(false\);/,
    "the email preview modal must close on Escape",
  );
  assert.match(
    source,
    /return \(\) => window\.removeEventListener\("keydown", handleKeyDown\);/,
    "the modal key listener must be cleaned up",
  );
});

test("order lookup: mobile-safe card padding and 44px actions (W1-D)", () => {
  const file = "app/[lang]/orders/page.tsx";
  const source = readSource(file);

  const card = tagAtToken(source, "card-exhibition mt-8");
  const mobile = inlinePaddingPx(card.className, "base");
  const desktop = inlinePaddingPx(card.className, "sm");
  assert.ok(mobile >= 20, `${file}:${card.line} lookup card needs >= 20px inline padding on phones (found ${mobile})`);
  assert.ok(desktop >= 40, `${file}:${card.line} lookup card needs >= 40px inline padding from sm: (found ${desktop})`);

  const actions = buttonTags(source).filter((b) => isBoxedControl(b.className));
  assert.ok(actions.length >= 2, "the resume-recent and Open Order actions must exist");
  for (const action of actions) assertBlockFloor(file, action, 44, "order-lookup action");
});

test("admin studio: every wide table scrolls in its own track (W1-D)", () => {
  const file = "components/admin-product-studio-client.tsx";
  const source = readSource(file);
  let found = 0;
  for (let at = source.indexOf("<table"); at !== -1; at = source.indexOf("<table", at + 1)) {
    const wrapper = tagEndingBefore(source, at);
    const tokens = classTokens(wrapper.className);
    assert.ok(
      tokens.includes("scroll-contain") && tokens.includes("overflow-x-auto"),
      `${file}:${wrapper.line} table must sit inside a scroll-contain overflow-x-auto wrapper`,
    );
    found += 1;
  }
  assert.ok(found > 0, `${file} must render the numbered-pieces table`);
});

test("admin layout: header and main keep mobile-safe inline padding (W1-D)", () => {
  const file = "app/admin/(authed)/layout.tsx";
  const source = readSource(file);
  for (const [label, token] of [
    ["header", "<header"],
    ["main", "<main"],
  ]) {
    const element = tagAtStart(source, token);
    const mobile = inlinePaddingPx(element.className, "base");
    const desktop = inlinePaddingPx(element.className, "sm");
    assert.ok(
      mobile >= 16,
      `${file}:${element.line} ${label} needs >= 16px inline padding on phones (found ${mobile})`,
    );
    assert.ok(
      desktop >= 24,
      `${file}:${element.line} ${label} needs >= 24px inline padding from sm: (found ${desktop})`,
    );
  }
});
