#!/usr/bin/env bash
# Runs as root through SSM. Artifact directory contains binaries, never source.
set -euo pipefail
release="${1:?release directory required}"
region="${2:-us-east-1}"
umask 077
exec 9>/var/lock/starter-deploy.lock
flock -n 9
previous=$(readlink -e /opt/starter/current || true)
backup=$(mktemp -d)
refresh_was_enabled=$(systemctl is-enabled starter-db-refresh.timer 2>/dev/null || true)
managed_files=(/etc/starter/starter-api.env /etc/systemd/system/starter-api.service /etc/systemd/system/starter-db-refresh.service /etc/systemd/system/starter-db-refresh.timer)
for target in "${managed_files[@]}"; do
  [[ ! -f "$target" ]] || cp -p "$target" "$backup/$(basename "$target")"
done
rollback() {
  result=$?
  trap - EXIT
  if [[ "$result" != 0 ]]; then
    systemctl stop starter-api || true
    if [[ "$refresh_was_enabled" != enabled ]]; then
      systemctl disable --now starter-db-refresh.timer || true
    fi
    for target in "${managed_files[@]}"; do
      if [[ -f "$backup/$(basename "$target")" ]]; then
        cp -p "$backup/$(basename "$target")" "$target"
      else
        rm -f "$target"
      fi
    done
    if [[ -n "$previous" ]]; then
      ln -sfn "$previous" /opt/starter/current.next
      mv -Tf /opt/starter/current.next /opt/starter/current
      systemctl daemon-reload
      if [[ -f "$previous/refresh-database.py" ]]; then
        python3 "$previous/refresh-database.py" --no-restart --region "$region" || printf 'Could not refresh the restored database credential.\n' >&2
      fi
      systemctl restart starter-api || true
    fi
  fi
  rm -rf "$backup"
  exit "$result"
}
trap rollback EXIT
python3 - "$region" <<'PY'
import json, os, pathlib, subprocess, sys

def secret(name):
    return json.loads(subprocess.check_output(['aws', 'secretsmanager', 'get-secret-value', '--region', sys.argv[1], '--secret-id', name]))['SecretString']

env = json.loads(secret('silicon-starter/production/runtime'))
env['STARTER_DATABASE_URL'] = secret('silicon-starter/production/database-url').strip()
env.update(STARTER_BIND='127.0.0.1:8080', STARTER_FRONTEND_URL='https://starter.teamofsilicons.com', STARTER_AUTH_FILE='/var/lib/starter/auth.json')
# systemd EnvironmentFile double quotes require escaping backslash and quote.
def quote(value):
    if '\n' in value or '\r' in value or '\0' in value:
        raise ValueError('Multiline runtime value is unsupported')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'
text = ''.join(key + '=' + quote(value) + '\n' for key, value in env.items() if isinstance(value, str))
path = pathlib.Path('/etc/starter/starter-api.env')
path.write_text(text)
path.chmod(0o600)
PY
python3 "$release/refresh-database.py" --no-restart --region "$region"
for unit in starter-api.service starter-db-refresh.service starter-db-refresh.timer; do
  install -m 0644 "$release/$unit" "/etc/systemd/system/$unit"
done
ln -sfn "$release" /opt/starter/current.next
mv -Tf /opt/starter/current.next /opt/starter/current
systemctl daemon-reload
systemctl enable starter-api
systemctl restart starter-api
for attempt in {1..30}; do
  if curl -fsS http://127.0.0.1:8080/healthz; then
    systemctl enable --now starter-db-refresh.timer
    printf '\nInstalled %s\n' "$release"
    exit 0
  fi
  sleep 2
done
printf 'Health check failed; restoring previous release. See journalctl -u starter-api.\n' >&2
exit 1
