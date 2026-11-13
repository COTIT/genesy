#!/usr/bin/env bash
# Tests for deploy.sh: a real git checkout, stub docker/curl/sudo/flock/df and backup/restore
# scripts on PATH:  bash deploy/deploy.test.sh
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# The checkout: two commits; the env file is untracked, as on the VPS.
root="$t/root" d="$t/root/deploy/production"
mkdir -p "$d" "$t/bin" "$t/opslib"
cp "$HERE/production/docker-compose.production.yml" "$HERE/production/docker-compose.stripe.yml" \
  "$HERE/production/docker-compose.uploads.yml" "$d/"
g() { git -C "$root" -c user.email=t@t -c user.name=t "$@"; }
g init -q && g add . && g commit -qm one && A="$(g rev-parse HEAD)"
echo "# two" >> "$d/docker-compose.production.yml" && g commit -qam two && B="$(g rev-parse HEAD)"
printf 'TRAEFIK_NETWORK=traefik\nGENESY_REF=\nMAIL_FROM=x\n' > "$d/.env.production"

echo 20261101000000 > "$t/schema"
cat > "$t/bin/docker" << 'EOF'
#!/usr/bin/env bash
echo "docker $*" >> "$CALLS"
if [[ "$1" == compose ]]; then
  while (( $# )) && [[ "$1" != config && "$1" != exec && "$1" != run && "$1" != pull && "$1" != ps ]]; do
    [[ "$1" == --env-file ]] && env_file="$2"
    shift
  done
  case "${1:-}" in
    config) [[ "$2" == --services ]] && { printf 'api\ndb\nshop\n'; exit 0; }
      ref="$(sed -n 's/^GENESY_REF=//p' "$env_file")"
      printf 'postgres:18@sha256:abc\nghcr.io/cotit/genesy-api:%s\nghcr.io/cotit/genesy-shop:%s\n' "$ref" "$ref" ;;
    pull) [[ -z "${STUB_PULL_FAIL:-}" ]] || exit 1 ;;
    exec)
      case "$*" in
        *api/voice/readiness*) [[ -z "${STUB_ASSISTANT_READY_FAIL:-}" ]] || exit 1 ;;
        *to_regclass*) echo t ;;
        *checkpoint_migrations*) echo "${STUB_CHECKPOINT_VERSION:-5}" ;;
        *'encode(checksum'*) cat "$T/applied" ;;
        *) cat "$T/schema" ;;
      esac ;;
    run) [[ -z "${STUB_MIGRATE_FAIL:-}" ]] || exit 49
      [[ -n "${STUB_MIGRATE_TO:-}" ]] && echo "$STUB_MIGRATE_TO" > "$T/schema" ;;
    ps) cat "$T/ps" 2> /dev/null ;;
  esac
  exit 0
fi
case "$1 $2" in
  "image ls") cat "$T/tags" 2> /dev/null ;;
esac
exit 0
EOF
cat > "$t/bin/curl" << 'EOF'
#!/usr/bin/env bash
echo "curl $*" >> "$CALLS"
printf '%s' "${STUB_CODE:-200}"
EOF
printf '#!/usr/bin/env bash\n[[ "$1" == -n ]] && shift\nexec "$@"\n' > "$t/bin/sudo"
printf '#!/usr/bin/env bash\n[[ -z "${STUB_LOCKED:-}" ]]\n' > "$t/bin/flock"
printf '#!/usr/bin/env bash\necho "Filesystem 1024-blocks Used Available"\necho "/dev/x 1 1 ${STUB_FREE_KB:-99999999} 1%% /"\n' > "$t/bin/df"
cat > "$t/opslib/backup.sh" << 'EOF'
#!/usr/bin/env bash
echo "backup.sh $*" >> "$CALLS"
echo "backup: /srv/genesy-backups/$1/genesy-$1-X-$2.dump (1K, schema 1)"
EOF
printf '#!/usr/bin/env bash\necho "restore.sh $*" >> "$CALLS"\n' > "$t/opslib/restore.sh"
chmod +x "$t/bin/"* "$t/opslib/"*

