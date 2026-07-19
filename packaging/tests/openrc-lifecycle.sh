#!/usr/bin/env sh
set -eu

packages=$(cd "${1:?package directory is required}" && pwd)
fixtures=$(cd "${2:?fixture directory is required}" && pwd)

docker run --rm \
  --platform linux/amd64 \
  --volume "$packages:/packages:ro" \
  --volume "$fixtures:/fixtures:ro" \
  alpine:3.22 sh -euxc '
    apk add --no-cache curl bind-tools
    old=$(find /fixtures -maxdepth 1 -name "*0.0.0*.apk" | head -n 1)
    middle=$(find /fixtures -maxdepth 1 -name "*0.0.1*.apk" | head -n 1)
    current=$(find /packages -maxdepth 1 -name "*.apk" | head -n 1)
    apk add --allow-untrusted "$old"
    mkdir -p /run/openrc
    touch /run/openrc/softlevel
    cat > /etc/safexip/env <<EOF
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
    rc-update add safexip default
    rc-service safexip start
    for i in $(seq 1 30); do
      curl --fail --silent http://127.0.0.1:18080/health | grep -q "\"status\":\"ok\"" && break
      test "$i" -lt 30
      sleep 1
    done
    test "$(dig @127.0.0.1 -p 1053 127-0-0-1.xip.test A +short)" = 127.0.0.1
    test "$(dig @127.0.0.1 -p 1053 127-0-0-1.xip.test A +tcp +short)" = 127.0.0.1
    old_pid=$(cat /run/safexip/safexip.pid)
    apk add --upgrade --allow-untrusted "$middle"
    middle_pid=$(cat /run/safexip/safexip.pid)
    test "$middle_pid" != "$old_pid"
    curl --fail --silent http://127.0.0.1:18080/health >/dev/null
    rc-service safexip stop
    apk add --upgrade --allow-untrusted "$current"
    ! rc-service safexip status
    rc-service safexip start
    running_pid=$(cat /run/safexip/safexip.pid)
    apk del safexip
    ! kill -0 "$running_pid"
    test ! -e /etc/init.d/safexip
    ! rc-update show default | grep -qw safexip
  '
