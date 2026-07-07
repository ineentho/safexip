#!/usr/bin/env bash
set -euo pipefail

# Cross-compile safexip .deb for x86_64 Linux using Docker.
# Output: target/x86_64-linux/safexip_*.deb

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

IMAGE="rust:bullseye"

docker run --rm -i \
  --user "$(id -u):$(id -g)" \
  -v "${PROJECT_DIR}:/build" \
  -w /build \
  "${IMAGE}" \
  bash -c "
    set -euo pipefail
    apt-get update -qq
    apt-get install -y -qq dpkg-dev lintian
    cargo install cargo-deb
    cargo deb --target x86_64-unknown-linux-gnu -o /build/target/x86_64-linux/
  "
