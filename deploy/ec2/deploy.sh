#!/usr/bin/env bash
set -euo pipefail

host="${1:?usage: $0 user@ec2-host [binary]}"
binary="${2:-target/release/silicon-starter-api}"

[[ -x "$binary" ]] || {
  echo "binary is missing or not executable: $binary" >&2
  echo "build it on Linux first, or pass a Linux binary as the second argument" >&2
  exit 1
}

scp "$binary" "$host:/tmp/starter-api"
scp "$(dirname "$0")/starter-api.service" "$host:/tmp/starter-api.service"
ssh "$host" bash -s <<'REMOTE'
set -euo pipefail
if ! id starter >/dev/null 2>&1; then
  sudo useradd --system --home /var/lib/starter --shell /usr/sbin/nologin starter
fi
sudo install -d -o starter -g starter /var/lib/starter /etc/starter
sudo install -o starter -g starter -m 0755 /tmp/starter-api /usr/local/bin/starter-api
sudo install -o root -g root -m 0644 /tmp/starter-api.service /etc/systemd/system/starter-api.service
rm -f /tmp/starter-api /tmp/starter-api.service
sudo systemctl daemon-reload
sudo systemctl enable starter-api
sudo systemctl restart starter-api
sudo systemctl --no-pager --full status starter-api
REMOTE
