# Silicon Apps distribution

The `starter` app is authored and published by `si:tos`. Silicon Apps distributes the CLI. The Rust API and SolidJS frontend keep their existing deployments. Accounts and publishing configuration are managed through [Silicon Developer](https://developers.teamofsilicons.com) and the `silicon-apps` CLI.

Build with `bash scripts/build-cli-release.sh` on macOS. This needs Xcode command-line tools, Rust targets, cargo-zigbuild, Zig, cargo-xwin, LLVM/lld, Python 3.11+, and Silicon Apps. It produces six native executables and a separate deterministic Silicon Apps archive for each target. Each archive has a root `apps.yaml` with `schema_version: 1`, `app_id: starter`, the release version, `command: starter`, and its target's `binary` path. `scripts/package-apps.py` can package existing native builds.

The binaries implement `--help`, `accounts --json`, and `login status --json`, including signed-out use. Silicon Apps controls installed CLI updates; Starter's daemon only updates downloaded project checkouts.

```sh
silicon-apps login status --json
silicon-apps capabilities --json
silicon-apps setup starter show --json
silicon-apps upload starter target/cli-release/starter-apps-0.4.0-linux-x86_64.tar.gz --target linux-x86_64 --json
silicon-apps release starter --version 0.4.0 --package <package-id> --notes 'Silicon Accounts and Silicon Apps migration' --json
silicon-apps promote starter <release-id> --version 0.4.0 --json
silicon-apps publish starter --json
silicon-apps install starter
```

Upload each supported target and repeat `--package` for all accepted package IDs when releasing. Check the current target worker availability before upload. A missing validation worker can prevent an otherwise valid native package from being accepted. Keep accepted archive bytes for retries, and use a stable idempotency key for each upload, release, promotion and publication operation. Published releases are immutable; increase `crates/cli/Cargo.toml` and refresh `Cargo.lock` for a new one.

Catalog copy is in `catalog.json`. Sign-in settings use the browser callback `https://starter.teamofsilicons.com/auth/callback`, frontend origin `https://starter.teamofsilicons.com`, and Accounts webhook `https://backend.starter.teamofsilicons.com/webhooks/accounts`. App secrets and webhook signing secrets belong only in protected backend configuration.
