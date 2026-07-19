#!/usr/bin/env sh
set -eu

if command -v rc-service >/dev/null 2>&1; then
  rc-service safexip stop >/dev/null 2>&1 || true
fi
if command -v rc-update >/dev/null 2>&1; then
  rc-update del safexip >/dev/null 2>&1 || true
fi
