# Production

Office assistant stays disabled by default. See [assistant deployment](../ASSISTANT.md) for runtime secrets, restricted checkpoint roles, readiness and compatible rollback.

Product imports stay disabled by default. See [product import deployment](../PRODUCT_BULK.md) for release gates, file credentials and pausing existing workers safely.

The live Genesy stack on the VPS. It runs the API with its own Postgres, the apps (shop, office, till, field, flow) and the landing page, all served through the existing Traefik.

It is a separate compose project, `genesy-prod`. It never touches the `control360` or `genesy-prod-green` projects, and its router names (`genesy-production-*`) don't clash with theirs.

| Host                  | Service                                                 | Container port |
| --------------------- | ------------------------------------------------------- | -------------- |
| `api.genesy.app`      | api                                                     | 8080           |
| `shop.genesy.app`     | shop                                                    | 3000           |
| `office.genesy.app`   | office                                                  | 3000           |
| `till.genesy.app`     | till                                                    | 3000           |
| `flow.genesy.app`     | flow                                                    | 3000           |
| `app.ruahaenergy.com` | field                                                   | 8080           |
| `field.genesy.app`    | field (301 to `https://app.ruahaenergy.com`, path kept) | 8080           |
| `genesy.app`          | landing                                                 | 3000           |
| `www.genesy.app`      | landing (301 to `https://genesy.app`)                   | 3000           |

**How it differs from staging** (`../staging/README.md`):

- **No Mailpit.** Mail goes to the real SMTP server in `secrets/smtp_url`.
- **No fake payments.** The API image is the default build, which refuses `PAYMENTS_PROVIDER=fake`. Web payments answer 503 until Stripe is turned on ("Turning on Stripe").
- **Production payment realm.** Compose pins `GLOBADYME_ENVIRONMENT=production` for the API, workers and CLI. Workspace settings cannot select sandbox; Globadyme remains blocked until its official production API origin is verified.
- **No demo seed, no self-signup.** `GENESY_ALLOW_DEMO_SEED` is unset and `SIGNUP_ENABLED=false` (also the API default). The first account comes from `create-owner`; everyone else is invited.
- **External Postgres volume.** It is called `genesy-prod-pg` and you make it by hand, so no `down -v` can delete it.
- **No `noindex` header, no IP allow-list.**
- **Field release flag off.** `VITE_FEATURE_RESOURCES=off` hides equipment, loans and requests; no demo banner.
- **Uploads in the production B2 bucket, once its key is in** ("Turning on file uploads"); until then, and always on staging, file routes answer 503. No PDF renderer yet: those routes answer 503 too.

Every step below is for the user to run on the VPS, or for the coordinator once the user says go. VPS facts and DNS records: `../staging/README.md` ("The VPS", "DNS records"). Production needs the `api`, `shop`, `office`, `till`, `field` and `flow` records, plus the apex (`@`) and `www` for the landing page. The field app's own host, `app.ruahaenergy.com`, is an A (`185.77.96.44`) and AAAA (`2a02:4780:d:ca37::1`) record named `app` in RUAHA's GoDaddy zone `ruahaenergy.com`; that zone's other records (apex and `www` on Webflow, mail) are not ours. The landing page's contact address is `CONTACT_EMAIL` in `.env.production` (default `hello@genesy.app`, a placeholder).

## Files

| File                            | What                                                                 |
| ------------------------------- | -------------------------------------------------------------------- |
| `docker-compose.production.yml` | The stack, with Traefik labels                                       |
| `docker-compose.stripe.yml`     | Turns Stripe on; `deploy.sh` adds it once the Stripe key exists      |
| `docker-compose.uploads.yml`    | Turns file uploads on; `deploy.sh` adds it once the B2 key exists    |
| `../deploy.sh`                  | Deploy, rollback, prune, and `compose` with the right `-f` chain     |
| `../ops/`                       | Backups, restore tests, alerts (`../ops/README.md`)                  |
| `../set-provider-keys.sh`       | Writes Stripe or Resend keys into `secrets/` without echoing them    |
| `.env.production.example`       | Every variable, no secrets. Copy it to `.env.production` on the VPS. |
| `secrets/`                      | Secret files on the VPS only. Git ignores everything in it.          |
| `../images/*`                   | Web images, shared with staging                                      |
| `../../apps/api/Dockerfile`     | API image (default build, no build args)                             |

