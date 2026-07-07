#!/bin/bash
# ACME DNS-01 helper for safexip.
# Usage with lego:
#   lego --dns exec --domains '*.xip.example.com' --path ./.lego run \
#     ./safexip-acme-helper.sh
#
# Requires: SAFEXIP_API_KEY env var, SAFEXIP_API_URL env var (default: http://localhost:8080)

set -euo pipefail

API_URL="${SAFEXIP_API_URL:-http://localhost:8080}"
API_KEY="${SAFEXIP_API_KEY:-}"
: "${API_KEY:?SAFEXIP_API_KEY is required}"
: "${LEGO_DOMAIN:?must be run by lego --dns exec}"

ACME_NAME="_acme-challenge.${LEGO_DOMAIN}"

if [ "${LEGO_PRESENT:-}" = "true" ]; then
  : "${LEGO_TOKEN:?LEGO_TOKEN required for present}"
  curl -s -X POST "${API_URL}/v1/txt" \
    -H "Authorization: Bearer ${API_KEY}" \
    -H "Content-Type: application/json" \
    -d "$(cat <<EOF
{"name": "${ACME_NAME}", "value": "${LEGO_TOKEN}"}
EOF
)"
  echo "  present: ${ACME_NAME}"
fi

if [ "${LEGO_CLEANUP:-}" = "true" ]; then
  curl -s -X DELETE "${API_URL}/v1/txt?name=${ACME_NAME}" \
    -H "Authorization: Bearer ${API_KEY}"
  echo "  cleanup: ${ACME_NAME}"
fi
