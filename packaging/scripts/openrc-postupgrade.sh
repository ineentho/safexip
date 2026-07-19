#!/usr/bin/env sh
set -eu

setcap cap_net_bind_service=+ep /usr/bin/safexip
if command -v rc-service >/dev/null 2>&1 \
  && rc-service safexip status >/dev/null 2>&1; then
  rc-service safexip restart
fi