export CALLS="$t/calls" T="$t" PATH="$t/bin:$PATH" GENESY_ROOT="$root" GENESY_OPS_LIB="$t/opslib" HEALTH_WAIT=0
run() { : > "$CALLS"; bash ${TRACE:-} "$HERE/deploy.sh" production "$@" > "$t/out" 2>&1; }
state() { sed -n "s/^$1=//p" "$d/.deploy-state"; }
line() { grep -n -m1 -- "$1" "$CALLS" | cut -d: -f1; }
before() { local a b; a="$(line "$1")" b="$(line "$2")"; [[ -n "$a" && -n "$b" && "$a" -lt "$b" ]]; }

# First deploy (given a short sha): pull → backup → migrate → up --no-build → health, in that order.
run "${A:0:7}" || fail "deploy A: $(cat "$t/out")"
before "compose .* pull" "backup.sh production pre-$A" || fail "pull not before backup"
if grep -q "compose .* build" "$CALLS"; then fail "pull mode built"; fi
grep -q "up -d --wait --wait-timeout 300 --no-build" "$CALLS" || fail "up may build after a pull"
before "backup.sh production pre-$A" "run --rm cli migrate" || fail "backup not before migrate"
before "run --rm cli migrate" "up -d --wait --wait-timeout 300" || fail "migrate not before up"
before "up -d --wait --wait-timeout 300" "curl " || fail "health not after up"
grep -q "curl .*--resolve api.genesy.app:443:127.0.0.1 https://api.genesy.app/healthz" "$CALLS" || fail "api health not checked"
grep -q "curl .*--resolve flow.genesy.app:443:127.0.0.1 https://flow.genesy.app/$" "$CALLS" || fail "flow health not checked"
grep -q "curl .*--resolve app.ruahaenergy.com:443:127.0.0.1 https://app.ruahaenergy.com/$" "$CALLS" || fail "field health not checked"
[[ "$(grep -c '^curl' "$CALLS")" == 8 ]] || fail "not every host checked"
grep -qx "GENESY_REF=$A" "$d/.env.production" || fail "GENESY_REF not set"
[[ "$(state live)" == "$A" && -z "$(state previous)" ]] || fail "state after A: $(cat "$d/.deploy-state")"
if grep -q "stripe.yml" "$CALLS"; then fail "stripe overlay without its key"; fi
grep -q "$A deploy\|deploy $A ok" "$d/deploys.log" || fail "not logged"

# Timing stays on stderr and in the log, including one total despite command substitutions.
for step in checkout images-pull database-ready backup migration startup health prune total; do
  grep -Eq "timing action=deploy ref=$A step=$step elapsed_seconds=[0-9]+ status=ok$" "$d/deploys.log" \
    || fail "missing successful timing: $step"
done
[[ "$(grep -c 'step=total ' "$d/deploys.log")" = 1 ]] || fail "subshell wrote extra total timings"
: > "$CALLS"
bash "$HERE/deploy.sh" production "$A" > "$t/stdout" 2> "$t/stderr"
if grep -q '^timing ' "$t/stdout"; then fail "timing contaminated command stdout"; fi
grep -q 'step=total .*status=ok' "$t/stderr" || fail "total timing missing from stderr"
[[ "$(state pre_deploy_dump)" == "/srv/genesy-backups/production/genesy-production-X-pre-$A.dump" ]] \
  || fail "timing contaminated captured backup path"

# A failing migration retains its original exit status and stops before application startup.
if STUB_MIGRATE_FAIL=1 run "$A"; then fail "migration failure passed"; else status=$?; fi
[[ "$status" = 49 ]] || fail "timing changed migration exit status: $status"
grep -q 'step=migration .*status=failed' "$t/out" || fail "failed migration timing missing"
grep -q 'step=total .*status=failed' "$t/out" || fail "failed total timing missing"
if grep -q 'up -d --wait --wait-timeout' "$CALLS"; then fail "startup ran after failed migration"; fi

