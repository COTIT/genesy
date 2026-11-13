# Staging

Office assistant stays disabled by default. See [assistant deployment](../ASSISTANT.md) for runtime secrets, restricted checkpoint roles, readiness and compatible rollback.

Product imports stay disabled by default. See [product import deployment](../PRODUCT_BULK.md) for release gates, file credentials and pausing existing workers safely.

A full Genesy stack on the VPS, next to the existing services. It runs the API with its own Postgres, Mailpit (a mail catcher), the apps (shop, office, till, field, flow) and the landing page, all served through the VPS's existing Traefik.

Think of it as a dress rehearsal stage: same sets as opening night, fake money in the till, and every letter lands in a box backstage.

| Host                        | Service | Container port |
| --------------------------- | ------- | -------------- |
| `staging-api.genesy.app`    | api     | 8080           |
| `staging-shop.genesy.app`   | shop    | 3000           |
| `staging-office.genesy.app` | office  | 3000           |
| `staging-till.genesy.app`   | till    | 3000           |
| `staging-flow.genesy.app`   | flow    | 3000           |
| `staging-field.genesy.app`  | field   | 8080           |
| `staging.genesy.app`        | landing | 3000           |

**What staging is not:**

- Legacy checkout defaults to `PAYMENTS_PROVIDER=fake`. Workspace Globadyme accounts are restricted to sandbox by the Compose-pinned `GLOBADYME_ENVIRONMENT=sandbox`; API workers and the seed command share that pin.
- No outgoing mail. Every mail lands in Mailpit.
- No PDF renderer: those routes answer 503, as designed. File uploads answer 503 too until "Turning on file uploads" is done; staging then uses its own B2 bucket, never production's.
- No database backups to B2: the nightly backup stays on the VPS (`../ops/README.md`).
- Field is built with `VITE_FEATURE_RESOURCES=on` (equipment, loans, requests) and the demo banner.
- No prod secret, ever.

Every step below is for the user to run on the VPS, or for the coordinator once the user says go.

## Files

| File                         | What                                                                                 |
| ---------------------------- | ------------------------------------------------------------------------------------ |
| `docker-compose.staging.yml` | The stack, with Traefik labels                                                       |
| `docker-compose.uploads.yml` | File uploads: staging's own B2 bucket, added by `deploy.sh` once `secrets/b2_uploads_key` exists |
| `docker-compose.local.yml`   | Local override: no Traefik, ports on `127.0.0.1:18080` and `18100`–`18105`           |
| `.env.staging.example`       | Every variable, no secrets. Copy it to `.env.staging` on the VPS.                    |
| `secrets/`                   | Secret files on the VPS only. Git ignores everything in it.                          |
| `../images/next.Dockerfile`  | shop, office, till, landing and flow (Next standalone, `--build-arg APP=…`); shared with production |
| `../images/field.Dockerfile` | field: a Vite build, served by nginx; shared with production                         |
| `../images/field.nginx.conf` | SPA fallback and the `/v1` forward to the API                                        |
| `smoke.sh`                   | Local check: build, start, probe, then remove everything it added                    |
| `../deploy.sh`               | Deploy, rollback, prune, and `compose` with the right `-f` chain (production README)  |
| `../ops/`                    | Backups, restore tests, alerts for both stacks (`../ops/README.md`)                   |
| `../../apps/api/Dockerfile`  | API image. The build arg `ALLOW_FAKE_PAYMENTS=1` is set by the staging compose only. |

### The fake-payments build arg

A release build normally refuses `PAYMENTS_PROVIDER=fake`. The staging compose builds the API with `ALLOW_FAKE_PAYMENTS=1`, which sets `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`.

That has a side effect: every `debug_assert!` in the API and its crates runs on staging. They are extra self-checks. They can make staging a little slower, and an assertion that fails stops the request with a panic instead of letting it pass quietly.

In `apps/api/src`, the only thing `cfg!(debug_assertions)` decides is the payments choice. The default build (prod) never gets this arg.

## The VPS (read on 2026-11-01)

