-- 0008_payment_email_session_hardening.sql — Payment / email / session
-- robustness hardening (findings doc 2026-10-21, backend half):
-- reservation idempotency identity, order-stored locale, a paid-email
-- delivery ledger and server-side admin session revocation.
--
-- Contracts this migration carries:
-- * "One order per pay tap" (G01): `orders.idempotency_key` is the client-
--   supplied identity of one pay attempt (request body `idempotency_key` or
--   the `Idempotency-Key` header). UNIQUE, so a lost-response retry can never
--   become a second ghost order holding stock: the second INSERT loses the
--   `orders_idempotency_key_key` race and the handler returns the existing
--   order instead. NULL keys stay legal (older clients send none, and NULLs
--   never collide in Postgres).
-- * Order-stored locale (G02): `orders.locale` remembers the customer's
--   storefront language ('en' / 'zh-HK' per LANG-01) so every later email
--   (admin FPS confirmation, receipts) can be sent in the language the buyer
--   actually used. NOT NULL with a CHECK so a typo can never silently fall
--   back to English; DEFAULT 'en' keeps pre-0008 rows valid.
-- * Paid-email delivery ledger (G02): `paid_email_sent_at` records the ONE
--   moment the paid-confirmation email actually landed with the mail gateway
--   (set via `POST /api/orders/{order_code}/email-delivery`, COALESCE'd so a
--   repeat can never move it) and `email_last_error` keeps the last failure
--   reason for the admin "re-send receipt" surface. Together they replace the
--   fire-and-forget "did we try?" flag with "did the customer get it?".
--   `paid_email_claimed_at` is the RECOVERABLE in-flight claim of that one
--   send (set by `claim: true`, cleared on outcome): claiming is a transient
--   "someone is sending now" lease, NEVER proof of delivery — only
--   `paid_email_sent_at` is. The lease expires after 2 minutes, so a web
--   tier that died mid-dispatch cannot block the receipt forever, and a
--   failed manual re-send can never wipe an already-confirmed
--   `paid_email_sent_at`.
-- * Server-side session revocation (G17): `revoked_admin_sessions` is the
--   deny-list of logged-out (or otherwise cancelled) admin session token
--   hashes. Logout stores SHA-256(token) — never the token itself — and
--   `require_admin` refuses any token whose hash is listed and unexpired.
--   Rows become irrelevant once the token's own `exp` passes (the signature
--   already rejects it), so the logout handler prunes `expires_at <= now()`.

-- ---------------------------------------------------------------------------
-- 1. Orders: idempotency identity, locale, paid-email ledger
-- ---------------------------------------------------------------------------

ALTER TABLE orders ADD COLUMN IF NOT EXISTS locale TEXT NOT NULL DEFAULT 'en' CHECK (locale IN ('en', 'zh-HK'));
ALTER TABLE orders ADD COLUMN IF NOT EXISTS idempotency_key TEXT UNIQUE;
ALTER TABLE orders ADD COLUMN IF NOT EXISTS paid_email_sent_at TIMESTAMPTZ;
ALTER TABLE orders ADD COLUMN IF NOT EXISTS paid_email_claimed_at TIMESTAMPTZ;
ALTER TABLE orders ADD COLUMN IF NOT EXISTS email_last_error TEXT NOT NULL DEFAULT '';

-- ---------------------------------------------------------------------------
-- 2. Admin session revocation deny-list
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS revoked_admin_sessions (
    token_hash TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_revoked_admin_sessions_expires_at
    ON revoked_admin_sessions (expires_at);
