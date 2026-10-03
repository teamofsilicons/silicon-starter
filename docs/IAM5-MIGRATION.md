# Starter IAM 5 migration

This release follows the [IAM 5 migration guide](https://docs.iam.teamofsilicons.com/migrating-to-iam-5/). It vendors `silicon-iam-client` 5.0.0 from reviewed IAM commit `f1e9c4768029aacabe337ca41be52e05023d1631`; package provenance is in `vendor/silicon-iam-client/VENDORED.md`. The application protocol remains compatible with IAM 5.2.1; deployment and integration evidence is recorded below.

## Ordinary sessions and contexts

Each login is bound to one canonical Carbon or Silicon, one organization, and one configured world. Ordinary token pairs and introspection must agree on that identity and carry an active singular IAM 5 authorization snapshot. Refresh cannot change the actor, organization or world. Ordinary login requests only `self.identity.read`, `self.profile.read`, and `self.membership.read`; Briefcase approval is separate.

The website offers Continue as Carbon and Continue as Silicon. A server-stored, single-use attempt binds the chosen kind, browser, callback state and validated return destination. The backend verifies the exchanged principal through authenticated introspection before setting a session. Popup messages carry only the attempt and completion status, and the opener checks the exact origin, popup window and attempt before reloading verified session state. Blocked popups use full-page login; canceled or declined login preserves the current work.

Browser credentials stay in encrypted server storage. A private browser group retains its own saved contexts, and logging out removes only the selected session. Public `context_id` markers fence browser and CLI requests and responses; a stale tab or delayed response cannot operate through a replacement account. CLI profiles retain independent credentials per API and world. See [CLI context and recovery commands](../crates/cli/README.md).

## Required backend configuration

| Setting | Purpose |
| --- | --- |
| `STARTER_IAM_APP_ID=starter` | Bare IAM application ID. |
| `IAM_URL` | IAM service; defaults to the production backend URL. |
| `STARTER_IAM_APP_SECRET` | Starter application credential for production IAM. |
| `STARTER_AUTH_FILE` | Durable encrypted ordinary-session and exchange-receipt file. Required for configured IAM login. |
| `STARTER_AUTH_ENCRYPTION_KEY` | Stable 32-byte encryption key represented by exactly 64 hexadecimal characters. Keep in the runtime secret manager. |
| `STARTER_DATABASE_URL` | Durable catalog PostgreSQL URL; `DATABASE_URL` is also accepted. |
| `BRIEFCASE_URL` | Briefcase receiver; defaults to its production backend URL. |
| `STARTER_IAM_TEST_APP_SECRET` and `STARTER_TESTING_ENVIRONMENT_KEY` | Both required to select a testing backend. Never sent by browser/CLI clients. |

Use HTTPS for service URLs. Loopback HTTP is permitted for isolated development. The EC2 installer requires the encryption key in the runtime secret and defaults to `/var/lib/starter/auth-iam5.enc`; it does not generate a new key on each deployment. Preserve the key with backups. Rotating it without re-encrypting or retiring matching state makes saved credentials and pending actions unreadable.

The ordinary store has a `STARTER-IAM5` encrypted envelope and an OS file lock. The adjacent `<STARTER_AUTH_FILE>.features.sqlite` database stores encrypted feature roots, refresh receipts, permission correlation, and immutable pending publication bundles; SQLite transactions and per-account leases fence concurrent processes. Back up both stores, the encryption key, and PostgreSQL as a coordinated set. Use SQLite's backup facility or stop writers before copying its database/WAL. These files are backend credentials, not browser assets. Pending publication state is retained for recovery and currently has no automatic retention cleanup.

Old plaintext/session-schema files are rejected without modification. Preserve them for rollback, choose a new path, and sign in again; never insert a schema marker or rewrite old opaque credentials to make them appear current. An empty backend without IAM configuration can still serve local public catalog data, but login and provider operations require configuration.

## Testing world isolation and catalog cutover

The backend resolves its configured world through IAM. Public world names are `production` or `testing:<environment-UUID>`. The full saved binding also includes environment version, key generation and clean time. Testing API reads, ordinary refresh, feature authority and persistence fail closed after these markers change. Restart with current testing configuration and sign in again. The CLI `--world` flag is an assertion about the backend, not a way to switch its credentials.

Catalog snapshots now use `starter_world_state`, keyed by the full world fingerprint. On first production startup only, the previous `starter_state` singleton is copied into the production row if no production row exists. Testing never adopts that singleton or a production snapshot; a cleaned testing generation starts with an empty catalog. The historical table is preserved and no longer receives current writes.

If historical author identifiers still require migration, complete the [identifier migration](PUBLIC-IDENTIFIER-MIGRATION.md) against the legacy singleton before the first IAM 5 startup. Its migration script does not edit the new world-partitioned table. Restore rejects legacy authors and world-mismatched snapshots rather than guessing their identities.

The catalog remains a single-instance in-memory snapshot backed by PostgreSQL. Do not run multiple independent catalog writers as a scaling strategy; the cross-process credential/feature locks do not make catalog snapshot writes distributed transactions.

## Explicit Briefcase permission and publication

Starter requests five receiver operations, all with audience `briefcase`: `briefcase.folders.create`, `briefcase.uploads.reserve`, `briefcase.uploads.commit`, `briefcase.uploads.status`, and `briefcase.link_access.update`. Register these exact external endpoints in Starter's IAM app metadata before testing. The application never approves IAM consent on the user's behalf.

1. `POST /api/v1/briefcase/authorization` with `{}` and a stable `Idempotency-Key` starts manual-code review.
2. `GET /api/v1/briefcase/authorizations/{request_id}` reads the original review.
3. `POST /api/v1/briefcase/authorizations/{request_id}/complete` with `{ "code": "..." }` and a retained idempotency key completes the reviewed request.

The website starts that same authorization with `{ "popup": true, "return_to": "/permissions" }`. The backend binds a random state, original account/context, IAM request ID and validated return destination to `/auth/briefcase/callback`. Only the configured IAM origin and exact consent request URL are opened. After code exchange and encrypted storage, the callback sends a status-only completion to its exact-origin opener. Decline keeps the original action; uncertain exchanges retain an encrypted code and the original mutation key behind a callback retry link. The popup remains open for that retry. The CLI retains the manual code flow.

All replies include `request_id`, `context_id`, full `authorization`, `completed`, and safe `roots` summaries. No provider token or test credential reaches the client. Decline/expiry requires a new review; HTTP 412 means the endpoint graph or terms changed. These outcomes preserve the ordinary login and pending publication. CLI/browser completion does not automatically publish.

A publication saves its original bundle bytes, commit, version, notes, owner context and request identity before requesting missing permission. It pins the approved Briefcase actor and organization before mutations. Retrying or reapproving cannot move that pending action to a different provider destination. A new push cannot replace its prepared bytes. For intentional changes, start a new action after reconciling any uncertain result.

Receiver mutations use the endpoint's reusable `X-IAM-OBO-Access-Token`, `X-App-ID`, and approved `X-Org-ID`; they do not use legacy HMAC/request proofs or forward the ordinary IAM bearer. A refresh retries the exact original provider bytes. Upload bytes use only the narrow upload capability (plus organization/testing selector). Lost commit responses are reconciled with the original upload operation's status before another mutation. The local release is recorded only after the provider confirms commit and public-link access.

The receiver must advertise service `silicon-briefcase`, API `v1`, revision `3.0.0` for the five JSON operations and revision `2.0.0` for capability byte transfer. An incompatible receiver is rejected before sending delegated credentials. Legacy `BRIEFCASE_APP_ID`, `BRIEFCASE_APP_SECRET`, and request-proof configuration are unused; Starter's IAM app credential owns its durable feature roots.

## Verification and coordinated rollout

Local checks:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
# With a disposable local PostgreSQL database, exercises actual partition SQL:
STARTER_TEST_DATABASE_URL=postgresql://localhost/starter_test cargo test -p silicon-starter-api postgres_partitions
```

The backend protocol suite covers ordinary exchange/refresh recovery, cross-context denial, provider consent/code and refresh replay after restart, decline and HTTP 412, immutable publication bytes, provider-destination fencing, capability-only transfer, lost commit recovery, receiver compatibility, encrypted state, and testing generation changes. The PostgreSQL test creates and removes a unique schema; without `STARTER_TEST_DATABASE_URL` it does not exercise PostgreSQL.

Before a live release, verify the IAM 5 ordinary and consent routes, reviewed endpoint registration, compatible Briefcase receiver, browser and CLI login for both actor kinds, same-token provider verification, testing clean/rotation, and an actual small publication. Confirm the pending-action recovery paths with the same logical request identities. These local tests do not establish live rollout readiness.

Stop writers for cutover and coordinated rollback. After IAM 5 writes, an old binary sees only the retained legacy catalog snapshot; ordinary binary rollback alone can hide newer records. Restore matching database, encrypted stores, configuration and binaries together, or reconcile newer writes and fix forward. Do not overwrite a published Honeycomb archive or claim deployment from local validation.

## Integration with the published Starter 0.3.0 feature set

The IAM 5 candidate includes public release `906d054ce7508644208d0147040bcbf3a77374f6`: versioned gene/ISI/function blocks, content-hash downloads, semantic discovery, authenticated developer pulls, and the centralized update registry/daemon. Its new block uploader uses the same five Briefcase endpoint roots as starter releases. Private blocks never enable a public link; public blocks require explicit link authorization after commit. The original bytes, metadata, provider destination and upload operation survive a declined review, lost reply and backend restart. No additional endpoint scope is introduced.

The browser saves a pending block in IndexedDB under the exact API/account context, retains it through reload, and offers a separate-tab storage review plus exact retry or local discard. The CLI retains one pending block per profile: `starter publish gene:example --retry` resends the saved bytes and metadata; `--cancel` discards only the local retry. Neither path silently adopts a replacement login. Background updates use each checkout's stored API, profile and world, while the single daemon and central enabled/disabled registry retain the released behavior.

Block publications require one stable `Idempotency-Key` (16–255 visible ASCII characters). Retrying retains that key and the exact saved request. An expired or cancelled provider upload remains attached to the old action; explicitly discarding it and publishing again creates a fresh key and upload operation. Changed metadata or content cannot reuse the old key, including metadata-only retries after the content is already stored.

## Release 0.3.1 validation and review status (3 October 2026)

The release incorporates the pushed `feat/iam5-contexts-20261003` candidate at `b1aac9d` and completes the current guide's Carbon/Silicon login selection, browser login and OBO popups, exact-origin completion validation, safe return destinations, and encrypted retry recovery. `/healthz` now reports the release version and IAM protocol.

Workspace tests, all 39 API tests with a disposable PostgreSQL database, Clippy, CLI/context/template/installer checks, TypeScript, 18 frontend tests, and the browser regression suite passed. The six native CLI targets build and the Honeycomb archive validates. A local release candidate also exchanged a real production Carbon SLT with IAM 5.2.1, introspected the single `tos` membership, rejected a mismatched context, and obtained an unapproved five-root Briefcase review. No personal OBO consent was approved or production content published during that check.

Honeycomb accepted configuration revision 4 / IAM revision 41 with the three ordinary IAM scopes and five Briefcase endpoints. Release `0.3.1`, ID `5fff3be5-9923-4fa1-af88-156423180bb1`, was accepted on the production channel (22,146,444 bytes; SHA-256 `8ba6fe57b2e425e4ca25639f0261a9ade337fe926f4019f815c8ffecac8f85a8`). Its publication request `31805a0a-8d99-422f-8319-87baa82be20a` awaits a Honeycomb validator at the [received requests page](https://console.honeycomb.teamofsilicons.com/requests/received). No provider permission approval remains pending, but the Honeycomb catalog and its downloads are private until validation. GitHub distribution is independent of this gate.

Live isolated testing remains incomplete: Honeycomb environment `e8ed6739-4ee4-4cdb-ba07-487c364b52c2`, import operation `20b67c3a-8fcb-45aa-87d1-06fd87a15c55`, has ready IAM, Honeycomb and Briefcase participants but Starter lacks the protected Honeycomb lifecycle participant endpoint. No testing credentials were delivered. This release supports a separately configured, generation-fenced testing backend; it does not implement dynamic Honeycomb-managed testing lifecycle operations. Do not mark that integration verified or acknowledge lifecycle operations without isolating their actual data. A real consent-approved storage publication and Silicon production login still require verification.
