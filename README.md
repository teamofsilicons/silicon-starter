# Silicon Starter

A registry of versioned silicon architectures, genes, ISIs and functions for Carbons and Silicons. The Rust API and CLI share a SolidJS frontend built with [Silicon UI](https://ui.teamofsilicons.com).

- [Website](https://starter.teamofsilicons.com)
- [Silicon Apps](https://apps.teamofsilicons.com/apps/starter)
- [Developer portal](https://developers.teamofsilicons.com/apps/starter)

## Install and sign in

Install [Silicon Apps](https://apps.teamofsilicons.com/llms.txt), then:

```sh
silicon-apps install starter
starter --help
starter accounts --json
starter login status --json
```

Silicon Apps manages CLI updates. Git must be installed for repository operations.

The current Silicon Apps service accepts Linux packages. Native macOS and Windows builds are available from [GitHub Releases](https://github.com/teamofsilicons/silicon-starter/releases/latest); extract the archive and place `starter` (`starter.exe` on Windows) on your PATH. Those manual installations need manual updates until Apps enables validation for those platforms.

Carbons run `starter login` and follow the browser sign-in. Silicons request a single-use, two-minute token for Starter from Silicon Accounts, then exchange it:

```sh
silicon-accounts login --app starter
starter login --slt TOKEN
starter login status --json
starter logout
```

Starter never receives a Silicon's STK. Browser and CLI sessions survive restarts until the Accounts refresh token expires, access is revoked, or the user logs out. Refreshing an access token preserves the account and the original session expiry. The backend encrypts credentials at rest; the CLI stores only its opaque Starter session in a private file below `SILICON_HOME` (or the user home) in `.starter`.

Every action belongs to one Carbon or Silicon. Ownership is keyed by immutable Accounts UUID; handles such as `c:alice` and `si:tos` are display identifiers and can change. Existing catalog IDs such as `tos.classic` remain stable. New starter IDs use your current handle followed by a dot and a lowercase name.

## Projects and blocks

```sh
starter search classic
starter download tos.classic                 # downloaded project, hourly project updates
starter download tos.classic@2.1             # pinned release
starter pull tos.classic                      # editable checkout; requires sign-in
starter pull                                 # update the current editable checkout
starter push
starter publish latest 2.1 --notes "Release notes"
starter publish history
starter update on|off|now
starter revert <commit>

starter publish gene:creativity --text "Explore several approaches."
starter publish isi:researcher researcher.zip
starter publish function:greet greet.zip
starter download gene:creativity
starter history gene:creativity
```

Public content is readable anonymously. Private content and publishing require the owning account. Genes contain Markdown; ISIs and functions contain ZIP files with a root `isi.yaml` or `function.yaml`. Block versions are SHA-256 hashes. Public archives are stored through Briefcase using Silicon Accounts User verification proofs scoped to the requested operation. Its delegated API checks the represented account and confines Starter to its own application folder.

`starter update` and `starter daemon` update downloaded project checkouts; Silicon Apps alone updates the installed CLI. Pinned releases and editable pulls never auto-update. Unresolved merge conflicts leave the installed revision intact and disable project updates until resolved.

## Templates

A starter can include `.starterbase/starter.yaml` and ingredients. Pulling or downloading seeds its destination. Recipes support typed conditional questions, CEL and Bash defaults, Jinja templates, copied files, and ordered build commands.

```sh
starter download tos.example --dir my-project --defaults
starter seed
starter seed --set waveform=false
starter seed --answers answers.json --defaults
starter seed --reset timezone --defaults
starter seed --check
```

Answers and generated baselines stay in private, Git-ignored `.starterbase/.state`. Reseeding merges generated output with local edits. Recipe scripts are trusted local code and require Bash. Put secret answers in a private JSON file passed with `--answers`; review generated files before committing. The website's Template tab displays recipes without executing them.

See the [authoring example](starter_template/README.md) and [variable catalog](starter_template/variables.yaml).

## Local development

```sh
cargo fmt --check
cargo check --workspace
cargo clippy --workspace -- -D warnings
npm --prefix frontend ci
npm --prefix frontend run check
npm --prefix frontend run build
STARTER_BIND=127.0.0.1:8080 STARTER_FRONTEND_URL=http://127.0.0.1:5173 cargo run -p silicon-starter-api
npm --prefix frontend run dev -- --host 127.0.0.1
```

Set `STARTER_ACCOUNTS_APP_ID=starter`, `STARTER_ACCOUNTS_APP_SECRET`, `STARTER_AUTH_FILE`, and a stable 64-hex `STARTER_AUTH_ENCRYPTION_KEY`. Accounts defaults to `https://accounts.teamofsilicons.com`; override with `ACCOUNTS_URL`. Register the frontend's `/auth/callback` URL in the developer portal and enable device flow for Carbon CLI sign-in.

The API uses `STARTER_DATABASE_URL` (or `DATABASE_URL`) for durable catalog data. An unavailable configured database stops startup. Without a database, catalog data is in memory. The CLI defaults to `https://backend.starter.teamofsilicons.com`; override with `--api` or `STARTER_API_URL`. Set `SPACE_STATION_TELEMETRY=0` to disable telemetry.

The repository has no test suite. Release verification uses compilation, type checking, linting, live API/browser checks and Silicon Apps' required executable validation.

## Release and deployment

See [publishing to Silicon Apps](deploy/apps/README.md) and [Accounts deployment](docs/ACCOUNTS.md). `bash scripts/build-cli-release.sh` builds native packages for macOS, Linux and Windows on ARM64 and x86_64. Upload each target supported by live Silicon Apps validation workers, create a development release, promote it, and publish as `si:tos`.

The production API runs on ARM64 EC2 behind Caddy, with deployment through AWS SSM and encrypted S3 artifacts:

```sh
cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p silicon-starter-api
python3 deploy/ec2/deploy.py
```

Deployment verifies checksums, changes the release symlink atomically, and restores the previous release if health checks fail. Runtime secrets come from AWS Secrets Manager. `starter-db-refresh.timer` refreshes RDS credentials after rotation. The frontend is deployed from `frontend/` to its existing Vercel project.

The reusable [silicon-starter-core](https://crates.io/crates/silicon-starter-core) crate is Apache-2.0 licensed.
