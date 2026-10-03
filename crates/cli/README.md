# Starter CLI IAM5 contexts

Select a separate account/organization with `--profile <name>` or `STARTER_PROFILE`. Each profile keeps private sessions per API origin and world. `--world` / `STARTER_WORLD` defaults to `production`; testing deployments use `testing:<environment-UUID>`. The world is an assertion about the backend configuration, and the CLI does not send test application credentials.

```sh
starter --profile work login '<IAM SLT>'
starter --profile work login status --json
starter --profile test --api http://127.0.0.1:8080 --world testing:<UUID> --org tos login si:tester
starter --profile work login --recover
```

Login persists its idempotency key and exact SLT before I/O. Recover an interrupted exchange within ten minutes, or use `login --cancel` to discard the local retry receipt. Old plaintext `.starter/session` files are never sent; authenticate again to create a scoped session. Saved responses must include canonical Carbon/Silicon actor, one organization, public context ID and an unchanged world fingerprint. An organization flag can assert the selected organization but cannot retarget a bearer.

A checkout records its originating API, profile, world and context without credentials. Background updates use this origin through the one shared daemon. A replacement login does not adopt old jobs. `starter context show` inspects the origin; `starter context bind` explicitly binds the current checkout to the selected login. Legacy and explicitly anonymous checkouts make anonymous requests until explicitly bound. Session credentials and login/permission retry receipts are private files under `.starter/profiles`.

Briefcase permission is separate from login:

```sh
starter --profile work permission authorize
starter --profile work permission status
starter --profile work permission complete --code-file -
starter --profile work permission complete       # recover a saved uncertain completion
starter --profile work publish retry             # explicitly retry the saved publication
```

Open the returned HTTPS IAM review link and approve there. Completion validates the original account, organization, context, request ID and state. Retry keys and the exact code are saved before I/O; success clears the code. A decline, expiry or HTTP412 terms change requires fresh review. `permission cancel` discards only the local review. Approval never publishes automatically.

A failed or permission-blocked publication keeps its exact commit, release version, notes, context and idempotency key in private `.git/starter-publication.json`. `publish retry` uses those original bytes even if local HEAD changed. `publish cancel` discards the local receipt without undoing any remote result. Another account cannot resume it.

Verification: `cargo test -p starter -p silicon-starter-core`, `cargo clippy -p starter -p silicon-starter-core --all-targets -- -D warnings`, `python3 crates/cli/tests/iam5_contexts.py <built-starter>` and `python3 scripts/check-cli.py <built-starter>` run without production access.
