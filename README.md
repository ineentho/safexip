# safexip

Self-hosted xip-style DNS server with ACME DNS-01 API for wildcard TLS certs.

## How it works

1. Delegate `xip.example.com` to safexip via NS record
2. safexip resolves `vite-127-0-0-1.xip.example.com` → `127.0.0.1`
3. POST ACME challenge tokens via HTTP API → safexip serves them as TXT records
4. Generate wildcard certs locally — private keys never leave your machine

## Usage

```bash
# Start server
SAFEXIP_API_KEY=my-secret \
safexip --domain xip.example.com \
  --ns-hostname ns1.xip.example.com \
  --ns-ip 1.2.3.4

# Generate wildcard cert
lego --dns exec --domains '*.xip.example.com' --path ./.lego run \
  ./safexip-acme-helper.sh

# Use the cert with any TLS service (e.g. lazy)
lazy proxy \
  --suffix .xip.example.com \
  --cert ./.lego/certificates/_.xip.example.com.crt \
  --key ./.lego/certificates/_.xip.example.com.key
```

## DNS API

| Hostname pattern | Returns |
|---|---|
| `auth-127-0-0-1.xip.example.com` | A `127.0.0.1` |
| `_acme-challenge.xip.example.com` | TXT (from API) |
| `xip.example.com` | SOA, NS |

## HTTP API

All endpoints require `Authorization: Bearer <api-key>`.

`POST /v1/txt` — set ACME challenge token

```json
{"name": "_acme-challenge.xip.example.com", "value": "token"}
```

`DELETE /v1/txt?name=_acme-challenge.xip.example.com` — remove token

## Packaging

### .deb (Debian/Ubuntu)

```bash
# On the target Debian/Ubuntu machine:
cargo install cargo-deb
cargo deb --install

# Or cross-compile from macOS via Docker:
make cross-deb
# Output: target/x86_64-linux/safexip_*.deb
```

The `.deb` installs:
- `/usr/bin/safexip`
- `/lib/systemd/system/safexip.service`
- `/usr/share/safexip/safexip-acme-helper.sh`

### systemd

Configure via `/etc/safexip/env`:

```ini
SAFEXIP_API_KEY=your-secret-key
SAFEXIP_DOMAIN=xip.example.com
SAFEXIP_NS_HOSTNAME=ns1.xip.example.com
SAFEXIP_NS_IP=1.2.3.4
SAFEXIP_API_PORT=8080
```

```bash
systemctl daemon-reload
systemctl enable --now safexip
```

## Deployment via Docker

```yaml
# docker-compose.yml
services:
  safexip:
    build: .
    ports:
      - "53:53/udp"
      - "53:53/tcp"
      - "8080:8080"
    environment:
      SAFEXIP_API_KEY: "${SAFEXIP_API_KEY}"
    command: >
      --domain xip.example.com
      --ns-hostname ns1.xip.example.com
      --ns-ip <server-public-ip>
```
