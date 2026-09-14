#!/usr/bin/env bash
# Run as root through SSM once on a new EC2 host.
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
aws s3 cp s3://silicon-starter-native-artifacts-vhcsnkljdtyn/releases/proxy-2.11.4.tgz "$work/proxy.tgz" --region us-east-1 --only-show-errors
printf '%s  %s\n' ca42d7b30b28a1e31571ff93a86f742803d9d32c686ce5622234f21cd129d2a6 "$work/proxy.tgz" | sha256sum -c -
tar -xzf "$work/proxy.tgz" -C "$work"
id caddy >/dev/null 2>&1 || useradd --system --home-dir /var/lib/caddy --shell /sbin/nologin caddy
install -m 0755 "$work/caddy" /usr/local/bin/caddy
install -d -m 0755 /etc/caddy
install -m 0644 "$work/Caddyfile" /etc/caddy/Caddyfile
install -m 0644 "$work/caddy.service" /etc/systemd/system/caddy.service
/usr/local/bin/caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl daemon-reload
systemctl enable caddy
printf 'Caddy installed. After API health and DNS are ready, run: systemctl start caddy\n'
