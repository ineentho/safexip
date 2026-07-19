# Delegated-zone ACME pre-release validation

Run this check once for every release candidate against a disposable or approved
publicly delegated safexip zone. It is intentionally not a pull-request gate:
the test depends on public DNS propagation, an HTTPS endpoint, CA availability,
and secret credentials.

## Prerequisites

- The candidate image is deployed with public authoritative UDP and TCP DNS.
- The parent zone delegates the test zone and public recursive resolvers see it.
- The safexip HTTP API is reachable through trusted HTTPS.
- `lego` 5, `dig`, `openssl`, and `sha256sum` are installed.
- A new empty directory is used for staging account and certificate state.

Keep the API key in a mode-0600 file. Never put production and staging ACME
accounts in the same lego directory.

## Run

```bash
install -d -m 0700 /tmp/safexip-lego-staging
install -m 0600 /dev/null /tmp/safexip-api-key
# Securely write only the API key to /tmp/safexip-api-key.

ACME_EMAIL=release-test@example.com \
ACME_DOMAIN=xip-test.example.com \
HTTPREQ_ENDPOINT=https://api.xip-test.example.com \
HTTPREQ_USERNAME=safexip \
HTTPREQ_PASSWORD_FILE=/tmp/safexip-api-key \
AUTHORITATIVE_DNS=203.0.113.10 \
LEGO_PATH=/tmp/safexip-lego-staging \
LEGO_SERVER=letsencrypt-staging \
RECURSIVE_DNS=1.1.1.1 \
./scripts/acme-e2e.sh | tee acme-e2e.log
```

The script performs and verifies:

1. an initial staging certificate for the apex and wildcard;
2. a second identical `lego run` that leaves the certificate unchanged;
3. a forced staging renewal that changes the certificate;
4. public-recursive visibility of the delegation;
5. removal of the challenge TXT RR after lego calls `/cleanup`.

Successful issuance and renewal exercise safexip authentication, `/present`,
authoritative and recursive DNS propagation, CA validation, `/cleanup`, and
local certificate storage. Preserve the log and certificate metadata with the
release checklist, but do not preserve private keys or the API key as evidence.

Delete the disposable credentials after recording the result:

```bash
rm -rf /tmp/safexip-lego-staging /tmp/safexip-api-key
```
