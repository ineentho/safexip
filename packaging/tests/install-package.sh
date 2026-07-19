#!/usr/bin/env sh
# shellcheck disable=SC2016 # The command strings expand inside their containers.
set -eu

format=${1:?package format is required}
packages=$(cd "${2:?package directory is required}" && pwd)

case "$format" in
  deb)
    image=debian:bookworm-slim
    script='\
      pkg=$(find /packages -maxdepth 1 -name "*.deb" | head -n 1); \
      apt-get update; \
      apt-get install -y "$pkg"; \
      safexip --version; \
      dpkg-query --status safexip | grep "^Depends:" | grep -qw systemd; \
      test -f /usr/lib/systemd/system/safexip.service; \
      test ! -e /etc/init.d/safexip'
    ;;
  rpm)
    image=fedora:43
    script='\
      pkg=$(find /packages -maxdepth 1 -name "*.rpm" | head -n 1); \
      dnf install -y "$pkg"; \
      safexip --version; \
      rpm -q --requires safexip | grep -qx systemd; \
      test -f /usr/lib/systemd/system/safexip.service; \
      test ! -e /etc/init.d/safexip'
    ;;
  apk)
    image=alpine:3.22
    script='\
      pkg=$(find /packages -maxdepth 1 -name "*.apk" | head -n 1); \
      apk add --no-cache --allow-untrusted "$pkg"; \
      safexip --version; \
      apk info --depends safexip | grep -qx openrc; \
      apk info --depends safexip | grep -qx libcap; \
      test -x /etc/init.d/safexip; \
      id -u safexip >/dev/null; \
      getcap /usr/bin/safexip | grep -q cap_net_bind_service; \
      test ! -e /usr/lib/systemd/system/safexip.service'
    ;;
  archlinux)
    image=archlinux:base
    script='\
      pkg=$(find /packages -maxdepth 1 -name "*.pkg.tar.zst" | head -n 1); \
      sed -i "s/^#DisableSandboxSyscalls/DisableSandboxSyscalls/" /etc/pacman.conf; \
      pacman -Sy --noconfirm; \
      pacman -U --noconfirm "$pkg"; \
      safexip --version; \
      pacman -Qi safexip | grep "Depends On" | grep -qw systemd; \
      test -f /usr/lib/systemd/system/safexip.service; \
      test ! -e /etc/init.d/safexip'
    ;;
  *)
    echo "unsupported package format: $format" >&2
    exit 2
    ;;
esac

docker run --rm --platform linux/amd64 \
  --volume "$packages:/packages:ro" "$image" sh -euxc "$script"
