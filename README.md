# Silicon Starter

A CLI-first registry for versioned silicon architectures. The repository ships a Rust API/CLI and a SolidJS web client.

## Install

Install on macOS (Apple Silicon or Intel) or Linux (ARM64 or x86_64):

```sh
curl -fsSL https://starter.teamofsilicons.com/install.sh | sh
```

[Release downloads](https://github.com/teamofsilicons/silicon-starter/releases/latest) include the installer and binaries. The installer selects your platform, verifies the download checksum, and puts `starter` on your PATH. It uses `/usr/local/bin` (with `sudo` when needed), or falls back to `~/.local/bin` and configures your shell’s PATH. Open a new terminal if the installer updates your shell configuration. Git is required for repository operations; Rust is not required.

With [Honeycomb](https://docs.honeycomb.teamofsilicons.com/installation/) installed and access to `tos>starter`, install the six-platform package (macOS, Linux, or Windows; ARM64 or x86_64):

```sh
honeycomb install 'tos>starter'
starter --help
```

Honeycomb manages CLI installation and binary updates. `starter update` and `starter daemon` manage downloaded project checkouts. Git must already be on PATH; Honeycomb packages do not run setup scripts.

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
starter pull                        # update the current checkout using its saved starter id
starter update on|off|now
starter publish latest 2.1 --notes "release notes"
starter publish history
starter revert <commit>
```

The CLI reads `STARTER_API_URL` (default `https://backend.starter.teamofsilicons.com`) and stores its IAM session, checkout registry, and webhook settings below `SILICON_HOME` (or the operating system's user home) in `.starter`. `starter update on` clears a release pin so a downloaded checkout can resume tracking the latest archive. Set `SPACE_STATION_TELEMETRY=0` to disable optional telemetry.

## Template starters

A starter can optionally include `.starterbase/starter.yaml` and its ingredients. `starter pull` and `starter download` seed a new checkout automatically; starters without `.starterbase` keep their existing behavior.

```sh
starter pull tos.example --dir my-silicon
starter download tos.example --dir installed-silicon --defaults
starter seed                         # reconfigure using saved answers
starter seed --set waveform=false    # change a typed answer
starter seed --answers answers.json --defaults
starter seed --reset timezone --defaults
starter seed --check                 # validate without running commands
```

Interactive installs ask for a destination unless `--dir` is supplied. Occupied folders are rejected. Noninteractive runs use saved answers and defaults; `--defaults` also suppresses terminal questions. `--set` accepts JSON values or plain strings. Put secret answers in a private JSON file passed with `--answers` to avoid shell history.

Recipes support typed, conditional questions; CEL and Bash defaults with literal fallbacks; Jinja templates and includes; copied files; and ordered build commands. All preparation runs without terminal input. Build scripts receive `STARTER_SOURCE`, `STARTER_OUTPUT`, `STARTER_PROJECT`, `STARTER_INTERACTIVE`, and active `STARTER_VAR_*` answers. Recipe scripts are trusted local code and require Bash (on Windows, install Git Bash and put `bash` on PATH). Only generated output is managed transactionally; scripts own any external side effects.

Answers and the pure generated baseline live in private, Git-ignored `.starterbase/.state`. Reseeding and downloaded updates merge the old generated baseline, local edits, and newly generated files. Unrelated files remain intact. A failed build or unresolved update conflict leaves the installed revision intact and disables automatic updates; fix the problem and use `starter update on` to resume. Missing state in an existing instance requires recovery or reconciliation.

The recipe's `auto_update` initializes downloads and defaults to `false`. Local settings survive upgrades. Pinned downloads and developer pulls never auto-update. Checkout modes remain fixed: create a separate `--dir` when moving between development and downloaded instances. First developer seeding leaves personal configuration uncommitted; review generated files before committing credentials to any publishable Git history.

`starter push` validates the committed recipe and builds a default preview in a disposable checkout. Only after that succeeds does it add the default-preview commit to the author checkout and upload. Saved personal answers stay local and can be restored with `starter seed`. Commit source changes before pushing. The website's Template tab shows questions, conditions, defaults, build flow, and source ingredients without executing scripts.

See [the working authoring example](starter_template/README.md) and [the variable catalog](starter_template/variables.yaml). Run `python3 scripts/check-template.py target/debug/starter` for the isolated template lifecycle check.

## Build CLI releases

On macOS with Xcode command-line tools, Rust, Zig, cargo-zigbuild, cargo-xwin, LLVM/lld, Python 3.11+, and Honeycomb on PATH, run `bash scripts/build-cli-release.sh`. It builds all six native targets and writes the four standalone macOS/Linux archives, installer, validated `starter-honeycomb-<version>.tar.gz`, and `SHA256SUMS` to `target/cli-release`. Windows binaries use the static MSVC runtime. The packager reads the version from `crates/cli/Cargo.toml` and includes only the manifest and six binaries.

Check a built binary with `python3 scripts/check-cli.py target/aarch64-apple-darwin/release/starter`; check the installer without changing your machine with `python3 scripts/check-install.py`.

See [Honeycomb publishing](deploy/honeycomb/README.md) for upload, review, and registration details. The API/frontend structure and hosting do not change for Honeycomb distribution.

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
