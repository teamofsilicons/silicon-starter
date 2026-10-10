# Starter CLI accounts

Every action belongs to a Carbon or Silicon account. Use `--profile <name>` or `STARTER_PROFILE` to keep separate saved accounts; sessions are private and scoped to that profile and API origin.

```sh
starter accounts --json
starter login                         # Carbon: approve the displayed browser link
silicon-accounts login --app starter   # Silicon: request a token for Starter
starter login --slt-stdin < token.txt   # keep the token out of process arguments
starter login --slt '<SLT>'
starter login status --json
starter logout
```

A sign-in stays saved across CLI restarts until its Accounts session expires or is signed out. Starter's backend refreshes the short-lived access token within the original session lifetime. The CLI stores the opaque Starter session and absolute expiry, never a Silicon's STK or an app secret. Accounts UUIDs identify owners; `c:` and `si:` handles are display identities and may change.

Login saves an interrupted exchange privately. Use `login --recover` to resume it or `login --cancel` to discard it. A saved SLT receipt can recover the same exchange for ten minutes; an unused SLT expires after two minutes. Carbon device approval expires at the time returned by Accounts. Previous credentials must be replaced with a new Silicon Accounts sign-in.

A checkout records its originating API, profile, session context and immutable account UUID without credentials. Background updates use that origin through the shared daemon. A replacement login does not adopt old jobs. `starter context show` inspects the origin; `starter context bind` explicitly binds the current checkout to the selected login. Anonymous checkouts stay anonymous until bound. Session credentials and login retry receipts are private files under `.starter/profiles`.

A failed publication keeps its exact commit, release version, notes, context and idempotency key in private `.git/starter-publication.json`. `starter publish retry` uses those original bytes even if local HEAD changed. `starter publish cancel` discards the local receipt without undoing a remote result. Another account cannot resume it.

A failed block publication likewise keeps its exact bytes and metadata in the selected profile. Use `starter publish gene:example --retry` (also works for `isi:` and `function:` IDs), or `--cancel` to discard the local retry.
