# Honeycomb distribution

The application is `tos>starter` in the TOS organization. Honeycomb distributes the CLI; the Rust API remains on EC2 and the SolidJS website keeps its existing deployment. No runtime dependency on Honeycomb is needed.

## Release

Build and validate with `bash scripts/build-cli-release.sh` (prerequisites are in the repository README). The resulting archive contains a generated root `honeycomb.yaml` and six native binaries below `targets/`. `scripts/package-honeycomb.py` can package existing builds and runs the Honeycomb validator before succeeding.

Read the current configuration revision before upload:

```sh
honeycomb apps get 'tos>starter' --json
honeycomb --idempotency-key <unique-release-key> releases upload 'tos>starter' target/cli-release/starter-honeycomb-<version>.tar.gz --channel prod --revision <current-revision>
honeycomb install 'tos>starter' --version <version>
honeycomb publication get 'tos>starter' --json
```

Keep the exact accepted archive for retries; a release version is immutable. Increment `crates/cli/Cargo.toml` for a new release, refresh `Cargo.lock`, rebuild, and use a new idempotency key. Upload and public approval are separate operations. While private, installation requires IAM login with TOS access.

## Registration recovery

Starter already existed in IAM as `01a0ab75-a5df-7be2-813e-6b5d93604dae`, at accepted configuration revision 1 / IAM revision 7, but its Honeycomb catalog record was missing. The standard legacy importer only accepts revision zero. A constrained variant recovered the missing catalog entry from IAM's accepted snapshot, at the matching revisions and private visibility. It preserved the IAM identity, credentials, permissions, and other catalog entries. Preview and an in-memory rehearsal passed before the backed-up transaction; live Honeycomb reconciliation then accepted IAM revision 7.

- Recovery operation: `319dff81-bba3-4d65-8816-88e040bec445`.
- SSM execution: `3b90175c-1819-4bfe-8553-6ce16dd23bd3`, host `i-06986627793021fb2`, region `us-east-2`.
- Recovery script and metadata: `/var/lib/silicon-honeycomb/operator-starter/` on that host.
- Verified SQLite backup: `/var/lib/silicon-honeycomb/backups/starter-catalog-recovery/20260916T204017Z-00135ab3-67a8-4da7-9e2b-0265e7542b3a.db`.

Catalog copy is in `catalog.json`. Webhook secrets belong in protected runtime configuration, never in this directory or an archive. The existing receiver is `https://backend.starter.teamofsilicons.com/webhooks/iam`.

The complete configuration, including the existing deployed webhook signing secret, was subsequently submitted through `honeycomb apps update`. IAM accepted configuration revision 2 / IAM revision 13 (operation `2a8cb648-9f69-4923-a126-6095aaaa6ce3`). This supplies the webhook configuration required for normal publication planning.

## Release status

Version `0.2.0` is publicly available on the `prod` channel. It adds `.starterbase` recipes, `starter seed`, typed questions, generated-file merges, default publication previews, and private instance state. Failed template updates disable automatic updates; downloaded histories remain unpushable.

The six-platform archive is 20,562,932 bytes with SHA-256 `f5bb055963db9de2d733ed67dea50620e3194764cd952f19aeadd2444e7129f5`. Honeycomb release ID: `c3112648-0c2a-498d-9d23-c83b8ecb3cec`. GitHub release: [v0.2.0](https://github.com/teamofsilicons/silicon-starter/releases/tag/v0.2.0).

Public review `04dcce57-78fc-44ba-b481-4c1adf98b723` completed for configuration revision 2. The application is active and public; subsequent release uploads under this accepted revision publish directly.

```sh
honeycomb publication get 'tos>starter' --json
```

## Validation

All six release targets compile and Honeycomb validates their archive. Workspace tests, Clippy, formatting, and the CLI smoke check pass. The smoke check also passes against a fresh Honeycomb installation in an isolated home on ARM64 macOS. Windows, Linux, and Intel macOS binaries have not been executed on their native operating systems in this release check.
