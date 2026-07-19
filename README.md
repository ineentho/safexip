# safexip

[![CI](https://github.com/ineentho/safexip/actions/workflows/ci.yml/badge.svg)](https://github.com/ineentho/safexip/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/ineentho/safexip)](https://github.com/ineentho/safexip/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

safexip is a small authoritative xip-style DNS server with the DNS-01 HTTP API expected by lego's `httpreq` provider. ACME accounts, certificates, and private keys stay with each employee; safexip stores only short-lived challenge values.

> [!IMPORTANT]
> safexip is intentionally authoritative for one delegated zone; it is not a
> recursive resolver or a general-purpose DNS hosting platform. The
> credential-bearing HTTP API must not be exposed over plain HTTP.

## Quick start

This local example keeps the credential-bearing API on loopback. It is suitable for evaluation when lego runs on the same machine; do not expose port 8080 directly to the internet.

```bash
export SAFEXIP_API_KEY="$(openssl rand -hex 32)"
export SAFEXIP_DOMAIN=xip.example.com
export SAFEXIP_NS_HOSTNAME=ns1.xip.example.com
export SAFEXIP_NS_HOSTNAME2=ns2.xip.example.com
export SAFEXIP_NS_IP=203.0.113.10

safexip
```

Delegate the zone with NS records and in-bailiwick A glue, then configure lego to use `http://127.0.0.1:8080` with username `safexip` and the generated key.

In another shell on the same host, preserve lego's account and private-key directory between runs and request a certificate:

```bash
install -d -m 0700 "${HOME}/.local/share/safexip/lego"
export HTTPREQ_ENDPOINT=http://127.0.0.1:8080
export HTTPREQ_USERNAME=safexip
export HTTPREQ_PASSWORD='<the same generated key>'
lego run \
  --path "${HOME}/.local/share/safexip/lego" \
  --email you@example.com \
  --dns httpreq \
  --domains '*.xip.example.com' \
  --accept-tos
```

For a remote, HTTPS-protected deployment, follow the **[production deployment guide](docs/production.md)**. It uses the latest image by default, supports pinning a specific release, and includes separate first-install, upgrade, recovery, and destructive credential-rotation procedures. Never expose the authenticated API over plain HTTP.

## What safexip serves

For `xip.example.com`:

| Query | Response |
|---|---|
| `203-0-113-10.xip.example.com` A | `203.0.113.10` |
| `_acme-challenge.xip.example.com` TXT | One record per active ACME value |
| `xip.example.com` NS / SOA | Authoritative records |
| Existing name with unsupported type | NODATA |
| Unknown name inside the zone | `NXDOMAIN` |
| Name outside the zone | `REFUSED` |

DNS is served over UDP and TCP. Oversized UDP responses are truncated so resolvers retry over TCP.

## HTTP API

`GET /health` is unauthenticated:

```json
{"status":"ok","domain":"xip.example.com"}
```

`POST /present` and `POST /cleanup` require HTTP Basic authentication. The username is ignored; the password must equal `SAFEXIP_API_KEY`. Authentication is checked before the request body is read, and unauthorized responses include the standard Basic authentication challenge.

```json
{"fqdn":"_acme-challenge.xip.example.com.","value":"base64url-acme-value"}
```

Only the configured zone's exact challenge name is accepted. Tokens must be non-empty DNS TXT strings no longer than 255 bytes. Duplicate values are refreshed, concurrent values are separate TXT records, active tokens are bounded, and abandoned tokens expire automatically. A presentation is rejected with HTTP 503 before it would make the complete active TXT record set too large for a DNS-over-TCP message; existing records are left unchanged.

The application also applies defense-in-depth HTTP limits: 1 KiB request bodies, a 10-second request timeout, a 5-second header timeout, 64 concurrent authenticated requests, 128 HTTP connections, and an authenticated mutation rate of 20 requests per second. The rate-limit burst is at least 100 and scales to twice the configured active-token limit so a normal concurrent present/cleanup cycle is not blocked. These limits bound a directly exposed or incorrectly proxied process, but they do not provide transport security: keep the API on loopback or a private backend network and use verified HTTPS for every remote client.

The endpoints implement lego's [`httpreq` provider](https://go-acme.github.io/lego/dns/httpreq/):

- `POST /present`
- `POST /cleanup`

## Configuration

Every option is available as an environment variable or equivalent command-line flag.

| Environment variable | Default | Description |
|---|---|---|
| `SAFEXIP_API_KEY` | required | API secret; at least 32 characters |
| `SAFEXIP_DOMAIN` | `xip.example.com` | Lowercased zone apex |
| `SAFEXIP_NS_HOSTNAME` | `ns1.xip.example.com` | Primary in-zone NS name |
| `SAFEXIP_NS_HOSTNAME2` | `ns2.xip.example.com` | Distinct secondary in-zone NS name |
| `SAFEXIP_NS_IP` | `127.0.0.1` | IPv4 glue address for both NS names |
| `SAFEXIP_DNS_BIND` | `0.0.0.0` | DNS listen address |
| `SAFEXIP_DNS_PORT` | `53` | DNS UDP/TCP port |
| `SAFEXIP_API_BIND` | `127.0.0.1` | HTTP API listen address |
| `SAFEXIP_API_PORT` | `8080` | HTTP API port |
| `SAFEXIP_TXT_TTL` | `60` | TXT TTL in seconds, 1–86400 |
| `SAFEXIP_DEFAULT_TTL` | `60` | A/NS TTL in seconds, 1–86400 |
| `SAFEXIP_TOKEN_LIFETIME` | `600` | Token lifetime in seconds, 1–86400 |
| `SAFEXIP_MAX_TOKENS` | `100` | Maximum active tokens, from 1 to the DNS-wire maximum calculated for the configured names |
| `RUST_LOG` | `safexip=info` | Tracing filter; use `safexip=debug` for DNS queries |

Configuration is validated before listeners start. Names are normalized to lowercase, nameservers must be distinct and inside the delegated zone, addresses and ports must parse correctly, and short API keys are rejected. The maximum token count is zone-dependent because DNS name and record overhead consume part of the 65,535-byte TCP message limit; an invalid setting reports the calculated maximum at startup. The count limit is a secondary bound: the API also accounts for the exact active token lengths and DNS message overhead on every presentation.

## Other installation methods

Download packages and checksums from the
[latest GitHub release](https://github.com/ineentho/safexip/releases/latest).
Every release includes static-musl amd64 packages tested in clean containers for these distributions:

| Distribution tested by CI | Package | Service manager |
|---|---|---|
| Debian 12 (Bookworm) | `.deb` | systemd |
| Fedora 43 | `.rpm` | systemd |
| Alpine 3.22 | `.apk` | OpenRC |
| Arch Linux rolling container | `.pkg.tar.zst` | systemd |

All packages install `/usr/bin/safexip` and `/etc/safexip/env`. Debian, Fedora, and Arch packages install `/usr/lib/systemd/system/safexip.service`; Alpine installs `/etc/init.d/safexip`. The systemd unit uses a dynamic unprivileged user with only `CAP_NET_BIND_SERVICE`; the OpenRC package creates an unprivileged `safexip` user and grants the binary only that capability.

Installation deliberately leaves the service stopped because the packaged API key is empty. Configure `/etc/safexip/env`, then enable and start it:

```bash
# Debian, Fedora, or Arch
sudo systemctl enable --now safexip

# Alpine
sudo rc-update add safexip default
sudo rc-service safexip start
```

Upgrades restart safexip only when it was already running. An inactive service remains inactive. Package removal stops and disables the service before removing its service definition.

Build from source with Rust 1.88 or newer:

```bash
cargo build --release --locked
```

To build Linux packages on an amd64 Linux host, install the Rust `x86_64-unknown-linux-musl` target, a musl linker (the `musl-tools` package on Debian/Ubuntu), and [nFPM](https://nfpm.goreleaser.com/):

```bash
rustup target add x86_64-unknown-linux-musl
make package
```

## Development and releases

Run the local quality gates:

```bash
make check
scripts/validate-production-docs.sh
```

[`release-plz`](https://release-plz.dev/) maintains a release PR with the next
version and changelog. Complete [`docs/release-checklist.md`](docs/release-checklist.md)
before merging that PR. Merging it creates the matching `vX.Y.Z` tag and GitHub
release, then dispatches the release workflow.

The repository setting **Allow GitHub Actions to create and approve pull
requests** must be enabled so the workflow can maintain the release PR. For a
manual fallback, create a matching `vX.Y.Z` tag and dispatch the release
workflow from that tag.
CI and the release workflow verify that the production guide and Compose
artifacts are complete, use the latest image by default, and preserve existing
deployment state when initialization is rerun.

The release workflow verifies formatting, Clippy, unit and real-listener integration tests, dependency advisories, tag/version equality, static linkage, clean installation of every package format, and systemd/OpenRC upgrade and removal behavior. Its service tests exercise health plus UDP and TCP DNS. It also smoke-tests the exact amd64/arm64 image digests with the documented UID, capabilities, read-only filesystem, and resource limits. Four amd64 Linux packages, checksums, and the public multi-platform Docker tags `${VERSION}` and `latest` are published only after the required checks pass, then the packages are attached to the GitHub release. Before a release, also perform the [delegated-zone ACME staging validation](docs/pre-release-acme.md).

## License

Licensed under either the [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

## Security

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md),
not in a public issue.
