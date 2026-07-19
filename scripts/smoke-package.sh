#!/usr/bin/env bash
set -euo pipefail

package="${1:?usage: smoke-package.sh PACKAGE}"
api_key=0123456789abcdef0123456789abcdef

case "$package" in
  *.deb)
    apt-get update
    apt-get install -y --no-install-recommends curl dnsutils "$package"
    ;;
  *.rpm)
    dnf install -y curl bind-utils "$package"
    ;;
  *.pkg.tar.zst)
    pacman -Sy --noconfirm curl bind
    pacman -U --noconfirm "$package"
    ;;
  *) echo "unsupported package: $package" >&2; exit 2 ;;
esac

cat >/etc/safexip/env <<EOF
SAFEXIP_API_KEY=$api_key
SAFEXIP_DOMAIN=xip.test
SAFEXIP_NS_HOSTNAME=ns1.xip.test
SAFEXIP_NS_HOSTNAME2=ns2.xip.test
SAFEXIP_NS_IP=127.0.0.1
SAFEXIP_DNS_BIND=127.0.0.1
SAFEXIP_DNS_PORT=5353
SAFEXIP_API_BIND=127.0.0.1
SAFEXIP_API_PORT=18080
EOF
chmod 0600 /etc/safexip/env

systemctl daemon-reload
if ! systemctl start safexip.service; then
  systemctl status safexip.service --no-pager || true
  journalctl --unit safexip.service --no-pager || true
  exit 1
fi
for _ in $(seq 1 100); do
  if curl --fail --silent http://127.0.0.1:18080/health | grep -q '"status":"ok"'; then
    break
  fi
  sleep 0.1
done
if ! systemctl is-active --quiet safexip.service; then
  systemctl status safexip.service --no-pager || true
  journalctl --unit safexip.service --no-pager || true
  exit 1
fi
test "$(dig @127.0.0.1 -p 5353 127-0-0-1.xip.test A +short)" = "127.0.0.1"
test "$(dig @127.0.0.1 -p 5353 127-0-0-1.xip.test A +tcp +short)" = "127.0.0.1"
systemctl stop safexip.service
systemctl is-active --quiet safexip.service && exit 1 || true