## One-time setup (on the VPS, as user `deploy`)

```bash
# 1. A deploy checkout, separate from staging and CI
sudo mkdir -p /srv/genesy-prod && sudo chown deploy /srv/genesy-prod
git clone git@github.com:COTIT/flow.ai.git /srv/genesy-prod
cd /srv/genesy-prod/deploy/production
```

```bash
# 2. Secret files (random, made on the VPS, never copied anywhere)
chmod 700 secrets
pw="$(openssl rand -hex 24)"
printf '%s' "$pw" > secrets/db_password
printf 'postgres://genesy:%s@db:5432/genesy' "$pw" > secrets/database_url
openssl rand -hex 32 | tr -d '\n' > secrets/credentials_key
unset pw
# The mail provider's SMTP URL (TLS: smtps://… or smtp://…?tls=required).
printf '%s' 'smtps://USER:PASSWORD@HOST:465' > secrets/smtp_url
chmod 444 secrets/*   # containers run as non-root users; the 700 folder keeps other users out
```

Mail goes through Resend ("Turning on Resend"). Until its key is in, put `smtps://unset.invalid:465` in `smtp_url`: the API boots, and mails fail and stay in the outbox.

```bash
# 3. The database volume (external: compose never creates or deletes it)
docker volume create genesy-prod-pg
```

```bash
# 4. Env file (Traefik values are already filled in; check MAIL_FROM)
cp .env.production.example .env.production && chmod 600 .env.production
```

```bash
# 5. Read-only GHCR login (token: ../staging/README.md "GHCR pull token"). Skip if done for staging.
#    One login serves staging and production: both run as user deploy.
install -d -m 700 ~/.config/genesy && install -m 600 /dev/null ~/.config/genesy/ghcr-token
nano ~/.config/genesy/ghcr-token   # paste the token, save; it never goes on a command line
docker login ghcr.io -u <token owner> --password-stdin < ~/.config/genesy/ghcr-token
```

```bash
# 6. The ops kit: nightly backups, restore tests, alerts, the sudo rule deploy.sh needs
cd /srv/genesy-prod && sudo bash deploy/ops/setup.sh
```

Back up `secrets/credentials_key` off the VPS, for example in the password manager. It encrypts the workspace mail credentials stored in the database. If it's lost, those credentials can't be read, and a backup can't be restored with them.

## Deploy from main (after CI is green)

Image preparation runs alongside checks and pushes run-specific candidates. The existing
`images` checks publish final commit tags only after every quality check, test shard and image
producer succeeds. Deploy only a fully green main run. Failed-job reruns can reuse successful
producer manifests from that same run; missing or expired manifests require a full rerun.
Candidate tags are not deployable commit SHAs. Never deploy a partial publication.

Deployment timings go to stderr and `deploy/production/deploys.log`. Include backup,
migration, health and cleanup time when comparing releases. Human waiting between promotions
is measured separately.

Only deploy a commit whose `ci` workflow passed on `main`, including its two `images` jobs (api and web), which pushed `ghcr.io/cotit/genesy-<app>:<sha>`, and that staging already runs. The VPS can't reach GitHub's git, so the commit comes over in a git bundle:

```bash
# On the laptop
git fetch origin && git bundle create /tmp/genesy.bundle origin/main
scp /tmp/genesy.bundle genesy:/tmp/
```

```bash
# On the VPS, as deploy
cd /srv/genesy-prod && deploy/deploy.sh production <green-main-sha> --bundle /tmp/genesy.bundle
```

`deploy.sh` does, in order, and stops at the first failure:

1. Takes a lock (one deploy at a time), checks there are 5 GB free, and checks the commit out.
2. Writes its full sha into `GENESY_REF` in `.env.production`: the images' tag.
3. Pulls that commit's images from GHCR (`IMAGE_SOURCE=pull`, the default). The old containers keep serving meanwhile.
4. Takes a backup (`pre-<sha>`, through `/usr/local/lib/genesy-ops/backup.sh`).
5. Runs the migrations once, in a one-off `cli` container (`genesy-360-api migrate`). The app containers never migrate: `SKIP_BOOT_MIGRATIONS=true`.
6. `up -d --wait --no-build`, then checks every host through Traefik: `api` `/healthz` must give 200, the apps 2xx/3xx. `--no-build` means the VPS never builds by accident: a missing image stops the deploy.
7. Records the live and previous versions in `.deploy-state`, adds a line to `deploys.log`, and prunes images.

