# Silicon Accounts deployment

Starter 0.4 uses Silicon Accounts and Silicon Apps. Configure the `starter` application in [Silicon Developer](https://developers.teamofsilicons.com/apps/starter), authenticated as `si:tos`.

## Runtime configuration

- `STARTER_ACCOUNTS_APP_ID=starter`
- `STARTER_ACCOUNTS_APP_SECRET`: the new application's secret, stored only on the backend.
- `ACCOUNTS_URL=https://accounts.teamofsilicons.com`
- `STARTER_AUTH_FILE=/var/lib/starter/auth-accounts.enc`
- `STARTER_AUTH_ENCRYPTION_KEY`: stable 32-byte hex key, preserved across deployments.
- `STARTER_ACCOUNTS_WEBHOOK_SECRET`: signing secret for `/webhooks/accounts`.
- `STARTER_FRONTEND_URL=https://starter.teamofsilicons.com`
- `STARTER_DATABASE_URL`: existing PostgreSQL database.

Register `https://starter.teamofsilicons.com/auth/callback` and the required local development callback. Enable `device_flow` for Carbon CLI sign-in and `remember_browser` for hosted sign-in. Every authenticated request checks Accounts for session revocation. Webhooks update display handles without changing UUID ownership; delayed events never revoke a newer sign-in.

Briefcase publication uses fresh, operation-scoped Silicon Accounts User verification proofs. Keep Starter enabled in Briefcase's proof issuer configuration. No app secret or Accounts access token is sent to Briefcase.

## Catalog migration

Startup copies the previous production snapshot into `starter_accounts_state` if the new table is empty. Original tables remain available for rollback. Sessions use a new encrypted store; existing users must sign in once after the cutover.

Legacy catalog ownership requires an explicit verified mapping. `STARTER_OWNER_MIGRATION` is a JSON object mapping each previous owner label to `{"uuid":"immutable-account-uuid","id":"si:handle-or-c:handle"}`. The mapping applies only to entries without a UUID, and is saved with the catalog. Never resolve a legacy owner to an account solely because their current handles match. Unmapped entries remain readable according to their existing visibility and cannot be modified until mapped. Catalog IDs are preserved.

Back up the runtime secret and retain the previous release before deployment. Never commit credential files. Cross-platform release archives contain only `apps.yaml` and native binaries.
