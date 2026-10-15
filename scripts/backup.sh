#!/usr/bin/env sh
# Automated Postgres backup with SHA-256 checksum & optional Cloudflare R2 sync (OPS-01).
set -eu

: "${DATABASE_URL:?DATABASE_URL is required}"
BACKUP_DIR="${BACKUP_DIR:-./backups}"
TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DUMP_FILE="${BACKUP_DIR}/storefront_${TIMESTAMP}.dump"
CHECKSUM_FILE="${DUMP_FILE}.sha256"

mkdir -p "${BACKUP_DIR}"

pg_dump --format=custom --no-owner --no-privileges --file="${DUMP_FILE}" "${DATABASE_URL}"
sha256sum "${DUMP_FILE}" > "${CHECKSUM_FILE}"

if [ -n "${R2_BUCKET:-}" ] && [ -n "${R2_ENDPOINT_URL:-}" ]; then
  aws s3 cp "${DUMP_FILE}" "s3://${R2_BUCKET}/$(basename "${DUMP_FILE}")" --endpoint-url "${R2_ENDPOINT_URL}"
  aws s3 cp "${CHECKSUM_FILE}" "s3://${R2_BUCKET}/$(basename "${CHECKSUM_FILE}")" --endpoint-url "${R2_ENDPOINT_URL}"
fi

echo "Backup written: ${DUMP_FILE}"