A commit already in the checkout needs no `--bundle`. If the health check fails, the new version stays up and the script prints the rollback command. Nothing rolls back on its own: someone decides.

The prune keeps the images of the live and previous version of **both** stacks: staging and production share the GHCR repositories, and the shop, till, landing and flow images even share their tag. Never run `docker image prune -a`.

## Rollback

```bash
deploy/deploy.sh production rollback
```

For releases without the assistant fingerprint, this switches back to the previous commit, `GENESY_REF` and retained images only when the database schema still fits. Assistant releases use the reviewed compatibility procedure in [assistant rollback and recovery](../ASSISTANT.md#rollback-and-recovery); unreviewed migrations stop binary rollback.

For conversation continuity (`20261113121048`), set `ASSISTANT_ENABLED=false` and restart the affected services on the new binaries to pause new sessions. Keep the new Office webhook and Rust ingestion, retention and deletion workers running, with their credentials, while deploying a fix forward. Older binaries lack these workers; the migration stays outside the compatible rollback allowlist. Do not reverse the migration or use `--restore-db` for routine release rollback. Database restore is separate disaster recovery and loses writes since the selected dump.

## Fallback: build on the VPS

Only when the `images` jobs or GHCR are down. Set `IMAGE_SOURCE=build` in `.env.production` and deploy as above: `deploy.sh` builds the images here instead of pulling them. That takes 10–30 minutes and runs on prod's Docker. Set it back to `pull` afterwards.

After a build, the prune caps the build cache at 10 GB (`docker builder prune --max-used-space 10GB`) instead of wiping it. The kept cache makes the next build fast: an unchanged API is a cache hit, a changed one rebuilds only its own crate, and the web images reuse their package store.

## Resource limits

Every container has a CPU, memory and process ceiling (`deploy.resources.limits`). Without one, a runaway container could take the whole VPS, which it shares with staging, CI and other projects. These are ceilings, not reservations.

| Service                           | CPUs     | Memory                   |
| --------------------------------- | -------- | ------------------------ |
| db                                | 1.5      | 2 GB (`shm_size` 256 MB) |
| api                               | 1.5      | 1 GB                     |
| shop, office, till, landing, flow | 0.5 each | 512 MB each              |
| field                             | 0.25     | 128 MB                   |

Staging runs smaller limits with a quarter of production's CPU weight (`cpu_shares: 256`), so production wins when both are busy. A container that goes over its memory is killed and restarted (`restart: unless-stopped`). The `vps-watch` alert reports that as a restart loop. If it happens, raise that service's limit here. Check real use with `docker stats --no-stream`.

## Flow (`flow.genesy.app`)

Flow is its own Next container (`flow`, image `ghcr.io/cotit/genesy-flow:<sha>`, built once and shared by staging and production like till). Its server forwards `/api/voice/*` to Office over the `internal` network at `http://office:3000`; the voice agent, ElevenLabs key, bridge secret and checkpoint database stay in Office (`../ASSISTANT.md`). Flow itself takes only `API_URL`, `VOICE_UPSTREAM_URL`, (`https://office.genesy.app`, where the assistant's Office links open) and `SHOP_PUBLIC_URL` (`https://shop.genesy.app`, where full-page storefront preview links open), both read per request, not at build,, no secret, and no variable in `.env.production`. Office keeps its one public host (`office.genesy.app`); Flow adds no public route to it. When Office is down or its assistant is off, Flow's voice calls fail and the rest of Flow keeps answering.

Before the first deploy of a commit that contains Flow:

1. **DNS first.** Add the `flow` A and AAAA records (staging README, "DNS records") and wait until they resolve. Without them the deploy's health check still passes (it asks the local Traefik with `--resolve`), but Let's Encrypt keeps failing for that host and `vps-watch` alerts on its missing certificate on every run.
2. **GHCR package.** The first green `images` run after the merge creates `genesy-flow`. Check its settings: visibility **private**, "Manage Actions access" lists `COTIT/flow.ai` with **Write**, and the machine account has **Read**. Without Read the VPS pull fails and the deploy stops before its backup and migrations.
3. **Rollback past Flow.** A rollback to a commit from before Flow leaves the newer `flow` container running (compose does not remove services the older file lacks). Stop it by hand: `deploy/deploy.sh production compose stop flow`.
4. **Memory.** Flow adds one Next container per environment: ceilings of 384 MB on staging and 512 MB on production ("Resource limits" above).

If the VPS still has an untracked `docker-compose.nodns.yml`, give `flow` the same `traefik.enable=false` entry as the other apps.

## Running compose by hand

Always go through `deploy.sh`, never a bare `docker compose`:

```bash
deploy/deploy.sh production compose ps
deploy/deploy.sh production compose logs --tail 100 api
```

It adds `docker-compose.nodns.yml` while that file exists, `docker-compose.stripe.yml` once `secrets/stripe_secret_key` exists, and `docker-compose.uploads.yml` once `secrets/b2_uploads_key` exists. A command that forgets one of those recreates the API without it. The commands below all use it.

## First owner

Self-signup is off, so the first account is made from the command line, once:

```bash
deploy/deploy.sh production compose --profile cli run --rm cli create-owner --email <email> --workspace <name>
```

This runs `genesy-360-api create-owner`. It creates the owner and their workspace, and prints a one-time password **once**. Hand that password to the owner over a private channel. Everyone else is invited from the office.

`cli` is its own service with no Traefik labels, so the one-off container never receives web traffic.

**The shop's app key:**

1. Log in to `office.genesy.app`.
2. Make an app key with scope `catalog:read`.
3. Put it in `STOREFRONT_APP_KEY` in `.env.production`.
4. Run `deploy/deploy.sh production compose up -d shop`.

## Backup and restore

Nightly dumps, a daily restore test and the live-restore steps are in `../ops/README.md`. A dump by hand:

```bash
sudo /usr/local/lib/genesy-ops/backup.sh production
```

## Turning on Stripe

Payments answer 503 until this is done. Code and contract: `apps/api/docs/api/payments.md`. One Stripe account serves every workspace.

1. **Stripe dashboard, live mode:**
   - Developers → API keys: copy the secret key (`sk_live_…`), or make a restricted key (`rk_live_…`) with Checkout Sessions and Refunds set to Write.
   - Developers → Webhooks → Add endpoint:
     - URL `https://api.genesy.app/v1/payments/webhooks/stripe` (one endpoint for every workspace; no workspace in the path).
     - Events `checkout.session.completed`, `checkout.session.async_payment_succeeded`, `checkout.session.async_payment_failed`, `checkout.session.expired`, `refund.updated`, `refund.failed`.
     - Copy its signing secret (`whsec_…`).
   - Settings → Payment methods: turn on what the shop offers (cards, iDEAL, Bancontact, SEPA debit). Checkout shows what is on there.
2. **On the VPS:** write the keys. They are asked for without echo, so they never land in shell history.

   ```bash
   cd /srv/genesy-prod && deploy/set-provider-keys.sh production stripe
   ```

3. **Restart the API.** `deploy.sh` adds `docker-compose.stripe.yml` by itself once `secrets/stripe_secret_key` exists:

   ```bash
   deploy/deploy.sh production compose up -d --wait api
   ```

4. **Check it:** `deploy/deploy.sh production compose logs api | grep -i payments` no longer shows `PAYMENTS_PROVIDER not set`. Pay one small real order in the shop, refund it from the office, and look for both in the Stripe dashboard (Payments, and Webhooks → the endpoint's deliveries, all 200).

A wrong or missing key stops the API at boot with a message that names the variable, never the value. To turn Stripe off, move `secrets/stripe_secret_key` and `secrets/stripe_webhook_secret` out of `secrets/` and restart the API the same way. To roll a key, run the script again and restart.

## Turning on Resend (mail)

No code: Resend takes SMTP, so `secrets/smtp_url` becomes `smtps://resend:<API key>@smtp.resend.com:465`.

1. **Resend dashboard → Domains → Add domain** `genesy.app`. Resend then lists the DNS records to add. Copy the values from the dashboard: the DKIM key is unique, and the MX host depends on the region you pick.

   | Type | Name (zone `genesy.app`) | Value (from the dashboard)                          | Notes                 |
   | ---- | ------------------------ | --------------------------------------------------- | --------------------- |
   | MX   | `send`                   | `feedback-smtp.<region>.amazonses.com`, priority 10 | bounces (return path) |
   | TXT  | `send`                   | `v=spf1 include:amazonses.com ~all`                 | SPF                   |
   | TXT  | `resend._domainkey`      | `p=MIGfMA0…` (long key)                             | DKIM                  |
   | TXT  | `_dmarc`                 | `v=DMARC1; p=none;`                                 | optional, recommended |

   Add them in OpenProvider and press Verify in Resend; it can take up to a few hours. If `_dmarc` already exists, keep the existing one.

2. **Resend → API Keys:** make a key with Sending access, limited to `genesy.app`.
3. **On the VPS:** `deploy/set-provider-keys.sh production resend`. It rewrites `secrets/smtp_url` without echoing the key.
4. `MAIL_FROM` in `.env.production` must use the verified domain (the example already says `Genesy <no-reply@genesy.app>`).
5. Restart the API: `deploy/deploy.sh production compose up -d --wait api`. Then invite yourself from the office and check that the mail arrives, and that Resend → Emails shows it as delivered.

## Turning on file uploads

Product images, documents and farm photos go to the existing production B2 bucket, under `genesy/<workspace>/…`. Until this is done, file routes answer 503. Staging has its own bucket and key (`../staging/README.md`, "Turning on file uploads"), so demo files never land in the production bucket.

The bucket gets **two keys that can't see each other's files**:

| Key name                 | Bucket                | File name prefix | Capabilities                                                         | Used by                          |
| ------------------------ | --------------------- | ---------------- | -------------------------------------------------------------------- | -------------------------------- |
| `genesy-prod-uploads`    | the production bucket | `genesy/`        | `listBuckets, readFiles, writeFiles, deleteFiles`                    | the API (this section)           |
| `genesy-prod-db-backups` | the production bucket | `db-backups/`    | `listBuckets, writeFiles` (**no** `readFiles`, **no** `deleteFiles`) | `backup.sh` (`../ops/README.md`) |

A bug in the API can't touch the backups, and the backup script can't read or change customer files. The backup key can't even read old dumps back, so someone who takes over the VPS can't download or wipe them. Never use one key for both jobs.

1. **Make the uploads key** with the b2 CLI (`b2 account authorize` with the master key first). The CLI sets the exact capabilities; the web console's presets add more:

   ```bash
   b2 key create --bucket <bucket> --name-prefix genesy/ genesy-prod-uploads listBuckets,readFiles,writeFiles,deleteFiles
   ```

   It prints the key ID and the application key once. Check it with `b2 key list --long`.

2. **On the VPS:** write the application key (asked without echo):

   ```bash
   cd /srv/genesy-prod && deploy/set-provider-keys.sh production b2-uploads
   ```

3. **In `.env.production`:** set `B2_UPLOADS_BUCKET=<bucket>` and `B2_UPLOADS_KEY_ID=<key ID>`. `B2_UPLOADS_ENDPOINT` and `B2_UPLOADS_REGION` default to `eu-central-003`; set them if the bucket lives elsewhere.
4. **Restart:** `deploy/deploy.sh production compose up -d --wait api shop office`. `deploy.sh` now adds `docker-compose.uploads.yml`. That gives the API the bucket and key, and sets `FILES_ORIGIN` (the shop's and office's image CSP) to the bucket's endpoint.
5. **Check it:** upload a product image in the office and see it in the shop. In B2 the file shows up under `genesy/<workspace id>/`.

To turn uploads off, move `secrets/b2_uploads_key` out of `secrets/` and restart the same way. Files already in the bucket stay there.

## Never

- Never run `docker volume rm genesy-prod-pg`: it deletes production data. (`down -v` can't, since the volume is external.)
- Never run `docker image prune -a`: it deletes the previous version's images, and a rollback then has to rebuild them. `deploy.sh production prune` keeps them.
- Never set `GENESY_ALLOW_DEMO_SEED`, `PAYMENTS_PROVIDER=fake` or `ALLOW_FAKE_PAYMENTS` here.
- Never reuse staging's secret files.
- Never give the uploads key and the backup key the same prefix, and never use the bucket's master key on the VPS.
- Never paste a Stripe or Resend key into chat, an issue, an env file or a command line: only into `set-provider-keys.sh`.
