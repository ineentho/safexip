#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

for required in README.md docs/production.md docs/release-checklist.md \
  deploy/compose.yml deploy/.env.example deploy/initialize.sh \
  deploy/initialize-employee.sh; do
  test -f "$required" || {
    printf 'missing required release documentation artifact: %s\n' "$required" >&2
    exit 1
  }
done

cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
documented_version=$(sed -n 's/^SAFEXIP_IMAGE=ineentho\/safexip:\([^@]*\)@sha256:.*/\1/p' deploy/.env.example)
if [ "$documented_version" != "$cargo_version" ]; then
  printf 'documented image version %s does not match Cargo version %s\n' \
    "${documented_version:-<missing>}" "$cargo_version" >&2
  exit 1
fi

grep -q 'REPLACE_WITH_RELEASE_DIGEST' deploy/.env.example || {
  printf 'deploy/.env.example must fail closed until a verified release digest is supplied\n' >&2
  exit 1
}

if grep -E 'install -m 0?600 /dev/null .*(acme\.json|api-key)|tee .*safexip\.env' \
  README.md docs/*.md >/dev/null; then
  printf 'documentation contains a known destructive initialization pattern\n' >&2
  exit 1
fi

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

python3 - "$tmp_dir/snippets" README.md docs/*.md <<'PY'
from pathlib import Path
import sys

output = Path(sys.argv[1])
output.mkdir()
count = 0
for name in sys.argv[2:]:
    lines = Path(name).read_text().splitlines()
    block = None
    body = []
    for line in lines:
        if block is None and line in ("```bash", "```sh"):
            block = line[3:]
            body = []
        elif block is not None and line == "```":
            count += 1
            (output / f"snippet-{count}.{block}").write_text("\n".join(body) + "\n")
            block = None
        elif block is not None:
            body.append(line)
    if block is not None:
        raise SystemExit(f"unterminated shell block in {name}")
if count == 0:
    raise SystemExit("no shell snippets found")
PY

for snippet in "$tmp_dir"/snippets/*; do
  bash -n "$snippet"
done
sh -n deploy/initialize.sh deploy/initialize-employee.sh "$0"

server_dir="$tmp_dir/server"
deploy/initialize.sh "$server_dir" >/dev/null
python3 - "$server_dir" <<'PY'
from pathlib import Path
import re
import stat
import sys

root = Path(sys.argv[1])
acme = root / "letsencrypt/acme.json"
env = root / "safexip.env"
if acme.read_bytes() != b"":
    raise SystemExit("new ACME state file is not empty")
if not re.fullmatch(rb"SAFEXIP_API_KEY=[0-9a-f]{64}\nRUST_LOG=safexip=info\n", env.read_bytes()):
    raise SystemExit("generated server environment has an unexpected format")
for path in (acme, env):
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise SystemExit(f"incorrect mode for {path}")
PY
printf '%s\n' 'existing-traefik-acme-account-and-private-key' >"$server_dir/letsencrypt/acme.json"
printf '%s\n' 'SAFEXIP_API_KEY=existing-server-api-credential' \
  'RUST_LOG=safexip=debug' >"$server_dir/safexip.env"
cp "$server_dir/letsencrypt/acme.json" "$tmp_dir/acme.before"
cp "$server_dir/safexip.env" "$tmp_dir/server-env.before"
deploy/initialize.sh "$server_dir" >"$tmp_dir/server-rerun.log"
cmp "$tmp_dir/acme.before" "$server_dir/letsencrypt/acme.json"
cmp "$tmp_dir/server-env.before" "$server_dir/safexip.env"
grep -q 'Preserving existing' "$tmp_dir/server-rerun.log"

employee_dir="$tmp_dir/employee"
deploy/initialize-employee.sh "$employee_dir" >/dev/null
test ! -s "$employee_dir/api-key"
printf '%s\n' 'existing-employee-api-credential' >"$employee_dir/api-key"
cp "$employee_dir/api-key" "$tmp_dir/employee-key.before"
deploy/initialize-employee.sh "$employee_dir" >"$tmp_dir/employee-rerun.log"
cmp "$tmp_dir/employee-key.before" "$employee_dir/api-key"
grep -q 'Preserving existing' "$tmp_dir/employee-rerun.log"

if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
  compose_dir="$tmp_dir/compose"
  mkdir "$compose_dir"
  cp deploy/compose.yml "$compose_dir/compose.yml"
  cp deploy/.env.example "$compose_dir/.env"
  cp "$server_dir/safexip.env" "$compose_dir/safexip.env"
  mkdir "$compose_dir/letsencrypt"
  cp "$server_dir/letsencrypt/acme.json" "$compose_dir/letsencrypt/acme.json"
  (cd "$compose_dir" && docker compose config --quiet)
else
  printf 'docker compose is unavailable; skipping Compose rendering\n' >&2
fi

printf 'Production documentation validation passed for safexip %s.\n' "$cargo_version"
