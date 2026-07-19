#!/bin/sh
set -eu

DEPLOYMENT_DIR=${1:-/opt/safexip}

install -d -m 0750 "$DEPLOYMENT_DIR"
install -d -m 0700 "$DEPLOYMENT_DIR/letsencrypt"

acme_file="$DEPLOYMENT_DIR/letsencrypt/acme.json"
if [ -e "$acme_file" ] || [ -L "$acme_file" ]; then
  printf 'Preserving existing %s\n' "$acme_file"
else
  (umask 077; set -C; : >"$acme_file") 2>/dev/null || {
    printf 'Preserving concurrently created %s\n' "$acme_file"
  }
fi
chmod 0600 "$acme_file"

env_file="$DEPLOYMENT_DIR/safexip.env"
if [ -e "$env_file" ] || [ -L "$env_file" ]; then
  printf 'Preserving existing %s\n' "$env_file"
else
  api_key=$(openssl rand -hex 32)
  if (umask 077; set -C; printf 'SAFEXIP_API_KEY=%s\nRUST_LOG=safexip=info\n' "$api_key" >"$env_file") 2>/dev/null; then
    printf 'Created %s\n' "$env_file"
  else
    printf 'Preserving concurrently created %s\n' "$env_file"
  fi
fi
chmod 0600 "$env_file"
