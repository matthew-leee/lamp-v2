#!/usr/bin/env sh
# Exercised backup restore verification against an ephemeral scratch database (OPS-01).
set -eu

DUMP_FILE="${1:?Usage: restore-verify.sh <dump-file>}"
: "${VERIFY_POSTGRES_URL:?VERIFY_POSTGRES_URL is required (admin connection to scratch Postgres)}"

if [ -f "${DUMP_FILE}.sha256" ]; then
  sha256sum -c "${DUMP_FILE}.sha256"
fi

SCRATCH_DB="restore_verify_$$"
cleanup() {
  psql "${VERIFY_POSTGRES_URL}" -v ON_ERROR_STOP=1 -q -c "DROP DATABASE IF EXISTS ${SCRATCH_DB};" >/dev/null 2>&1 || true
}
trap cleanup EXIT

psql "${VERIFY_POSTGRES_URL}" -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE ${SCRATCH_DB};"

BASE_URL="${VERIFY_POSTGRES_URL%/*}"
TARGET_URL="${BASE_URL}/${SCRATCH_DB}"

pg_restore --no-owner --no-privileges --dbname="${TARGET_URL}" "${DUMP_FILE}"

# Verify all 10 authoritative schema tables exist after restore.
for table in \
  products \
  product_content \
  reference_prices \
  pieces \
  site_content \
  settings \
  audit_log \
  admin_users \
  orders \
  payment_events; do
  EXISTS="$(psql "${TARGET_URL}" -tAc "SELECT to_regclass('public.${table}') IS NOT NULL;")"
  if [ "${EXISTS}" != "t" ]; then
    echo "Restore verification failed: missing table public.${table}" >&2
    exit 1
  fi
done

# Verify stock invariant (stock_reserved + stock_quarantined <= stock_total) across all rows.
VIOLATIONS="$(psql "${TARGET_URL}" -tAc "SELECT COUNT(*) FROM products WHERE stock_reserved < 0 OR stock_quarantined < 0 OR stock_total < 0 OR stock_reserved + stock_quarantined > stock_total;")"
if [ "${VIOLATIONS}" != "0" ]; then
  echo "Restore verification failed: ${VIOLATIONS} product rows violate STOCK-01 invariant" >&2
  exit 1
fi

echo "Restore verification passed on ${SCRATCH_DB} (10 tables + STOCK-01 invariant verified)."
