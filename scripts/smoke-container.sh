#!/usr/bin/env bash
set -euo pipefail

image="${1:?usage: smoke-container.sh IMAGE}"
name="safexip-smoke-${RANDOM}"
api_key=0123456789abcdef0123456789abcdef

cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

test "$(docker image inspect --format '{{.Config.User}}' "$image")" = "10001"

docker run --detach --name "$name" \
  --user 10001:10001 \
  --cap-drop ALL --cap-add NET_BIND_SERVICE \
  --security-opt no-new-privileges=true \
  --read-only --tmpfs /tmp:size=1m,mode=1777 \
  --memory 128m --cpus 1 --pids-limit 128 \
  --publish 127.0.0.1::53/udp \
  --publish 127.0.0.1::53/tcp \
  --publish 127.0.0.1::8080/tcp \
  --env SAFEXIP_DOMAIN=xip.test \
  --env SAFEXIP_NS_HOSTNAME=ns1.xip.test \
  --env SAFEXIP_NS_HOSTNAME2=ns2.xip.test \
  --env SAFEXIP_NS_IP=127.0.0.1 \
  --env SAFEXIP_API_BIND=0.0.0.0 \
  --env SAFEXIP_API_KEY="$api_key" \
  "$image" >/dev/null

test "$(docker inspect --format '{{.Config.User}}' "$name")" = "10001:10001"
test "$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "$name")" = "true"
test "$(docker inspect --format '{{.HostConfig.Memory}}' "$name")" = "134217728"
test "$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$name")" = "128"
test "$(docker inspect --format '{{.HostConfig.CapDrop}}' "$name")" = "[ALL]"
test "$(docker inspect --format '{{.HostConfig.CapAdd}}' "$name")" = "[CAP_NET_BIND_SERVICE]"
test "$(docker inspect --format '{{.HostConfig.SecurityOpt}}' "$name")" = "[no-new-privileges=true]"

udp_port="$(docker port "$name" 53/udp | sed 's/.*://')"
tcp_port="$(docker port "$name" 53/tcp | sed 's/.*://')"
api_port="$(docker port "$name" 8080/tcp | sed 's/.*://')"

for _ in $(seq 1 100); do
  if curl --fail --silent "http://127.0.0.1:${api_port}/health" | grep -q '"status":"ok"'; then
    break
  fi
  sleep 0.1
done
curl --fail --silent "http://127.0.0.1:${api_port}/health" | grep -q '"domain":"xip.test"'
test "$(dig @127.0.0.1 -p "$udp_port" 127-0-0-1.xip.test A +short)" = "127.0.0.1"
test "$(dig @127.0.0.1 -p "$tcp_port" 127-0-0-1.xip.test A +tcp +short)" = "127.0.0.1"

body='{"fqdn":"_acme-challenge.xip.test.","value":"container-smoke"}'
curl --fail --silent --user "test:${api_key}" -H 'content-type: application/json' \
  --data "$body" "http://127.0.0.1:${api_port}/present" >/dev/null
dig @127.0.0.1 -p "$udp_port" _acme-challenge.xip.test TXT +short | grep -q '"container-smoke"'
curl --fail --silent --user "test:${api_key}" -H 'content-type: application/json' \
  --data "$body" "http://127.0.0.1:${api_port}/cleanup" >/dev/null
test -z "$(dig @127.0.0.1 -p "$udp_port" _acme-challenge.xip.test TXT +short)"

docker stop --time 15 "$name" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$name")" = "0"
