#!/bin/sh
set -eu

CONFIG_DIR=${1:-"${HOME}/.config/safexip"}
key_file="$CONFIG_DIR/api-key"

install -d -m 0700 "$CONFIG_DIR"
if [ -e "$key_file" ] || [ -L "$key_file" ]; then
  printf 'Preserving existing %s\n' "$key_file"
else
  if (umask 077; set -C; : >"$key_file") 2>/dev/null; then
    printf 'Created %s; fill it with the API key using a secure editor or password-manager CLI.\n' "$key_file"
  else
    printf 'Preserving concurrently created %s\n' "$key_file"
  fi
fi
chmod 0600 "$key_file"
