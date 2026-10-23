# Findings: Payment / Email / Session Robustness — ELI10 Brief

- **Date:** 2026-10-21
- **Project:** lamp (v2 storefront + operator studio)
- **Source:** spec authoring audit (M2, reviewer `gpt-6.1-sol` round 1 findings `G01`,
  `G02`, `G17`, `G18`, `G23`) verified against `api/src/` and `web/` during the
  clean-room redesign program.
- **Status:** **ALL ITEMS RESOLVED & VERIFIED (2026-10-23)** across `api/`, `web/`, `ops/Caddyfile`, `docker-compose.yml`, and `scripts/vps.mjs` (264 Rust tests + 24 Web tests green).
- **⚠️ Export rule:** this file documents current-system behavior and must **never**
  ship to the clean-room redesign repository (it describes the existing
  implementation). The redesign session's requirements live in `spec/` + `spec/00`
  §0.9 only.

## TL;DR

| Area | Verdict |
|---|---|
| Stock & pricing integrity | ✅ Strong — fail-closed, race-proof, DB-authoritative prices |
| Payment verification | ✅ Strong — HMAC-signed settlement, thoughtful late-payment handling |
| Receipts & notifications | ⚠️ Fragile — fire-and-forget, one known silent-skip path |
| Admin session revocation | ⚠️ Weak — logout cannot cancel a key |
| Upload hygiene | ⚠️ Weak — needs media-only rules + safe naming |

None of these can lose money or oversell. All of them can lose **trust** — a paid
customer in silence, a held unit blocked by a ghost order, a stolen admin key.

## 1. Payment — mostly solid, two real gaps

**Solid:** stock is protected by database row locks — concurrent grabs of the last
unit resolve to exactly one winner (`409` for the loser); prices always come from the
database; settlement only counts on receipt of a mathematically signed gateway
message (`PAY-01`); a manual transfer landing after expiry routes to human review
(`late_payment_review`) instead of silently failing.

**Gap 1 — Ghost orders (no idempotency identity on reservation).** A lost response
followed by a retry (bad connection, second tab) can create **two orders**, each
holding stock. In a 16-unit store, ghost holds block real buyers. Nothing in the
system recognizes "this is the same pay attempt" (`G01`).
*Trace: `api/src/orders.rs` creates a fresh order per successful reservation; no
idempotency key in the request DTO.*

**Gap 2 — Paid… and silence (webhook-only settlement).** If the customer pays on the
gateway's hosted page and never returns to the site, the store learns "paid" via
webhook — but the webhook route knows no recipient, so **no paid-confirmation email
is sent**. A customer who just paid hears nothing (`G02`).
*Trace: `web/app/api/webhooks/stripe/route.ts` forwards + revalidates only.*

## 2. Messages (email) — the weakest area

Fire-and-forget: one send attempt, no tracking, no retry, no re-send tool.

- **No durable recovery.** If anything interrupts the sequence "payment committed →
  email attempted", the email is gone. No failed-email ledger, no admin "re-send
  receipt" (`G02`).
- **Live silent-skip:** the demo/test card settlement path relies on a
  `customerEmail` field the current checkout client never sends — the paid email is
  skipped without any error (`G02`).
- **Language is not remembered.** Orders do not store the customer's locale, so the
  admin FPS-confirmation email is always English (`G02`).
- **Duplicate guard is weak.** The "don't send twice" flag answers "did the gateway
  repeat this exact event?", not "did this customer already get their receipt?".
- **Single-transport rigidity.** Resend is preferred and does **not** fall back to
  SMTP on failure; Gmail App Password formatting needs normalization
  (`web/lib/email-receipt.ts`, `G15`).

## 3. Locks (admin sessions) — keys cannot be cancelled

- **Logout is browser-local.** It deletes the cookie; the stateless token itself
  stays valid until expiry (7 days). No revocation switch exists (`G17`,
  `api/src/admin.rs`).
- **One guard layer, not two.** The backend's `X-Requested-With` CSRF guard is
  satisfied by the web layer adding the header itself on the way in; the web layer's
  own mutation endpoints must carry their own request-origin protection (`G17`).
- **Session expiry mid-edit** can bounce a save and lose operator work (`G23`).

## 4. The photo-upload closet

- Accepts **any file content** (an HTML/SVG upload could be served from the store's
  own origin).
- Buffers the whole file **before** the size check (memory spike risk).
- Timestamp + original-name is not collision-safe — silent overwrite possible.
- Files land in the container filesystem; survival across redeploys depends on a
  persistent volume (`G18`, `web/app/admin/api/upload/route.ts`).

## 5. What the docs implied vs. what the code does (`G23`)

Resilient secondary-data loading, analytics rendering, reduced-motion handling,
confirmation on self-service cancellation, and save-time format validation of
settings are **target behaviors not yet implemented** in the current frontend. The
spec now labels them as targets (`spec/00` §0.9), not existing behavior.

## 6. Fix split (decision list for the founder)

| Fix | Layer | Scope |
|---|---|---|
| Paid-email recipient handling + "re-send receipt" in admin | Web (frontend server routes) | Inside redesign scope |
| Double-submit guard + "find my order" recovery | Web | Inside redesign scope |
| Upload media-only policy, safe naming, persistent volume | Web + ops | Inside redesign scope |
| Session-expiry work protection; honest logout wording | Web | Inside redesign scope |
| "One order per pay tap" idempotency key on reservation | **Backend** | Needs approved backend change |
| Email retry queue / failed-email ledger | **Backend** (or web store) | Needs approved backend change |
| Order-stored locale | **Backend** | Needs approved backend change |
| Server-side session revocation | **Backend** | Needs approved backend change |
| Persist uploads on a volume now; watch Resend/SMTP delivery | Ops | Do on live store regardless |

## Related

- Spec requirements: `spec/06` (email), `spec/12` (`FN-API-03/09/10/11`), `spec/13`
  (`FN-STATE-07/22`), `spec/00` §0.9 (limitations ledger).
- Review evidence: `spec/reviews/review-01-2026-10-21-gpt-6.1-sol.md`.
