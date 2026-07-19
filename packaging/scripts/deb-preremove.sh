#!/usr/bin/env sh
set -eu

case "${1:-}" in
  remove|deconfigure)
    if command -v systemctl >/dev/null 2>&1; then
      systemctl disable --now safexip.service >/dev/null 2>&1 || true
      systemctl reset-failed safexip.service >/dev/null 2>&1 || true
    fi
    ;;
esac
