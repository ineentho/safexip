#!/usr/bin/env sh
set -eu

rm -f /run/safexip/safexip.pid
rmdir /run/safexip >/dev/null 2>&1 || true
