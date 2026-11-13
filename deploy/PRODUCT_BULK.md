# Product imports

Admission defaults to `PRODUCT_BULK_ENABLED=false` in both environments. Enable it only after the [verification gates](../apps/api/docs/api/product-bulk-verification.md), normal production optimization build and staging checks pass. Use the existing green-main CI image publication and deployment flow.

## Runtime configuration

`deploy/deploy.sh <environment> compose …` automatically adds `docker-compose.product-bulk.yml` when admission is enabled or `${SECRETS_DIR:-./secrets}/anthropic_api_key` exists. `SECRETS_DIR` is relative to `deploy/<environment>` unless absolute. An enabled deployment requires a nonempty key file and the overlay. Only unset, `false` and `true` are accepted flag values.

The overlay mounts the key only in the API and sets `ANTHROPIC_API_KEY_FILE=/run/secrets/anthropic_api_key`. The API's native workers make extraction calls; Flow, Office, migration and seed containers do not receive this credential. The uploads overlay continues to supply the environment's own private file storage independently.

At an authorized deployment, copy that environment's already approved key into its own `anthropic_api_key` file without printing it, putting it in arguments or enabling shell tracing. Do not source an env file as shell code. Keep the secrets directory private to the operator (mode 700). A mode-600 key file must be owned by UID 65532 so the non-root API can read the Compose file-secret mount; mode 600 owned by the deploy user is unreadable inside that container. Use the same runtime-readable ownership when rotating it.

An existing mode-600 operator env file may retain `ANTHROPIC_API_KEY`: Compose uses that file for interpolation, and this overlay passes only `_FILE` into the API. Do not add a plaintext `ANTHROPIC_API_KEY` container variable or use `env_file:` to pass the whole operator file. The API rejects simultaneous key and key-file configuration. Never copy a production key or storage configuration into staging.

## Enable and pause

1. Prepare the approved key file and existing private uploads configuration in the correct environment. Keep admission false while deploying and checking the new images/migrations. Refresh the installed ops kit from the reviewed commit when releasing this shared-library change so backup/restore/watch helpers use the same overlay chain as `deploy.sh`.
2. Complete staging verification, including live extraction under an explicitly approved spend cap. Workspace extraction caps default to zero; admission alone does not authorize provider spend or product writes.
3. After the release gates pass, set `PRODUCT_BULK_ENABLED=true` in that environment's operator env file and apply the API through the normal deployment workflow. Production uses the same reviewed commit already verified on staging.
4. To pause new runs, set `PRODUCT_BULK_ENABLED=false` and apply the API configuration through `deploy.sh`. Keep the key file and uploads key in place. The overlay remains attached so existing jobs, reconciliation and retention cleanup keep their configuration. Existing approved mutations retain their own transaction, permission and approval checks.

Do not remove credentials or storage access as a pause mechanism. Keep the newer worker binaries running when fixing a failed import; receipts and migration compatibility govern recovery.

Configuration regression checks use synthetic secrets only:

```sh
bash deploy/deploy.test.sh
bash deploy/compose.test.sh
```
