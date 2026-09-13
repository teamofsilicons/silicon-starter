# Silicon Starter

A CLI-first registry for versioned silicon architectures. The repository ships a Rust API/CLI and a SolidJS web client.

## Local

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd frontend && npm ci && npm run build
STARTER_BIND=127.0.0.1:8090 cargo run -p silicon-starter-api
```

The API starts with representative public starters and rejects any create request without a valid Stemcell `silicon.yaml`. Set `STARTER_API_URL` for the CLI. `starter iam --json` prints the app metadata; IAM SLTs are accepted by `starter login <SLT>` and exchanged only by a configured backend.

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