- Traefik v3.6, Docker provider, `exposedByDefault=false` (every router here sets `traefik.enable=true`).
- External network `traefik`. Entrypoints `web` (redirects to HTTPS) and `websecure`. Cert resolver `le` (HTTP challenge). These are the defaults in `.env.staging.example`.
- IPv4 `185.77.96.44`, IPv6 `2a02:4780:d:ca37::1`. 4 cores, 15 GB RAM.
- Deploy user `deploy`. Staging lives in `/srv/genesy-staging` and production in `/srv/genesy-prod`.
- Existing hosts: `control.genesy.app` and `*.control.genesy.app` only. Nothing else under `genesy.app` is taken.
- Traefik is the first hop (no CDN), so the IP allow-list and the API's per-IP rate limit see the visitor's address.

## DNS records (OpenProvider, zone `genesy.app`)

Staging and production, one A and one AAAA record per host:

| Type | Name             | Value                 | TTL  |
| ---- | ---------------- | --------------------- | ---- |
| A    | `staging-api`    | `185.77.96.44`        | 3600 |
| AAAA | `staging-api`    | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging-shop`   | `185.77.96.44`        | 3600 |
| AAAA | `staging-shop`   | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging-office` | `185.77.96.44`        | 3600 |
| AAAA | `staging-office` | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging-till`   | `185.77.96.44`        | 3600 |
| AAAA | `staging-till`   | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging-field`  | `185.77.96.44`        | 3600 |
| AAAA | `staging-field`  | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging-flow`   | `185.77.96.44`        | 3600 |
| AAAA | `staging-flow`   | `2a02:4780:d:ca37::1` | 3600 |
| A    | `api`            | `185.77.96.44`        | 3600 |
| AAAA | `api`            | `2a02:4780:d:ca37::1` | 3600 |
| A    | `shop`           | `185.77.96.44`        | 3600 |
| AAAA | `shop`           | `2a02:4780:d:ca37::1` | 3600 |
| A    | `office`         | `185.77.96.44`        | 3600 |
| AAAA | `office`         | `2a02:4780:d:ca37::1` | 3600 |
| A    | `till`           | `185.77.96.44`        | 3600 |
| AAAA | `till`           | `2a02:4780:d:ca37::1` | 3600 |
| A    | `field`          | `185.77.96.44`        | 3600 |
| AAAA | `field`          | `2a02:4780:d:ca37::1` | 3600 |
| A    | `flow`           | `185.77.96.44`        | 3600 |
| AAAA | `flow`           | `2a02:4780:d:ca37::1` | 3600 |
| A    | `staging`        | `185.77.96.44`        | 3600 |
| AAAA | `staging`        | `2a02:4780:d:ca37::1` | 3600 |
| A    | `@` (apex)       | `185.77.96.44`        | 3600 |
| AAAA | `@` (apex)       | `2a02:4780:d:ca37::1` | 3600 |
| A    | `www`            | `185.77.96.44`        | 3600 |
| AAAA | `www`            | `2a02:4780:d:ca37::1` | 3600 |

Traefik answers on IPv6 (checked 2026-11-01: HTTP 301 on `[2a02:4780:d:ca37::1]`), so the AAAA records are safe.

Let's Encrypt can only issue the certificates once these names resolve. The `staging-flow` and `flow` records are new with Flow: add them before the first deploy that contains it ("Flow" below).

## One-time setup (on the VPS)

```bash
# 1. A deploy checkout (as user deploy)
sudo mkdir -p /srv/genesy-staging && sudo chown deploy /srv/genesy-staging
git clone git@github.com:COTIT/flow.ai.git /srv/genesy-staging
cd /srv/genesy-staging/deploy/staging
```

```bash
# 2. Secret files (random, made on the VPS, never copied anywhere)
chmod 700 secrets
pw="$(openssl rand -hex 24)"
printf '%s' "$pw" > secrets/db_password
printf 'postgres://genesy:%s@db:5432/genesy' "$pw" > secrets/database_url
openssl rand -hex 32 | tr -d '\n' > secrets/credentials_key
unset pw
chmod 444 secrets/*   # containers run as non-root users; the 700 folder keeps other users out
```

```bash
# 3. Env file (Traefik values are already filled in)
cp .env.staging.example .env.staging && chmod 600 .env.staging
```

```bash
# 4. Read-only login to GHCR, where CI pushes the private images (token: "GHCR pull token" below).
#    One login serves staging and production: both run as user deploy.
install -d -m 700 ~/.config/genesy && install -m 600 /dev/null ~/.config/genesy/ghcr-token
nano ~/.config/genesy/ghcr-token   # paste the token, save; it never goes on a command line
docker login ghcr.io -u <token owner> --password-stdin < ~/.config/genesy/ghcr-token
```

Then deploy (next section). Keep `credentials_key` once you have it: it encrypts workspace mail credentials stored in the database. A new key makes those stored credentials unreadable.

### GHCR pull token (once, in GitHub)

Repository rename: the source is now `COTIT/flow.ai`. Existing image/package names,
Compose projects, server paths and domains stay unchanged. For an existing VPS checkout,
update only its Git remote: `git remote set-url origin git@github.com:COTIT/flow.ai.git`.
Verify that the existing packages (including `genesy-buildcache`) still grant the renamed
repository Actions write access before the next release. A green `ci` run on `main`
confirms publication; a local test run alone does not.

The VPS needs read access to the seven private `ghcr.io/cotit/genesy-*` packages, nothing more. These steps are yours, in GitHub.

1. **COTIT → Settings → Packages:** allow private container packages to be created. The first green `images` run then creates `genesy-api`, `-shop`, `-office`, `-till`, `-field`, `-landing` and `-flow`. Check each one's settings: visibility **private**, and "Manage Actions access" lists `COTIT/flow.ai` with **Write**.
2. **A machine account** (for example `genesy-deploy-bot`) in COTIT, given **Read** on those seven packages only (package settings → Manage access). It needs no repository access.
3. **On that account:** a **classic** personal access token with only `read:packages`, expiring in a year (set a reminder). GHCR does not take fine-grained tokens unless GitHub lists a Packages permission for them when you create one.

Fallback without a machine account: your own classic token with only `read:packages`. It can read every package you can, so the machine account is safer.

To rotate: write the new token into `~/.config/genesy/ghcr-token` on the VPS, run the `docker login` line again, then delete the old token.

## Deploy from main (after CI is green)

Image preparation runs alongside checks and pushes run-specific candidates. The existing
`images` checks publish final commit tags only after every quality check, test shard and image
producer succeeds. Deploy only a fully green main run. Failed-job reruns can reuse successful
producer manifests from that same run; missing or expired manifests require a full rerun.
Candidate tags are not deployable commit SHAs. Never deploy a partial publication.

Deployment timings go to stderr and `deploy/staging/deploys.log`. Include backup,
migration, health and cleanup time when comparing releases. Human waiting between promotions
is measured separately.


Only deploy a commit whose `ci` workflow passed on `main`, including its two `images` jobs (api and web). They promoted this commit's reviewed image digests, or reused digests from the last green commit when inputs did not change (`ghcr.io/cotit/genesy-<app>:<sha>`, with `-staging` for api, office and field); the VPS only pulls them. It is the same script and the same steps as production (`../production/README.md`, "Deploy from main", "Rollback" and "Fallback: build on the VPS"); only the env name changes:

```bash
cd /srv/genesy-staging && deploy/deploy.sh staging <green-main-sha> --bundle /tmp/genesy.bundle
deploy/deploy.sh staging rollback            # back to the previous version
```

On staging the migrations run in a one-off `seed` container (`genesy-360-api migrate`, not the seed). Every manual compose command goes through `deploy/deploy.sh staging compose …`, which adds the Stripe and no-DNS files when they are there. The nightly backup, weekly restore test and alerts come from the ops kit, installed once from the production checkout (`../ops/README.md`).

**The shop's app key.** The shop needs a storefront key before it shows a catalog:

1. Log in to `staging-office`.
2. Make an app key with scope `catalog:read`.
3. Put it in `STOREFRONT_APP_KEY` in `.env.staging`.
4. Run `deploy/deploy.sh staging compose up -d shop`.

**Read mail (Mailpit).** From your own machine:

```bash
ssh -L 8025:127.0.0.1:18025 genesy
```

Then open <http://localhost:8025>.

## Flow (`staging-flow.genesy.app`)

Flow is its own Next container (`flow`, image `ghcr.io/cotit/genesy-flow:<sha>`, built once and shared by staging and production like till). Its server forwards `/api/voice/*` to Office over the `internal` network at `http://office:3000`; the voice agent, ElevenLabs key, bridge secret and checkpoint database stay in Office (`../ASSISTANT.md`). Flow itself takes only `API_URL`, `VOICE_UPSTREAM_URL`, (`https://staging-office.genesy.app`, where the assistant's Office links open) and `SHOP_PUBLIC_URL` (`https://staging-shop.genesy.app`, where full-page storefront preview links open), both read per request, not at build,, no secret, and no variable in `.env.staging`. Office keeps its one public host (`staging-office.genesy.app`); Flow adds no public route to it. When Office is down or its assistant is off, Flow's voice calls fail and the rest of Flow keeps answering.

Before the first deploy of a commit that contains Flow:

1. **DNS first.** Add the `staging-flow` A and AAAA records ("DNS records" above) and wait until they resolve. Without them the deploy's health check still passes (it asks the local Traefik with `--resolve`), but Let's Encrypt keeps failing for that host and `vps-watch` alerts on its missing certificate on every run.
2. **GHCR package.** The first green `images` run after the merge creates `genesy-flow`. Check its settings: visibility **private**, "Manage Actions access" lists `COTIT/flow.ai` with **Write**, and the machine account has **Read**. Without Read the VPS pull fails and the deploy stops before its backup and migrations.
3. **Rollback past Flow.** A rollback to a commit from before Flow leaves the newer `flow` container running (compose does not remove services the older file lacks). Stop it by hand: `deploy/deploy.sh staging compose stop flow`.
4. **Memory.** Flow adds one Next container per environment: ceilings of 384 MB on staging and 512 MB on production (production README, "Resource limits").

If the VPS still has an untracked `docker-compose.nodns.yml`, give `flow` the same `traefik.enable=false` entry as the other apps.

## Turning on Stripe (test mode)

Staging runs the in-process fake until this is done. The staging image keeps debug assertions on, so the API refuses a live key here: only Stripe test mode, test cards (`4242 4242 4242 4242`) and no real money.

1. **Stripe dashboard, test mode:** copy the test secret key (`sk_test_…`). Then add a webhook endpoint `https://staging-api.genesy.app/v1/payments/webhooks/stripe` with the same events as production (`deploy/production/README.md`, "Turning on Stripe"), and copy its signing secret (`whsec_…`).
2. **On the VPS:** `deploy/set-provider-keys.sh staging stripe` (asks for both, no echo, refuses a live key).
3. **Restart the API.** `deploy.sh` adds `docker-compose.stripe.yml` by itself once `secrets/stripe_secret_key` exists. The `seed` service keeps its own fake whatever is set.

   ```bash
   deploy/deploy.sh staging compose up -d --wait api
   ```

4. If `STAGING_IP_ALLOWLIST` is set, Stripe's webhook calls are blocked: add Stripe's webhook IP ranges (stripe.com/files/ips/ips_webhooks.txt) or leave the list empty.

Mail on staging stays on Mailpit; Resend is production only.

## Turning on file uploads

Product images (the demo's AFM photos too) go to staging's own private B2 bucket, under `genesy/<workspace>/…`. It mirrors production (`../production/README.md`, "Turning on file uploads") with a bucket and key of its own:

| Key name | Bucket | File name prefix | Capabilities |
| --- | --- | --- | --- |
| `genesy-staging-uploads` | the staging bucket (`genesy-staging-files-…`, region `eu-central-003`) | `genesy/` | `listBuckets, readFiles, writeFiles, deleteFiles` |

1. **On the VPS:** write the application key (asked without echo): `cd /srv/genesy-staging && deploy/set-provider-keys.sh staging b2-uploads`.
2. **In `.env.staging`:** set `B2_UPLOADS_BUCKET=<bucket>` and `B2_UPLOADS_KEY_ID=<key ID>`. The endpoint and region default to `eu-central-003`.
3. **Restart:** `deploy/deploy.sh staging compose up -d --wait api shop office`. `deploy.sh` now adds `docker-compose.uploads.yml`. It gives the API and the `seed` service the bucket and key, runs the seed as `seed-demo --images`, and sets `FILES_ORIGIN` (the shop's and office's image CSP) to the bucket's endpoint.
4. **Check it:** upload a product image in the office and see it in the shop.

**AFM's photos.** `seed-demo --images` fetches them one at a time from AFM's image CDN and keeps a copy in the bucket under `genesy/demo-cache/afm/` (a sha256 of the URL). Later reseeds read the copy and never need AFM. If AFM can't be reached and no copy exists, those products are seeded without a photo and one stderr line gives the count. The rest of the seed is unchanged. `down -v` doesn't touch the bucket, so the copies survive a reset. A reset leaves the old workspaces' files in the bucket; delete `genesy/` there now and then if it grows.

To turn uploads off, move `secrets/b2_uploads_key` out of `secrets/` and restart the same way.

## Reset

This wipes the staging database and every caught mail, starts clean, then reseeds:

```bash
cd /srv/genesy-staging
deploy/deploy.sh staging compose down -v
deploy/deploy.sh staging compose up -d --wait db
deploy/deploy.sh staging compose --profile seed run --rm seed migrate   # the API doesn't migrate at boot
deploy/deploy.sh staging compose up -d --wait
deploy/deploy.sh staging compose --profile seed run --rm seed
```

`down -v` only removes this project's volume (`genesy-staging_pg`). The secret files stay, so the new database gets the same password.

To remove staging entirely, run `deploy/deploy.sh staging compose down -v --rmi all`, then delete `/srv/genesy-staging`, the staging DNS records and the staging timers (`sudo systemctl disable --now genesy-backup-staging.timer genesy-restore-test-staging.timer genesy-prune-staging.timer`).

## Demo data

After the first deploy, and after each reset:

```bash
deploy/deploy.sh staging compose --profile seed run --rm seed
```

This runs `genesy-360-api seed-demo`, which fills every app with demo data: AFM's real catalogue (75 products in 10 categories, with photos once file uploads are on), 300 orders, 40 invoices, a week of till shifts, and 60 farmers over four seasons (full list: `apps/api/docs/api/demo-seed.md`). It only runs where `GENESY_ALLOW_DEMO_SEED=1`, and only the staging compose sets that.

It prints a logins table (`app | role | login | secret | path`), then the storefront app key, sample paths (passports, shop orders) and `seeded in N.N s`. The passwords, PINs and keys are new on every run and are printed only this once, so save them somewhere safe before sharing them.

`seed` is its own service with no Traefik labels, so the one-off container never receives web traffic.

## Local smoke check (no VPS, no Traefik)

```bash
deploy/staging/smoke.sh
```

It:

- builds all five images, including the release Rust build (10–20 minutes);
- starts the stack with throwaway secrets;
- checks `api /healthz`;
- checks each app's home page, and that each app's `/v1` forward reaches the API;
- checks that the API booted with fake payments.

When it finishes, it removes its containers and volume, deletes only the images that were not there before, and runs `docker builder prune -af`. Other containers (`genesy-dev-pg`, `genesy-pg-*`) are never touched.

## Globadyme verification

Use a disposable staging database and the branch source for migration, SQLx, API and browser checks. Keep the live staging data out of test fixtures. Follow the normal backup/readiness deployment flow before exposing the branch; the migration refuses a destructive downgrade once gateway account or financial history exists. Application rollback requires schema compatibility, and financial data must never be restored over newer money activity.

Workspace sandbox account setup and provider smoke checks are in [Globadyme hosted payments](../../apps/api/docs/api/globadyme.md). Production collection remains gated pending an official origin. No sandbox/live provider payment is proven by loopback tests alone.
