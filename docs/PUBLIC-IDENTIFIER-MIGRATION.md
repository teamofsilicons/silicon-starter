# Starter identifier cutover

Starter uses application ID `starter`, owned by IAM organization `tos`. Carbon IDs are `c:<handle>` and Silicon IDs are `si:<handle>`. Organization selection stays separate. Starter repository IDs (`tos.example`), bundle IDs (`tos>interface`), resource UUIDs, Git commits, and release versions keep their meaning.

The live [IAM OpenAPI](https://docs.iam.teamofsilicons.com/openapi.yaml) defines these identifiers; some IAM prose examples still show the retired qualified IDs. The [Honeycomb package contract](https://docs.honeycomb.teamofsilicons.com/package-format/) and [compatibility guide](https://docs.honeycomb.teamofsilicons.com/compatibility/) describe canonical manifests and verified legacy archive aliases. Starter uses direct HTTP adapters, so there is no IAM SDK dependency to upgrade.

This procedure is for an operator's coordinated cutover. Source changes and local checks do not migrate production, publish releases, or change IAM/Honeycomb registrations.

## Prepare and stop writers

1. Obtain the authoritative IAM mapping for each world, including retained and removed discussion authors. Resolve global actor/application collisions in IAM first. Confirm `tos>starter` maps to `starter` owned by `tos`, and the Briefcase provider maps to `briefcase`; never select ownership from a handle alone.
2. Rehearse against a restored database. Back up the complete PostgreSQL database, deployment configuration, `STARTER_AUTH_FILE`, and local checkout/registry state. Preserve credentials separately.
3. Stop the Starter API and clients that publish or push. Reconcile any uncertain IAM exchange or Briefcase upload before changing audiences. Keep the exact original operation keys, request bytes, and receipts; do not resubmit an uncertain operation as new work.

## Migrate saved discussion authors

The database stores one JSON snapshot in `starter_state` at `id = 1`. Repository ownership is an organization ID, so it needs no actor-account rebinding. Only `discussions[*][*].author` is a stored public actor reference. The historical `authenticated-user` placeholder carries no actor identity and remains unchanged.

Translate the restricted IAM export to this format, retaining its exact `scope_key`. The example represents production; use the actual exported key for the selected world:

```json
{
  "scope_key": "production",
  "actors": [
    {"kind":"carbon","old_id":"alice0","new_id":"c:alice0"},
    {"kind":"silicon","old_id":"assistant:tos","new_id":"si:assistant","org_id":"tos"}
  ]
}
```

Include every retained author in that world's snapshot. The input is operator-verified authority, not a mapping inferred from the database. Keep it private with the backups. Use a separate database and map for each testing world; Starter has no mixed-world storage or automatic production fallback.

With Python 3 and `psql` installed, load the selected database URL securely into `STARTER_DATABASE_URL` (or `DATABASE_URL`/`PGDATABASE`). Preview first:

```sh
python3 scripts/migrate-identifiers.py --map /private/iam-starter-map.json --scope-key production
python3 scripts/migrate-identifiers.py --map /private/iam-starter-map.json --scope-key production \
  --apply --backup /private/starter-before-identifiers.json
```

The tool rejects missing mappings, collisions, wrong actor kinds, conflicting Silicon ownership, and a mismatched world. Apply requires a new private snapshot backup and compares the original snapshot in the database update, refusing a concurrent change. Repeating the same approved map is safe. Normal API startup refuses legacy discussion authors until this step succeeds.

The migration changes only author fields. Discussion IDs, parent links, bodies, organization ownership, catalog keys, YAML, Git bundle bytes, commit hashes, Briefcase entry UUIDs, and IAM webhook receipt IDs remain intact. Published repository files are user-authored artifacts: update outdated Silicon YAML or app selectors through normal source edits and a new release, never by rewriting historical archives.

## Configuration and sessions

- Set `STARTER_IAM_APP_ID=starter`. Retain its app secret and webhook signing secret. The API rejects qualified app IDs at startup and IAM request boundaries.
- If configured, `BRIEFCASE_APP_ID` is Starter's **calling credential ID**, so it is also `starter`; omitting it uses `STARTER_IAM_APP_ID`. Its existing secret must belong to that caller. The OBO audience is `briefcase`, and the selected organization comes from the authorized repository owner in the exchange body. `BRIEFCASE_ORG_ID` is no longer used. External scopes use `obo:briefcase:<endpoint_id>` with the existing endpoint IDs.
- Back up the old auth file and select a new, absent `STARTER_AUTH_FILE`. The EC2 installer defaults to `/var/lib/starter/auth-identifiers-v1.json` and preserves `/var/lib/starter/auth.json`. An explicit runtime-secret path overrides the default and must also be changed if it points to a legacy store.
- This cutover intentionally invalidates old Starter sessions. A legacy auth file is rejected without modification; do not add a schema marker to bypass this check. Browser users log in again. CLI users obtain a fresh IAM SLT for `starter` and run `starter login <SLT>`. Existing opaque session/token strings are never rewritten. Coordinate IAM-side revocation/expiry of old grants and refresh families with the IAM operator.
- Rebuild and deploy the API and frontend together. The browser's batch/bundle callback selects the `starter` entry only. New session files record identifier schema 1 and retain current credential bytes across restarts.

Stage the upgraded binary and configuration while the API is stopped, then start it under maintenance for verification. Do not use the ordinary `deploy/ec2/deploy.py` automatic rollback during this cutover: it can restart an old binary against migrated state. Keep a failed cutover stopped until coordinated restore or forward repair is complete; resume ordinary deployment only after the compatible release is established.

## Templates and Honeycomb

New templates render `silicon.id: si:<handle>` and a separate `silicon.org_id`, with bare `dm`, `briefcase`, and `waveform` app IDs. For existing instances, stop/disconnect the interpreter, back up YAML and `.starterbase/.state`, update the recipe, then reseed with the exact IAM mapping:

```sh
starter seed --defaults --set silicon_id=si:assistant --set silicon_org_id=tos
silicon compile /absolute/path/to/silicon.yaml
```

Keep the existing token, YAML path, `SILICON_HOME`, ISI names, and session directories. Reconnect that same configuration with the upgraded interpreter. Empty template defaults support previews; they do not create an IAM identity or a usable runtime login.

Build a new `0.2.2` CLI release with `app_id: starter`. Do not overwrite the accepted `0.2.1` archive or its checksum. Follow [Honeycomb publishing](../deploy/honeycomb/README.md); migrate each local Honeycomb `installed.json` with Honeycomb's approved-map tool while maintenance workers are stopped. Physical installation paths remain intact.

## Verify and resume

Run local regression checks:

```sh
cargo fmt --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/check-identifiers.py
cargo build -p starter --locked
python3 scripts/check-cli.py target/debug/starter
python3 scripts/check-template.py target/debug/starter
npm --prefix frontend run build
node frontend/src/browser-check.mjs
```

Exercise the actual SQL and backup workflow with `python3 scripts/check-identifiers.py --postgres-bin /path/to/postgresql/bin`. It starts and removes a disposable local PostgreSQL cluster, ignoring configured production database connections.

On the restored environment, verify Carbon and Silicon login, explicit organization authorization and cross-organization denial, private repository access, discussion author continuity, unchanged archives, and Briefcase publication with the canonical audience and selected organization. Recheck catalog ownership and Honeycomb installation in each world before reopening writes. Record the map, snapshot counts, unchanged resource IDs/hashes, migration counts, and deployed versions.

Before reopening writes, rollback restores matching database, auth file, configuration, IAM/Honeycomb state, and old binaries together. The ordinary EC2 deploy script's binary/configuration rollback is insufficient after a coordinated identifier cutover. After new writes, stop traffic and reconcile those writes before restoring, or fix forward.
