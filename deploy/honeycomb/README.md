# Honeycomb distribution

The application is `starter`, owned by organization `tos` in IAM. Honeycomb distributes the CLI; the Rust API remains on EC2 and the SolidJS website keeps its existing deployment. No runtime dependency on Honeycomb is needed.

New manifests use `app_id: starter`, starting with the local `0.2.2` release source. Application ownership is separate from the ID; bundle IDs such as `tos>interface` are unchanged. Use an identifier-compatible Honeycomb CLI as described in its [compatibility guide](https://docs.honeycomb.teamofsilicons.com/compatibility/).

## Release

Build and validate with `bash scripts/build-cli-release.sh` (prerequisites are in the repository README). The resulting archive contains a generated root `honeycomb.yaml` and six native binaries below `targets/`. `scripts/package-honeycomb.py` can package existing builds and runs the Honeycomb validator before succeeding.

Read the current configuration revision before upload:

```sh
honeycomb apps get starter --json
honeycomb --idempotency-key <unique-release-key> releases upload starter target/cli-release/starter-honeycomb-<version>.tar.gz --channel prod --revision <current-revision>
honeycomb install 'starter@<version>'
honeycomb publication get starter --json
```

Keep the exact accepted archive for retries; a release version is immutable. Increment `crates/cli/Cargo.toml` for a new release, refresh `Cargo.lock`, rebuild, and use a new idempotency key. Upload and public approval are separate operations. While private, installation requires IAM login with TOS access.

Existing release archives keep their original bytes, manifest IDs, versions, and checksums. Honeycomb's migrated catalog supplies the exact verified legacy manifest alias for those releases; do not repack an old version merely to replace `tos>starter`. Migrate existing local `installed.json` registries with Honeycomb's mapping tool and the approved IAM mapping while maintenance workers are stopped; retain package paths and command wrappers.

## IAM 5 release 0.3.1 (3 October 2026 UTC)

The API and website run IAM 5 code from `93b409bd50969f6201de634504f3428071f2d040`. The native API release is `/opt/starter/releases/53e82efea6be279a`, binary SHA-256 `6c3ed29f061985ebf2c133f28ad3877223d1a524c1467c17d9f4935b6bf8c469`. The stopped-writer cutover backup is `/var/lib/starter/backups/iam5-cutover/20261003T180530Z`. All three starters were preserved; production CLI Carbon login, browser callback exchange, anonymous browsing after old-cookie expiry, and stale-context denial passed.

Honeycomb accepted configuration revision 4 / IAM revision 41 and release `5fff3be5-9923-4fa1-af88-156423180bb1` on `prod`. Its immutable six-platform archive is 22,146,444 bytes, SHA-256 `8ba6fe57b2e425e4ca25639f0261a9ade337fe926f4019f815c8ffecac8f85a8`. The exact ordinary scopes and five Briefcase operations are effective, with no provider permission review pending.

Updating the app configuration made the Honeycomb catalog private pending validator request `31805a0a-8d99-422f-8319-87baa82be20a`. A validator must review it in [received requests](https://console.honeycomb.teamofsilicons.com/requests/received) before public Honeycomb downloads resume. [GitHub v0.3.1](https://github.com/teamofsilicons/silicon-starter/releases/tag/v0.3.1) is distributed independently. The historical public state below does not describe the current configuration.

Shared testing acceptance is incomplete because Starter lacks Honeycomb's protected lifecycle participant. See [migration validation and remaining acceptance checks](../../docs/IAM5-MIGRATION.md#release-031-validation-and-review-status-3-october-2026); do not report the pending environment as ready.

## Identifier release 0.2.2 (24 September 2026 IST)

[GitHub v0.2.2](https://github.com/teamofsilicons/silicon-starter/releases/tag/v0.2.2) is published from source commit `a0e4402ad0b6a4cc9f3c953577c633ebad648fdf`. The six-platform Honeycomb archive is 20,768,539 bytes, SHA-256 `fae2024770d29f3fef9b75e13de033943c27b214a928b68c7a2e26e40927797c`; accepted release ID `c2407fbb-d8fb-4073-9870-9b6fe61005c5` on `prod`.

The API identifier cutover is live in release directory `/opt/starter/releases/57d0c3509be89dea` on the existing Starter host. Its binary SHA-256 is `d278f85dfda67c33560111b72ab8f4b56b215be714a9fcaa77ea140a175f25e0`. The private backup at `/var/lib/starter/backups/identifier-cutover/20260923T202652Z` contains the full PostgreSQL dump, exact old environment/auth store, snapshot, and evidence. The database contained one repository and no discussions; its snapshot hash remained `dbd6244f892dda82ee10cf764169845a4bc57953e929773a3d6f2bc8b3f52d49`. Existing credentials and repository bytes were preserved; fresh Carbon login and organization-denial checks passed.

Vercel's project root is now `frontend`, so main-branch pushes deploy the website correctly. The production site serves the canonical login flow.

Honeycomb configuration revision 3 corrects the API base URL to `https://backend.starter.teamofsilicons.com`, preserving stored secrets and approved scopes. Public publication request `01fa936f-b2ff-476a-97a6-4a7e204b701a` subsequently completed: verification on 3 October 2026 found the validator approved, activation accepted, IAM revision 35, and the catalog active and public. Both Honeycomb and GitHub distribution are public.

## Historical registration recovery

This records the pre-cutover recovery under `tos>starter`; operation IDs, revisions, paths, and receipts below are historical evidence, not current configuration claims.

Starter already existed in IAM as `01a0ab75-a5df-7be2-813e-6b5d93604dae`, at accepted configuration revision 1 / IAM revision 7, but its Honeycomb catalog record was missing. The standard legacy importer only accepts revision zero. A constrained variant recovered the missing catalog entry from IAM's accepted snapshot, at the matching revisions and private visibility. It preserved the IAM identity, credentials, permissions, and other catalog entries. Preview and an in-memory rehearsal passed before the backed-up transaction; live Honeycomb reconciliation then accepted IAM revision 7.

- Recovery operation: `319dff81-bba3-4d65-8816-88e040bec445`.
- SSM execution: `3b90175c-1819-4bfe-8553-6ce16dd23bd3`, host `i-06986627793021fb2`, region `us-east-2`.
- Recovery script and metadata: `/var/lib/silicon-honeycomb/operator-starter/` on that host.
- Verified SQLite backup: `/var/lib/silicon-honeycomb/backups/starter-catalog-recovery/20260916T204017Z-00135ab3-67a8-4da7-9e2b-0265e7542b3a.db`.

Catalog copy is in `catalog.json`. Webhook secrets belong in protected runtime configuration, never in this directory or an archive. The existing receiver is `https://backend.starter.teamofsilicons.com/webhooks/iam`.

The complete configuration, including the existing deployed webhook signing secret, was subsequently submitted through `honeycomb apps update`. IAM accepted configuration revision 2 / IAM revision 13 (operation `2a8cb648-9f69-4923-a126-6095aaaa6ce3`). This supplies the webhook configuration required for normal publication planning.

## Historical release status

Version `0.2.1` was released publicly on the `prod` channel before the identifier cutover. The 0.2 release adds `.starterbase` recipes, `starter seed`, typed questions, generated-file merges, default publication previews, and private instance state. The patch makes local recipe update preferences authoritative and rolls back generated updates when their Git history commit fails. Failed template updates disable automatic updates; downloaded histories remain unpushable.

The six-platform archive is 20,768,494 bytes with SHA-256 `5c8854b12f211bd977bdefc712779c0c52c09358fe245faf737e9d5c4d48db62`. Honeycomb release ID: `318ff11a-7ca9-42e8-aaec-15ceeb164f60`. GitHub release: [v0.2.1](https://github.com/teamofsilicons/silicon-starter/releases/tag/v0.2.1).

Public review `04dcce57-78fc-44ba-b481-4c1adf98b723` completed for configuration revision 2. The application was active and public at that release. Read current publication and configuration state before another upload:

```sh
honeycomb publication get starter --json
```

## Historical release validation

All six release targets compile and Honeycomb validates their archive. Workspace tests, Clippy, formatting, and the CLI smoke check pass. The smoke check also passes against a fresh Honeycomb installation in an isolated home on ARM64 macOS. Windows, Linux, and Intel macOS binaries have not been executed on their native operating systems in this release check.
