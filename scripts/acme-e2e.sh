#!/usr/bin/env bash
set -euo pipefail

: "${ACME_EMAIL:?set ACME_EMAIL}"
: "${ACME_DOMAIN:?set ACME_DOMAIN to the delegated zone apex}"
: "${HTTPREQ_ENDPOINT:?set HTTPREQ_ENDPOINT to the public HTTPS API}"
: "${HTTPREQ_USERNAME:?set HTTPREQ_USERNAME}"
: "${HTTPREQ_PASSWORD_FILE:?set HTTPREQ_PASSWORD_FILE}"
: "${AUTHORITATIVE_DNS:?set AUTHORITATIVE_DNS to the candidate DNS address}"

LEGO_BIN="${LEGO_BIN:-lego}"
LEGO_PATH="${LEGO_PATH:-$(mktemp -d)}"
LEGO_SERVER="${LEGO_SERVER:-letsencrypt-staging}"
RECURSIVE_DNS="${RECURSIVE_DNS:-1.1.1.1}"
challenge="_acme-challenge.${ACME_DOMAIN}"
certificate="${LEGO_PATH}/certificates/${ACME_DOMAIN}.crt"

install -d -m 0700 "$LEGO_PATH"
common=(
  run --server "$LEGO_SERVER" --path "$LEGO_PATH" --email "$ACME_EMAIL"
  --dns httpreq --domains "$ACME_DOMAIN" --domains "*.${ACME_DOMAIN}"
  --accept-tos
)

echo "Issuing a staging certificate into $LEGO_PATH"
"$LEGO_BIN" "${common[@]}"
test -s "$certificate"
openssl x509 -in "$certificate" -noout -subject -issuer -dates
first_hash="$(sha256sum "$certificate" | awk '{print $1}')"

echo "Repeating the run; this must be a no-op"
"$LEGO_BIN" "${common[@]}"
second_hash="$(sha256sum "$certificate" | awk '{print $1}')"
test "$first_hash" = "$second_hash"

echo "Forcing one staging renewal"
"$LEGO_BIN" "${common[@]}" --ari-disable --renew-days 999 --no-random-sleep
renewed_hash="$(sha256sum "$certificate" | awk '{print $1}')"
test "$renewed_hash" != "$second_hash"

echo "Checking public delegation and challenge cleanup"
dig @"$AUTHORITATIVE_DNS" "$ACME_DOMAIN" SOA +short | grep -q .
dig @"$AUTHORITATIVE_DNS" "$ACME_DOMAIN" SOA +tcp +short | grep -q .
dig @"$RECURSIVE_DNS" "$ACME_DOMAIN" NS +short | grep -q .
for _ in $(seq 1 30); do
  if test -z "$(dig @"$AUTHORITATIVE_DNS" "$challenge" TXT +short)" && \
     test -z "$(dig @"$RECURSIVE_DNS" "$challenge" TXT +short)"; then
    echo "ACME staging issue/no-op/renew/cleanup validation passed"
    exit 0
  fi
  sleep 2
done
echo "TXT values remained visible after lego cleanup" >&2
exit 1
