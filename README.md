# Silicon Starter

A CLI-first registry for versioned silicon architectures. The repository ships a Rust API/CLI and a SolidJS web client.

## Install

Install on macOS (Apple Silicon or Intel) or Linux (ARM64 or x86_64):

```sh
curl -fsSL https://starter.teamofsilicons.com/install.sh | sh
```

[Release downloads](https://github.com/teamofsilicons/silicon-starter/releases/latest) include the installer and binaries. The installer selects your platform, verifies the download checksum, and puts `starter` on your PATH. It uses `/usr/local/bin` (with `sudo` when needed), or falls back to `~/.local/bin` and configures your shell’s PATH. Open a new terminal if the installer updates your shell configuration. Git is required for repository operations; Rust is not required.

The CLI connects to the production API at `https://backend.starter.teamofsilicons.com`. Override it with `--api http://127.0.0.1:8080` or `STARTER_API_URL=http://127.0.0.1:8080` for local development.

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

The CLI reads `STARTER_API_URL` (default `https://backend.starter.teamofsilicons.com`) and stores its IAM session, checkout registry, and webhook settings below `SILICON_HOME` (or `HOME`) in `.starter`. `starter update on` clears a release pin so a downloaded checkout can resume tracking the latest archive. Set `SPACE_STATION_TELEMETRY=0` to disable optional telemetry.

## Build CLI releases

On macOS with Xcode command-line tools, Rust, Zig, and cargo-zigbuild installed, run `bash scripts/build-cli-release.sh`. It builds all four platforms and writes the archives, installer, and `SHA256SUMS` to `target/cli-release` for a GitHub release.

Check a built binary with `python3 scripts/check-cli.py target/aarch64-apple-darwin/release/starter`; check the installer without changing your machine with `python3 scripts/check-install.py`.

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
