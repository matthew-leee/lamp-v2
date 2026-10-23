/**
 * Client-side reservation identity & recovery helpers (findings G01).
 *
 * - `getOrCreatePendingAttemptKey()` mints the identity of ONE pay attempt
 *   ("one order per pay tap") and DURABLY stores it in `localStorage` under a
 *   fingerprint of (cart items, payment method, customer email): a lost
 *   response followed by a refresh — or a retry from another TAB with the
 *   same cart + email — reuses the SAME key and the API answers with the
 *   FIRST order instead of a second stock-holding ghost. `clearPendingAttemptKey()`
 *   retires the key once the reservation succeeded, so the next pay tap is a
 *   new attempt.
 * - `saveRecentOrder` / `loadRecentOrder` remember the last created order in
 *   `localStorage` so `/{lang}/orders` can offer a one-click
 *   "Resume recent order" chip after a dropped connection.
 */

export const RECENT_ORDER_STORAGE_KEY = "lamp_recent_order_v2";

/** Durable pay-attempt identities, keyed by cart/email fingerprint. */
export const PENDING_ATTEMPT_STORAGE_KEY = "lamp_pending_attempt_v2";

/** Recent-order chips older than this are treated as stale and hidden. */
export const RECENT_ORDER_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * A pending pay attempt stays retryable for 30 minutes (a checkout session
 * plus its hold window), then its identity is pruned: a shopper who comes
 * back much later with the same cart is a NEW pay attempt.
 */
export const PENDING_ATTEMPT_MAX_AGE_MS = 30 * 60 * 1000;

export interface RecentOrder {
  order_code: string;
  created_at: string;
}

export function newIdempotencyKey(): string {
  try {
    const webCrypto = globalThis.crypto;
    if (webCrypto && typeof webCrypto.randomUUID === "function") {
      return `web_${webCrypto.randomUUID()}`;
    }
  } catch {
    // fall through to the non-crypto fallback
  }
  return `web_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 12)}`;
}

export function saveRecentOrder(orderCode: string, createdAt: string): void {
  try {
    if (typeof window === "undefined") return;
    const record: RecentOrder = {
      order_code: orderCode,
      created_at: createdAt || new Date().toISOString(),
    };
    window.localStorage.setItem(RECENT_ORDER_STORAGE_KEY, JSON.stringify(record));
  } catch {
    // storage disabled / full — recovery chip is a convenience, never a blocker
  }
}

export function loadRecentOrder(): RecentOrder | null {
  try {
    if (typeof window === "undefined") return null;
    const raw = window.localStorage.getItem(RECENT_ORDER_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<RecentOrder> | null;
    if (!parsed || typeof parsed.order_code !== "string" || !parsed.order_code) {
      return null;
    }
    const record: RecentOrder = {
      order_code: parsed.order_code,
      created_at:
        typeof parsed.created_at === "string" && parsed.created_at
          ? parsed.created_at
          : new Date(0).toISOString(),
    };
    const createdAtMs = Date.parse(record.created_at);
    if (
      Number.isFinite(createdAtMs) &&
      Date.now() - createdAtMs > RECENT_ORDER_MAX_AGE_MS
    ) {
      return null;
    }
    return record;
  } catch {
    return null;
  }
}

interface PendingAttempt {
  key: string;
  created_at: string;
}

/** Fingerprint of ONE pay attempt: the cart's lead SKU as a `${slug}:`
 * namespace prefix, then what the shopper is buying, how they pay and who
 * they are (email, case-folded). Two tabs with the same cart + email are the
 * SAME attempt and must share one `idempotency_key`; the sorted lead slug
 * prefix lets `hasPendingAttemptForSlug` find a recoverable attempt for a
 * product without touching the key store. */
export function pendingAttemptFingerprint(
  items: ReadonlyArray<{ slug: string; quantity: number }>,
  paymentMethod: string,
  customerEmail: string,
): string {
  const slugs = items.map((item) => item.slug).sort();
  const lines = items
    .map((item) => `${item.slug}x${item.quantity}`)
    .sort()
    .join(",");
  const leadSlug = slugs[0] ?? "";
  return `${leadSlug}:${lines}|${paymentMethod}|${customerEmail.trim().toLowerCase()}`;
}

/** Reads the durable map, dropping stale attempts so it can never grow
 * without bound or replay a key from a long-dead pay attempt. */
function readPendingAttempts(): Record<string, PendingAttempt> {
  const raw = window.localStorage.getItem(PENDING_ATTEMPT_STORAGE_KEY);
  if (!raw) return {};
  const parsed = JSON.parse(raw) as Record<string, Partial<PendingAttempt>>;
  const fresh: Record<string, PendingAttempt> = {};
  for (const [fingerprint, attempt] of Object.entries(parsed)) {
    if (!attempt || typeof attempt.key !== "string" || !attempt.key) continue;
    const createdAtMs =
      typeof attempt.created_at === "string" ? Date.parse(attempt.created_at) : NaN;
    if (
      !Number.isFinite(createdAtMs) ||
      Date.now() - createdAtMs > PENDING_ATTEMPT_MAX_AGE_MS
    ) {
      continue;
    }
    fresh[fingerprint] = {
      key: attempt.key,
      created_at: new Date(createdAtMs).toISOString(),
    };
  }
  return fresh;
}

/**
 * Returns the DURABLE identity of the pay attempt behind `fingerprint`,
 * minting and persisting one on first use. The same cart + payment method +
 * email therefore keeps its `idempotency_key` across a page refresh, a retry
 * after a lost response AND a second browser tab (findings G01 recovery).
 * Without usable storage it degrades to a one-shot in-memory key — a
 * convenience, never a blocker.
 */
export function getOrCreatePendingAttemptKey(fingerprint: string): string {
  try {
    if (typeof window === "undefined") return newIdempotencyKey();
    const attempts = readPendingAttempts();
    const existing = attempts[fingerprint];
    if (existing) return existing.key;
    const key = newIdempotencyKey();
    attempts[fingerprint] = { key, created_at: new Date().toISOString() };
    window.localStorage.setItem(PENDING_ATTEMPT_STORAGE_KEY, JSON.stringify(attempts));
    return key;
  } catch {
    return newIdempotencyKey();
  }
}

/** Retires the attempt identity once its reservation succeeded (or the
 * shopper abandoned it for good): the NEXT pay tap is a new order. */
export function clearPendingAttemptKey(fingerprint: string): void {
  try {
    if (typeof window === "undefined") return;
    const attempts = readPendingAttempts();
    if (!(fingerprint in attempts)) return;
    delete attempts[fingerprint];
    window.localStorage.setItem(PENDING_ATTEMPT_STORAGE_KEY, JSON.stringify(attempts));
  } catch {
    // storage disabled / full — recovery identity is a convenience, never a blocker
  }
}

/**
 * True when a NON-EXPIRED pay attempt for this product's `${slug}:` namespace
 * is still stored (findings G01 recovery). A lost response on the LAST unit
 * leaves the shopper's own order holding the stock — the page reads "sold
 * out" while their own recoverable attempt is still alive. Re-opening the
 * express checkout then replays the SAME `idempotency_key`, which the API
 * answers with the FIRST order: the one tap is recovered instead of being
 * blocked by its own ghost hold. Storage failures answer `false` — recovery
 * is an affordance, never a blocker.
 */
export function hasPendingAttemptForSlug(slug: string): boolean {
  try {
    if (typeof window === "undefined" || !slug) return false;
    const attempts = readPendingAttempts();
    return Object.keys(attempts).some((fingerprint) =>
      fingerprint.startsWith(`${slug}:`),
    );
  } catch {
    return false;
  }
}
