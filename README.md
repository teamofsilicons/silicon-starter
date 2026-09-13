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

## Install

The release installer will be published from GitHub once the first signed binary is cut:

```sh
curl -fsSL https://github.com/teamofsilicons/silicon-starter/releases/latest/download/install.sh | sh
```
