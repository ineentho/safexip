#!/usr/bin/env bash
set -euo pipefail

packages=$(cd "${1:?package directory is required}" && pwd)
fixtures=$(cd "${2:?fixture directory is required}" && pwd)
old=$(find "$fixtures" -maxdepth 1 -name '*0.0.0*.deb' | head -n 1)
middle=$(find "$fixtures" -maxdepth 1 -name '*0.0.1*.deb' | head -n 1)
current=$(find "$packages" -maxdepth 1 -name '*.deb' | head -n 1)

cleanup() {
  sudo apt-get remove -y safexip >/dev/null 2>&1 || true
  sudo systemctl daemon-reload >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_for_health() {
  for _ in {1..30}; do
    if curl --fail --silent http://127.0.0.1:18080/health \
      | grep -q '"status":"ok"'; then
      return 0
    fi
    sleep 1
  done
  sudo systemctl status safexip.service --no-pager || true
  return 1
}

sudo apt-get install -y "$old"
sudo tee /etc/safexip/env >/dev/null <<'EOF'
SAFEXIP_API_KEY=0123456789abcdef0123456789abcdef
SAFEXIP_DOMAIN=xip.test
SAFEXIP_NS_HOSTNAME=ns1.xip.test
SAFEXIP_NS_HOSTNAME2=ns2.xip.test
SAFEXIP_NS_IP=127.0.0.1
SAFEXIP_DNS_BIND=127.0.0.1
SAFEXIP_DNS_PORT=1053
SAFEXIP_API_BIND=127.0.0.1
SAFEXIP_API_PORT=18080
EOF
sudo systemctl enable --now safexip.service
wait_for_health

udp=$(dig @127.0.0.1 -p 1053 127-0-0-1.xip.test A +short)
tcp=$(dig @127.0.0.1 -p 1053 127-0-0-1.xip.test A +tcp +short)
test "$udp" = 127.0.0.1
test "$tcp" = 127.0.0.1

old_pid=$(systemctl show --property MainPID --value safexip.service)
sudo apt-get install -y "$middle"
wait_for_health
middle_pid=$(systemctl show --property MainPID --value safexip.service)
test "$middle_pid" -gt 0
test "$middle_pid" != "$old_pid"

sudo systemctl stop safexip.service
sudo apt-get install -y "$current"
if systemctl is-active --quiet safexip.service; then
  echo "inactive service was started during upgrade" >&2
  exit 1
fi

sudo systemctl start safexip.service
wait_for_health
running_pid=$(systemctl show --property MainPID --value safexip.service)
sudo apt-get remove -y safexip
if systemctl is-active --quiet safexip.service; then
  echo "service is still active after removal" >&2
  exit 1
fi
if kill -0 "$running_pid" >/dev/null 2>&1; then
  echo "service process survived package removal" >&2
  exit 1
fi
test ! -e /etc/systemd/system/multi-user.target.wants/safexip.service
sudo systemctl daemon-reload
test -z "$(systemctl show --property FragmentPath --value safexip.service 2>/dev/null || true)"

trap - EXIT
