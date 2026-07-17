# safexip

Self-hosted authoritative xip-style DNS server with an ACME DNS-01 HTTP API for wildcard TLS certificates. Private keys remain with the ACME client; safexip only stores challenge tokens, and automatically expires them after ten minutes by default.

## How it works

1. Delegate a subdomain such as `xip.example.com` to safexip using NS records in the parent zone.
2. safexip resolves encoded IPv4 hostnames such as `127-0-0-1.xip.example.com` to `127.0.0.1`.
3. During issuance, lego posts the ACME challenge to safexip's authenticated HTTP API.
4. safexip publishes the challenge at `_acme-challenge.xip.example.com`; lego keeps the certificate and private key locally.

## Installation

Download Linux packages from the [GitHub releases](https://github.com/ineentho/safexip/releases), use the Docker image `ineentho/safexip`, or build from source with Rust 1.88 or newer:

```bash
cargo build --release --locked
```

## Quick start

Generate a key and start the server. This example keeps the credential-bearing API on loopback, so lego must run on the same machine:

```bash
export SAFEXIP_API_KEY="$(openssl rand -hex 32)"
export SAFEXIP_DOMAIN=xip.example.com
export SAFEXIP_NS_HOSTNAME=ns1.xip.example.com
export SAFEXIP_NS_HOSTNAME2=ns2.xip.example.com
export SAFEXIP_NS_IP=1.2.3.4

safexip
```

In the parent zone, create the delegation and glue records:

```text
xip.example.com.     NS ns1.xip.example.com.
xip.example.com.     NS ns2.xip.example.com.
ns1.xip.example.com. A  1.2.3.4
ns2.xip.example.com. A  1.2.3.4
```

In another shell on the safexip host, request a certificate with [lego's `httpreq` provider](https://go-acme.github.io/lego/dns/httpreq/):

```bash
export SAFEXIP_API_KEY='<the same generated key>'
export HTTPREQ_ENDPOINT=http://127.0.0.1:8080
export HTTPREQ_USERNAME=safexip
export HTTPREQ_PASSWORD="$SAFEXIP_API_KEY"

lego run --dns httpreq --accept-tos \
  --domains '*.xip.example.com' \
  --email you@example.com \
  --path ./.lego
```

The certificate and key are written to `.lego/certificates/`.

## Remote ACME clients

HTTP Basic authentication does not protect credentials on an unencrypted connection. Keep `SAFEXIP_API_BIND=127.0.0.1` when lego runs locally. For remote clients, put an HTTPS reverse proxy in front of port 8080, configure `SAFEXIP_API_BIND` to an address reachable only by that proxy, and set `HTTPREQ_ENDPOINT` to the HTTPS URL. Do not expose the API directly over the public internet with plain HTTP.

## DNS records served

| Query | Response |
|---|---|
| `<octets>.xip.example.com` A, such as `127-0-0-1...` | Encoded IPv4 address |
| `_acme-challenge.xip.example.com` TXT | One TXT record per active token |
| `xip.example.com` NS / SOA | Zone NS and SOA records |
| Names outside the configured zone | `REFUSED` |

Both UDP and TCP DNS are supported. Oversized UDP responses set the truncation flag so resolvers retry over TCP.

## HTTP API

`GET /health` requires no authentication:

```json
{"status":"ok","domain":"xip.example.com"}
```

`POST /present` and `POST /cleanup` implement lego's default `httpreq` protocol and require HTTP Basic authentication. The username is ignored and the password must equal `SAFEXIP_API_KEY`.

```json
{"fqdn":"_acme-challenge.xip.example.com.","value":"<token>"}
```

Only the configured zone's exact ACME challenge name is accepted. `/present` adds or refreshes a token; `/cleanup` removes only the specified token. Distinct concurrent challenges are served as separate TXT records. Duplicate tokens are deduplicated, the active-token count is bounded, and abandoned tokens expire automatically.

## Docker

The image runs as an unprivileged user. To use the HTTP API from another container or an HTTPS reverse proxy, explicitly bind it inside the container:

```bash
docker run --rm \
  --name safexip \
  -p 53:53/udp -p 53:53/tcp -p 127.0.0.1:8080:8080/tcp \
  -e SAFEXIP_API_KEY="$(openssl rand -hex 32)" \
  -e SAFEXIP_DOMAIN=xip.example.com \
  -e SAFEXIP_NS_HOSTNAME=ns1.xip.example.com \
  -e SAFEXIP_NS_HOSTNAME2=ns2.xip.example.com \
  -e SAFEXIP_NS_IP=1.2.3.4 \
  -e SAFEXIP_API_BIND=0.0.0.0 \
  ineentho/safexip:0.2.0
```

Use an immutable version tag in production, not `latest`.

## Linux packages and systemd

Packages install `/usr/bin/safexip`, `/usr/lib/systemd/system/safexip.service`, and `/etc/safexip/env`. Edit `/etc/safexip/env`, replace the empty API key, configure the zone, and then start the service:

```bash
sudo systemctl enable --now safexip
```

The service uses a dynamic unprivileged user and receives only `CAP_NET_BIND_SERVICE` for port 53.

To build `.deb`, `.rpm`, `.apk`, and Arch Linux packages locally, run the following on an amd64 Linux host with [nFPM](https://nfpm.goreleaser.com/) installed:

```bash
make package
```

The Makefile refuses to create Linux packages on another operating system. Official packages are built on Linux by the release workflow.

## Configuration

Every option is available as an environment variable or equivalent command-line flag.

| Environment variable | Default | Description |
|---|---|---|
| `SAFEXIP_API_KEY` | required | API secret; at least 32 characters |
| `SAFEXIP_DOMAIN` | `xip.example.com` | Lowercased zone apex; a trailing dot is accepted |
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
| `SAFEXIP_MAX_TOKENS` | `100` | Maximum active tokens, 1–10000 |
| `RUST_LOG` | `safexip=info` | Tracing filter; use `safexip=debug` for DNS queries |

Configuration is validated before any listener starts. Names are normalized to lowercase, nameservers must be distinct and inside the delegated zone, addresses must parse correctly, and insecurely short API keys are rejected.

## Development and releases

Run the same quality gates as CI:

```bash
make check
```

To publish a release, update the version in `Cargo.toml`, commit it, and push a matching tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

The release workflow verifies formatting, Clippy, tests, dependency advisories, and tag/version equality before building. It publishes four amd64 Linux packages, checksums, and amd64/arm64 Docker images. The GitHub release is created only after both package and Docker jobs succeed. Manual workflow runs build package artifacts without publishing.

## License

MIT. See [LICENSE](LICENSE).