# Second deploy with a migration: previous recorded with its schema and the pre-deploy dump.
mkdir -p "$d/secrets" && touch "$d/secrets/stripe_secret_key"
STUB_MIGRATE_TO=20261102000000 run "$B" || fail "deploy B: $(cat "$t/out")"
grep -q -- "-f $d/docker-compose.stripe.yml" "$CALLS" || fail "stripe overlay missing with its key"
[[ "$(state live)" == "$B" && "$(state previous)" == "$A" ]] || fail "state after B: $(cat "$d/.deploy-state")"
[[ "$(state previous_schema)" == 20261101000000 && "$(state live_schema)" == 20261102000000 ]] || fail "schemas: $(cat "$d/.deploy-state")"
[[ "$(state pre_deploy_dump)" == "/srv/genesy-backups/production/genesy-production-X-pre-$B.dump" ]] || fail "dump: $(state pre_deploy_dump)"

# Rollback across a schema change needs --restore-db; nothing changes without it.
if run rollback; then fail "rollback across a migration without --restore-db"; fi
grep -q "needs --restore-db" "$t/out" || fail "no reason given"
grep -qx "GENESY_REF=$B" "$d/.env.production" || fail "refused rollback changed GENESY_REF"
echo 20261101000000 > "$t/schema.after"
run rollback --restore-db || fail "rollback --restore-db: $(cat "$t/out")"
before "restore.sh production /srv/genesy-backups/production/genesy-production-X-pre-$B.dump" "up -d --wait" || fail "restore not before up"
grep -qx "GENESY_REF=$A" "$d/.env.production" || fail "GENESY_REF not back to A"
[[ "$(g rev-parse HEAD)" == "$A" ]] || fail "checkout not back to A"
[[ "$(state live)" == "$A" && "$(state previous)" == "$B" && -z "$(state pre_deploy_dump)" ]] || fail "state after rollback: $(cat "$d/.deploy-state")"

# Same schema: plain rollback, no restore. A redeploy of the live commit keeps previous.
echo 20261101000000 > "$t/schema"
run "$B" && run "$B" || fail "redeploy B: $(cat "$t/out")"
[[ "$(state previous)" == "$A" ]] || fail "redeploy lost previous: $(cat "$d/.deploy-state")"
run rollback || fail "plain rollback: $(cat "$t/out")"
if grep -q "restore.sh" "$CALLS"; then fail "restored without a schema change"; fi
[[ "$(state live)" == "$A" ]] || fail "plain rollback state"

# Unhealthy: exits 1 with the rollback command, state already points at the new version.
if STUB_CODE=502 run "$B"; then fail "unhealthy deploy passed"; fi
grep -q "deploy/deploy.sh production rollback" "$t/out" || fail "no rollback hint"
[[ "$(state live)" == "$B" ]] || fail "unhealthy deploy not recorded"
grep -q "deploy $B unhealthy" "$d/deploys.log" || fail "unhealthy not logged"
# The shop may answer 503 until its app key is set; nothing else may.
cat > "$t/bin/curl" << 'EOF'
#!/usr/bin/env bash
echo "curl $*" >> "$CALLS"
[[ "$*" == *shop.genesy.app* ]] && printf 503 || printf 200
EOF
run "$B" || fail "shop 503 failed the deploy: $(cat "$t/out")"

# A failed pull stops before the backup; nothing changes.
if STUB_PULL_FAIL=1 run "$B"; then fail "deployed after a failed pull"; fi
if grep -q "backup.sh" "$CALLS"; then fail "backup after a failed pull"; fi

