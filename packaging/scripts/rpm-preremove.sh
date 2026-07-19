#!/usr/bin/env sh
set -eu

if [ "${1:-1}" -eq 0 ] 2>/dev/null \
  && command -v systemctl >/dev/null 2>&1; then
  systemctl disable --now safexip.service >/dev/null 2>&1 || true
  systemctl reset-failed safexip.service >/dev/null 2>&1 || true
fi
