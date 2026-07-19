#!/usr/bin/env sh
set -eu

if ! id -u safexip >/dev/null 2>&1; then
  addgroup -S safexip >/dev/null 2>&1 || true
  adduser -S -D -H -s /sbin/nologin -G safexip safexip >/dev/null 2>&1
fi

setcap cap_net_bind_service=+ep /usr/bin/safexip
