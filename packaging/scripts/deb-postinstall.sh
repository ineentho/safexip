#!/usr/bin/env sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 || true
  if [ "${1:-}" = "configure" ] && [ -n "${2:-}" ] \
    && systemctl is-active --quiet safexip.service; then
    systemctl restart safexip.service
  fi
fi
