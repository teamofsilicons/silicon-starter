# Silicon Starter

A CLI-first registry for versioned silicon architectures. The repository ships a Rust API/CLI and a SolidJS web client.

## Local

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix frontend ci
npm --prefix frontend run build
STARTER_BIND=127.0.0.1:8080 STARTER_FRONTEND_URL=http://127.0.0.1:5173 cargo run -p silicon-starter-api
```

In another terminal, run `npm --prefix frontend run dev -- --host 127.0.0.1` to open the web client. The browser regression check uses an isolated mock registry and requires Node 22+ and Chrome: `node frontend/src/browser-check.mjs`. Set `CHROME_BIN` if Chrome is installed outside the default macOS path.

The API loads saved starters from `STARTER_DATABASE_URL` (or `DATABASE_URL`). Without a database, local mode starts with an empty in-memory catalog; a configured but unavailable database stops startup. Creating a starter requires IAM authorization for an attached organization and a valid Stemcell `silicon.yaml`. Set `STARTER_API_URL` for the CLI. `starter iam --json` prints the app metadata; IAM SLTs are accepted by `starter login <SLT>` and exchanged only by a configured backend.

## CLI quick reference

```sh
starter download org.starter       # install with hourly updates
starter download org.starter@2.1   # install and pin a published release
starter pull org.starter            # editable checkout; never auto-updated
starter update on|off|now
starter publish latest 2.1 --notes "release notes"
starter publish history
starter revert <commit>
```

The CLI reads `STARTER_API_URL` (default `http://127.0.0.1:8080`) and stores its IAM session, checkout registry, and webhook settings below `SILICON_HOME` (or `HOME`) in `.starter`. `starter update on` clears a release pin so a downloaded checkout can resume tracking the latest archive. Set `SPACE_STATION_TELEMETRY=0` to disable optional telemetry.

## Install

The release installer will be published from GitHub once the first signed binary is cut:

```sh
curl -fsSL https://github.com/teamofsilicons/silicon-starter/releases/latest/download/install.sh | sh
```

## Backend on EC2

Production runs a static ARM64 Rust binary under `starter-api.service`. Caddy handles HTTPS; journald keeps logs. AWS Systems Manager provides shell access, with no SSH port exposed. PostgreSQL and IAM credentials are loaded from Secrets Manager into a root-only systemd environment file.

Build and manually deploy from this repository (requires Rust's ARM64 Linux target, cargo-zigbuild, Zig, and an authorized AWS CLI):

```sh
cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p silicon-starter-api
python3 deploy/ec2/deploy.py
```

The deploy uploads only the binary and service installer to private S3, verifies the checksum on EC2, switches an atomic release symlink, and restarts systemd. If deployment or health checks fail, it restores the previous binary, service unit, and environment file when a previous deployment exists.

Bootstrap Caddy once on a new host by sending the installer through SSM (replace the instance ID when rebuilding the stack):

```sh
aws ssm send-command --region us-east-1 --instance-ids i-0f507128879c0ed76 --document-name AWS-RunShellScript \
  --parameters "$(python3 -c 'import json,pathlib; print(json.dumps({"commands":[pathlib.Path("deploy/ec2/install-proxy.sh").read_text()]}))')"
```

The installer verifies the pinned native Caddy bundle from private S3. After the API is healthy and the backend DNS A record points to the instance's Elastic IP, run `sudo systemctl start caddy` in an SSM session:

```sh
aws ssm start-session --region us-east-1 --target i-0f507128879c0ed76
sudo journalctl -u starter-api -f
sudo systemctl restart starter-api
sudo journalctl -u caddy --since '1 hour ago'
```

Infrastructure is in `deploy/ec2/stack.yaml` (`silicon-starter-native`, us-east-1). The backend keeps its original domain and existing RDS database. There are no automatic deployments on git push, container images, or Rust source trees on the production host.