# Build fallback builds instead of pulling, and up may then use the built images.
echo "IMAGE_SOURCE=build" >> "$d/.env.production"
run "$B" || fail "build deploy: $(cat "$t/out")"
grep -q "compose .* build" "$CALLS" || fail "build mode did not build"
if grep -q "compose .* pull" "$CALLS"; then fail "build mode pulled"; fi
if grep -q -- "--no-build" "$CALLS"; then fail "build mode passed --no-build"; fi
sed -i.bak '/^IMAGE_SOURCE=/d' "$d/.env.production"

# Prune keeps live and previous of BOTH envs (shared GHCR repos; staging's -staging tags too),
# never pinned images.
sd="$root/deploy/staging" && mkdir -p "$sd"
echo "GENESY_REF=s1" > "$sd/.env.staging"
printf 'live=s1\nprevious=\n' > "$sd/.deploy-state"
printf '%s\n' "$A" "$B" old1 s1 s1-staging old2-staging > "$t/tags"
run prune || fail "prune: $(cat "$t/out")"
grep -q "image rm ghcr.io/cotit/genesy-api:old1$" "$CALLS" || fail "old tag kept"
grep -q "image rm ghcr.io/cotit/genesy-api:old2-staging$" "$CALLS" || fail "old staging tag kept"
for kept in "$A" "$B" s1 s1-staging; do
  if grep -q "image rm ghcr.io/cotit/genesy-api:$kept$" "$CALLS"; then fail "pruned kept tag $kept"; fi
done
if grep -q "image rm postgres" "$CALLS"; then fail "pruned a pinned image"; fi
grep -q "builder prune -f --max-used-space 10GB" "$CALLS" || fail "build cache not capped"

# A commit only in a bundle: refused without it, deployed with it.
other="$t/other" && git clone -q "$root" "$other"
echo "# three" >> "$other/deploy/production/docker-compose.production.yml"
git -C "$other" -c user.email=t@t -c user.name=t commit -qam three
C="$(git -C "$other" rev-parse HEAD)"
git -C "$other" bundle create -q "$t/c.bundle" HEAD 2> /dev/null || git -C "$other" bundle create "$t/c.bundle" HEAD 2> /dev/null
if run "$C"; then fail "unknown commit deployed"; fi
grep -q "pass --bundle" "$t/out" || fail "no bundle hint"
run "$C" --bundle "$t/c.bundle" || fail "bundle deploy: $(cat "$t/out")"
[[ "$(state live)" == "$C" ]] || fail "bundle commit not live"

# Guards.
if run nothing-like-a-sha; then fail "bad sha taken"; fi
if STUB_FREE_KB=1000 run "$B"; then fail "deployed on a full disk"; fi
if STUB_LOCKED=1 run "$B"; then fail "deployed while locked"; fi
if bash "$HERE/deploy.sh" nowhere "$B" > /dev/null 2>&1; then fail "unknown env taken"; fi
run compose ps || fail "compose pass-through: $(cat "$t/out")"
grep -q "^docker compose -f $d/docker-compose.production.yml -f $d/docker-compose.stripe.yml --env-file $d/.env.production ps$" "$CALLS" || fail "pass-through: $(cat "$CALLS")"
touch "$d/secrets/b2_uploads_key"
run compose ps || fail "compose with uploads: $(cat "$t/out")"
grep -q "^docker compose -f $d/docker-compose.production.yml -f $d/docker-compose.stripe.yml -f $d/docker-compose.uploads.yml --env-file" "$CALLS" \
  || fail "uploads overlay not added with its key: $(cat "$CALLS")"

