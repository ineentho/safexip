# safexip

Self-hosted xip-style DNS server with an ACME DNS-01 HTTP API for wildcard TLS certificates. Private keys never leave your machine — safexip only holds the short-lived challenge tokens.

## How it works

1. Delegate a subdomain (e.g. `xip.example.com`) to safexip via NS records in your parent zone.
2. safexip resolves xip-style hostnames: `127-0-0-1.xip.example.com` → `127.0.0.1`.
3. During issuance, lego POSTs the ACME challenge token to safexip, which serves it as a TXT record at `_acme-challenge.xip.example.com`.
4. Let's Encrypt validates the challenge; the cert (and its private key) are written locally by lego.

## Quick start

### Start the server

```bash
SAFEXIP_API_KEY=$(openssl rand -hex 32) \
SAFEXIP_DOMAIN=xip.example.com \
SAFEXIP_NS_HOSTNAME=ns1.xip.example.com \
SAFEXIP_NS_IP=1.2.3.4 \
safexip
```

In your parent zone, delegate the subdomain (glue A records must live in the parent):

```
xip.example.com.     NS ns1.xip.example.com.
xip.example.com.     NS ns2.xip.example.com.
ns1.xip.example.com. A  1.2.3.4
ns2.xip.example.com. A  1.2.3.4
```

### Generate a wildcard certificate

safexip speaks lego's `httpreq` DNS provider directly, so no helper script is needed:

```bash
export HTTPREQ_ENDPOINT=http://1.2.3.4:8080
export HTTPREQ_USERNAME=safexip        # any value; only the password matters
export HTTPREQ_PASSWORD=$SAFEXIP_API_KEY

lego run --dns httpreq --accept-tos \
  --domains '*.xip.example.com' \
  --email you@example.com \
  --path ./.lego
```

The cert and key are written to `.lego/certificates/`.

## DNS records served

| Query | Response |
|---|---|
| `<octets>.xip.example.com` A (e.g. `127-0-0-1...`) | A `<ip>` |
| `_acme-challenge.xip.example.com` TXT | TXT tokens set via the API |
| `xip.example.com` NS / SOA | zone NS and SOA |

## HTTP API

`GET /health` — no auth.

```json
{"status":"ok","domain":"xip.example.com"}
```

`POST /present` and `POST /cleanup` — require HTTP Basic auth (password = API key; username ignored). These follow lego's `httpreq` protocol.

```json
{"fqdn": "_acme-challenge.xip.example.com.", "value": "<token>"}
```

- `/present` adds a token. Multiple tokens coexist at the same name.
- `/cleanup` removes **only the specific token**, never other developers' tokens.

## Concurrent certificate generation

Multiple developers can issue certificates simultaneously. Each `lego` run uses a distinct ACME token; safexip stores all tokens at `_acme-challenge.xip.example.com` and serves them together, and each `lego` run cleans up only its own token. The practical limit is Let's Encrypt's duplicate-certificate rate limit (5 per week for the same name set), not safexip.

## Packaging

### Linux packages

```bash
# Requires nFPM.
make package
```

The package build writes `.deb`, `.rpm`, `.apk`, and Arch Linux packages to `dist/`.
Packages install `/usr/bin/safexip`, `/usr/lib/systemd/system/safexip.service`, and
`/etc/safexip/env`.

GitHub releases are published by pushing a version tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The release workflow uploads the Linux packages to the GitHub Release and pushes
Docker images to Docker Hub as `${DOCKERHUB_REPOSITORY}:<version>` and
`${DOCKERHUB_REPOSITORY}:latest`. Configure these repository settings first:

- `vars.DOCKERHUB_USERNAME`
- `vars.DOCKERHUB_REPOSITORY`, for example `henrikkarlsson/safexip`
- `secrets.DOCKERHUB_TOKEN`

Manual workflow runs build package artifacts without publishing a GitHub Release
or Docker image. Provide the optional `version` input to override the Cargo
version for those dry runs.

### systemd

Configure via `/etc/safexip/env`:

```ini
SAFEXIP_API_KEY=your-secret-key
SAFEXIP_DOMAIN=xip.example.com
SAFEXIP_NS_HOSTNAME=ns1.xip.example.com
SAFEXIP_NS_HOSTNAME2=ns2.xip.example.com
SAFEXIP_NS_IP=1.2.3.4
SAFEXIP_DNS_BIND=1.2.3.4
SAFEXIP_API_BIND=1.2.3.4
SAFEXIP_API_PORT=8080
# RUST_LOG=safexip=debug   # optional, for verbose DNS query logging
```

```bash
systemctl enable --now safexip
```

## Configuration

All options are env vars (or equivalent `--flags`):

| Env var | Default | Description |
|---|---|---|
| `SAFEXIP_API_KEY` | required | Shared secret for the HTTP API |
| `SAFEXIP_DOMAIN` | `xip.example.com` | Zone apex |
| `SAFEXIP_NS_HOSTNAME` | `ns1.xip.example.com` | Primary NS name |
| `SAFEXIP_NS_HOSTNAME2` | `ns2.xip.example.com` | Secondary NS name |
| `SAFEXIP_NS_IP` | `127.0.0.1` | IP for both NS names (glue) |
| `SAFEXIP_DNS_BIND` | `0.0.0.0` | DNS listen address |
| `SAFEXIP_DNS_PORT` | `53` | DNS port |
| `SAFEXIP_API_BIND` | `0.0.0.0` | HTTP API listen address |
| `SAFEXIP_API_PORT` | `8080` | HTTP API port |
| `SAFEXIP_TXT_TTL` | `60` | TTL for TXT records |
| `SAFEXIP_DEFAULT_TTL` | `60` | TTL for A/NS records |