# Product imports keep provider/storage configuration after admission is paused, in both envs.
bulk_root="$t/bulk-root"
bulk_run() {
  : > "$CALLS"
  GENESY_ROOT="$bulk_root" bash -c '. "$1"; env_setup "$2"; "${COMPOSE[@]}" ps' \
    _ "$HERE/ops/lib.sh" "$1" > "$t/bulk-out" 2>&1
}
for env in staging production; do
  bd="$bulk_root/deploy/$env"
  mkdir -p "$bd/secrets"
  cp "$HERE/$env/docker-compose.$env.yml" "$HERE/$env/docker-compose.product-bulk.yml" \
    "$HERE/$env/docker-compose.uploads.yml" "$HERE/$env/docker-compose.stripe.yml" \
    "$HERE/$env/docker-compose.assistant.yml" "$bd/"
  printf 'ANTHROPIC_API_KEY=fixture-env-key\n' > "$bd/.env.$env"
  bulk_run "$env" || fail "$env default bulk chain"
  ! grep -q product-bulk "$CALLS" || fail "$env bulk enabled by default/env key"
  printf 'PRODUCT_BULK_ENABLED=false\n' >> "$bd/.env.$env"
  bulk_run "$env" || fail "$env explicit bulk pause without key"
  ! grep -q product-bulk "$CALLS" || fail "$env paused bulk chained without key"
  printf 'PRODUCT_BULK_ENABLED=true\n' >> "$bd/.env.$env"
  if bulk_run "$env"; then fail "$env enabled bulk accepted without key file"; fi
  grep -q 'requires a nonempty' "$t/bulk-out" || fail "$env missing key error"
  [[ ! -s "$CALLS" ]] || fail "$env missing key reached Docker"
  touch "$bd/secrets/anthropic_api_key"
  if bulk_run "$env"; then fail "$env enabled bulk accepted empty key file"; fi
  rm "$bd/secrets/anthropic_api_key"
  mkdir "$bd/secrets/anthropic_api_key"
  if bulk_run "$env"; then fail "$env enabled bulk accepted key directory"; fi
  rmdir "$bd/secrets/anthropic_api_key"
  printf 'fixture-file-key' > "$bd/secrets/anthropic_api_key"
  bulk_run "$env" || fail "$env enabled bulk chain"
  grep -q -- "-f $bd/docker-compose.product-bulk.yml" "$CALLS" || fail "$env bulk overlay absent"
  for invalid in TRUE yes 0; do
    printf 'PRODUCT_BULK_ENABLED=%s\n' "$invalid" >> "$bd/.env.$env"
    if bulk_run "$env"; then fail "$env accepted bulk flag $invalid"; fi
    grep -q 'PRODUCT_BULK_ENABLED must be true or false' "$t/bulk-out" || fail "$env invalid flag error"
  done
  printf 'PRODUCT_BULK_ENABLED=false\n' >> "$bd/.env.$env"
  touch "$bd/secrets/b2_uploads_key" "$bd/secrets/stripe_secret_key" \
    "$bd/secrets/elevenlabs_api_key" "$bd/docker-compose.nodns.yml"
  bulk_run "$env" || fail "$env paused bulk chain"
  for overlay in nodns stripe uploads assistant product-bulk; do
    grep -q -- "-f $bd/docker-compose.$overlay.yml" "$CALLS" || fail "$env paused bulk lost $overlay"
  done
  for secret_dir in ./private-secrets "$t/absolute-secrets-$env"; do
    case "$secret_dir" in /*) key_dir="$secret_dir" ;; *) key_dir="$bd/$secret_dir" ;; esac
    mkdir -p "$key_dir"
    mv "$bd/secrets/anthropic_api_key" "$key_dir/anthropic_api_key"
    printf 'SECRETS_DIR=%s\n' "$secret_dir" >> "$bd/.env.$env"
    bulk_run "$env" || fail "$env custom secrets path $secret_dir"
    grep -q product-bulk "$CALLS" || fail "$env paused bulk lost custom key"
    mv "$key_dir/anthropic_api_key" "$bd/secrets/anthropic_api_key"
  done
  printf 'SECRETS_DIR=./secrets\nPRODUCT_BULK_ENABLED=true\n' >> "$bd/.env.$env"
  mv "$bd/docker-compose.product-bulk.yml" "$bd/bulk-overlay-away"
  if bulk_run "$env"; then fail "$env enabled bulk accepted missing overlay"; fi
  grep -q 'missing .*docker-compose.product-bulk.yml' "$t/bulk-out" || fail "$env missing overlay error"
done

# No DNS yet (VPS-only nodns overlay sets traefik.enable=false): every host would answer 404, so
# health is the containers' own state and no Traefik probe runs. Rollback uses the same check.
touch "$d/docker-compose.nodns.yml"
printf 'api running healthy\ndb running healthy\nshop running \n' > "$t/ps"
run "$B" || fail "nodns deploy: $(cat "$t/out")"
grep -q "health: Traefik routes off (no DNS yet); checked container health only" "$t/out" || fail "no routes-off line: $(cat "$t/out")"
if grep -q "^curl" "$CALLS"; then fail "probed Traefik under nodns"; fi
printf 'api running unhealthy\ndb exited \n' > "$t/ps"
if run "$B"; then fail "unhealthy containers passed under nodns"; fi
grep -q "health: failing: api=running/unhealthy db=exited shop=missing" "$t/out" || fail "nodns failure: $(cat "$t/out")"
if run rollback; then fail "unhealthy rollback passed under nodns"; fi
printf 'api running healthy\ndb running healthy\nshop running healthy\n' > "$t/ps"
run rollback || fail "nodns rollback: $(cat "$t/out")"
grep -q "routes off" "$t/out" || fail "rollback probed Traefik under nodns"
if grep -q "^curl" "$CALLS"; then fail "rollback probed Traefik under nodns"; fi

# Explicit compatible rollback accepts only reviewed checksums and a disabled assistant.
rm "$d/docker-compose.nodns.yml" "$d/.deploy-state"
echo 20261102000000 > "$t/schema"
run "$B" || fail 'compatibility baseline deploy'
mkdir -p "$root/apps/api/migrations"
printf 'CREATE SCHEMA assistant_checkpoint;\n' > "$root/apps/api/migrations/20261106184342_assistant_checkpoints.up.sql"
cp "$HERE/assistant-compatibility.py" "$root/deploy/"
python3 - "$root" "$t/applied" <<'PY'
import hashlib,json,pathlib,sys
root=pathlib.Path(sys.argv[1]); path='apps/api/migrations/20261106184342_assistant_checkpoints.up.sql'
checksum=hashlib.sha384((root/path).read_bytes()).hexdigest()
(root/'deploy/assistant-compatibility.json').write_text(json.dumps({'graphVersion':'office-assistant-v1','workflowVersion':2,'saverVersion':'1.1.2','checkpointSchema':'assistant_checkpoint','checkpointSchemaVersion':5,'invoiceDeliveryVersion':1,'batchVersion':1,'migrations':{path:checksum}}))
for name in ("batches", "batch_repo", "batch_handlers", "batch_dto"):
    target=root/f'apps/api/src/modules/assistant/{name}.rs'; target.parent.mkdir(parents=True,exist_ok=True); target.touch()
guard=root/'apps/api/src/http/assistant.rs'; guard.parent.mkdir(parents=True,exist_ok=True); guard.write_text('Assistant-Batch-Token batch_repo::claim(')
(root/'apps/api/src/modules/assistant/repo.rs').write_text('batch_repo::expire( batch_repo::prune(')
module=root/'apps/api/src/modules/invoices/delivery.rs'; module.parent.mkdir(parents=True); module.touch()
jobs=root/'apps/api/src/jobs/kinds.rs'; jobs.parent.mkdir(parents=True)
jobs.write_text('INVOICE_DELIVERY => return crate::modules::invoices::delivery::run(ctx, &job).await,\nINVOICE_DELIVERY_SWEEP => return crate::modules::invoices::delivery::sweep(ctx).await,\n')
pathlib.Path(sys.argv[2]).write_text('20261106184342|'+checksum+'\n')
PY
g add apps/api deploy/assistant-compatibility.py deploy/assistant-compatibility.json
g commit -qm assistant
R="$(g rev-parse HEAD)"
STUB_MIGRATE_TO=20261106184342 run "$R" || fail 'assistant deploy'
[[ "$(state assistant_fingerprint)" == office-assistant-v1/2/1.1.2/assistant_checkpoint/5/1/1 ]] || fail 'release fingerprint not recorded'
echo 'ASSISTANT_ENABLED=false' >> "$d/.env.production"
if run rollback --compatible-assistant; then fail 'pre-assistant rollback without worker accepted'; fi
sed 's/^previous_schema=.*/previous_schema=20261106184342/' "$d/.deploy-state" > "$t/same-schema-state"
cp "$d/.deploy-state" "$t/old-schema-state"
cp "$t/same-schema-state" "$d/.deploy-state"
if run rollback; then fail 'ordinary same-schema rollback bypassed assistant worker compatibility'; fi
grep -q -- '--compatible-assistant' "$t/out" || fail 'ordinary assistant rollback lacks compatibility instruction'
if run rollback --restore-db; then fail 'same-schema restore flag bypassed assistant worker compatibility'; fi
if run rollback --compatible-assistant; then fail 'same-schema pre-assistant rollback accepted'; fi
[[ "$(state live)" == "$R" ]] || fail 'rejected rollback changed running version'
cp "$t/old-schema-state" "$d/.deploy-state"
# A later reviewed migration can roll back to the first worker-capable release.
python3 - "$root" "$t/applied" <<'PY'
import hashlib,json,pathlib,sys
root=pathlib.Path(sys.argv[1]); path='apps/api/migrations/20261106204413_invoice_delivery.up.sql'
(root/path).write_text('CREATE TABLE invoice_deliveries (id uuid);\n')
checksum=hashlib.sha384((root/path).read_bytes()).hexdigest()
marker=root/'deploy/assistant-compatibility.json'; data=json.loads(marker.read_text()); data['migrations'][path]=checksum; marker.write_text(json.dumps(data))
pathlib.Path(sys.argv[2]).write_text('20261106204413|'+checksum+'\n')
PY
g add apps/api/migrations deploy/assistant-compatibility.json
g commit -qm 'later reviewed migration'
S="$(g rev-parse HEAD)"
STUB_MIGRATE_TO=20261106204413 run "$S" || fail 'later assistant deploy'
echo 'ASSISTANT_ENABLED=true' >> "$d/.env.production"
if STUB_ASSISTANT_READY_FAIL=1 run "$S"; then fail 'assistant deployed without readiness'; fi
run "$S" || fail 'assistant readiness deploy'
grep -q 'api/voice/readiness' "$CALLS" || fail 'assistant readiness not checked'
if run rollback --compatible-assistant; then fail 'compatible rollback while enabled'; fi
grep -q 'ASSISTANT_ENABLED=false' "$t/out" || fail 'no disable instruction'
echo 'ASSISTANT_ENABLED=false' >> "$d/.env.production"
if STUB_CHECKPOINT_VERSION=6 run rollback --compatible-assistant; then fail 'unknown checkpoint schema accepted'; fi
cp "$t/applied" "$t/applied.good"
printf '20261106204413|changed\n' > "$t/applied"
if run rollback --compatible-assistant; then fail 'changed migration accepted'; fi
cp "$t/applied.good" "$t/applied"
run rollback --compatible-assistant || fail "compatible rollback: $(cat "$t/out")"
! grep -q restore.sh "$CALLS" || fail 'compatible rollback restored business DB'
[[ "$(state live)" == "$R" && -n "$(state assistant_fingerprint)" ]] || fail 'compatibility evidence not recorded'

echo "deploy.test.sh: all passed"
