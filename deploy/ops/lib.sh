# shellcheck shell=bash disable=SC2034  # variables here are read by the scripts that source it
# Shared by deploy/deploy.sh and deploy/ops/*.sh (sourced, never run). Paths, the compose -f
# chain and the alert notifier live here once.
# Test overrides: GENESY_ROOT (checkout), GENESY_OPS_CONF (/etc/genesy-ops),
# GENESY_BACKUPS (/srv/genesy-backups), GENESY_OPS_STATE (/var/lib/genesy-ops).

OPS_CONF="${GENESY_OPS_CONF:-/etc/genesy-ops}"
BACKUPS="${GENESY_BACKUPS:-/srv/genesy-backups}"
OPS_STATE="${GENESY_OPS_STATE:-/var/lib/genesy-ops}"

# die <message>: stop; scripts that set $check also send it as a fail alert.
die() {
  echo "$(basename "$0"): $*" >&2
  [[ -z "${check:-}" ]] || notify "$check" fail "$*"
  exit 1
}

# env_setup <staging|production>: sets ROOT (checkout), DIR (deploy/<env>), ENV_FILE, COMPOSE
# (the full command with every -f the stack needs), MIGRATE_SVC, PUBLIC_HOSTS and ROUTES_OFF.
env_setup() {
  case "${1:-}" in
    production) ROOT="${GENESY_ROOT:-/srv/genesy-prod}" MIGRATE_SVC=cli
      PUBLIC_HOSTS="api.genesy.app shop.genesy.app office.genesy.app till.genesy.app app.ruahaenergy.com field.genesy.app flow.genesy.app genesy.app" ;;
    staging) ROOT="${GENESY_ROOT:-/srv/genesy-staging}" MIGRATE_SVC=seed
      PUBLIC_HOSTS="staging-api.genesy.app staging-shop.genesy.app staging-office.genesy.app staging-till.genesy.app staging-field.genesy.app staging-flow.genesy.app staging.genesy.app" ;;
    *) die "environment must be staging or production, got '${1:-}'" ;;
  esac
  GENESY_ENV="$1"
  DIR="$ROOT/deploy/$1"
  ENV_FILE="$DIR/.env.$1"
  [[ -f "$ENV_FILE" ]] || die "missing $ENV_FILE"
  COMPOSE=(docker compose -f "$DIR/docker-compose.$1.yml")
  # Untracked until DNS is live (README "Turning on Stripe"); sits between main and Stripe.
  # It sets traefik.enable=false, so no host answers through Traefik: ROUTES_OFF=1.
  ROUTES_OFF=""
  [[ -f "$DIR/docker-compose.nodns.yml" ]] && COMPOSE+=(-f "$DIR/docker-compose.nodns.yml") && ROUTES_OFF=1
  # Stripe is on exactly when its key file exists, so no command can forget the overlay.
  [[ -f "$DIR/secrets/stripe_secret_key" ]] && COMPOSE+=(-f "$DIR/docker-compose.stripe.yml")
  # File uploads likewise: each environment's own B2 bucket (staging's is not production's).
  [[ -f "$DIR/secrets/b2_uploads_key" && -f "$DIR/docker-compose.uploads.yml" ]] \
    && COMPOSE+=(-f "$DIR/docker-compose.uploads.yml")
  local runtime_secrets product_bulk_enabled
  runtime_secrets="$(env_value SECRETS_DIR)"
  runtime_secrets="${runtime_secrets:-./secrets}"
  [[ "$runtime_secrets" == /* ]] || runtime_secrets="$DIR/$runtime_secrets"
  case "$(env_value ASSISTANT_ENABLED)" in
    "" | false)
      # Cleanup keeps its provider credentials while new sessions are paused.
      [[ -f "$runtime_secrets/elevenlabs_api_key" && -f "$DIR/docker-compose.assistant.yml" ]] \
        && COMPOSE+=(-f "$DIR/docker-compose.assistant.yml")
      :
      ;;
    true) COMPOSE+=(-f "$DIR/docker-compose.assistant.yml") ;;
    *) die "ASSISTANT_ENABLED must be true or false" ;;
  esac
  product_bulk_enabled="$(env_value PRODUCT_BULK_ENABLED)"
  case "$product_bulk_enabled" in
    "" | false | true) ;;
    *) die "PRODUCT_BULK_ENABLED must be true or false" ;;
  esac
  # Pausing admission keeps credentials and the independent uploads overlay for existing jobs.
  if [[ "$product_bulk_enabled" == true || -f "$runtime_secrets/anthropic_api_key" ]]; then
    [[ -f "$runtime_secrets/anthropic_api_key" && -s "$runtime_secrets/anthropic_api_key" ]] \
      || die "product bulk requires a nonempty $runtime_secrets/anthropic_api_key"
    [[ -f "$DIR/docker-compose.product-bulk.yml" ]] \
      || die "missing $DIR/docker-compose.product-bulk.yml"
    COMPOSE+=(-f "$DIR/docker-compose.product-bulk.yml")
  fi
  COMPOSE+=(--env-file "$ENV_FILE")
}

# env_value KEY: the value of KEY in ENV_FILE (last one wins), empty when unset.
env_value() { sed -n "s/^$1=//p" "$ENV_FILE" | tail -1; }

# Restore grants refer to cluster roles absent from pg_dump. Verification never repairs ACLs.
checkpoint_roles() {
  local mode="$1" role_sql
  shift
  role_sql="$(dirname "${BASH_SOURCE[0]}")/assistant-role.sql"
  [[ -f "$role_sql" ]] || role_sql="$(dirname "${BASH_SOURCE[0]}")/../assistant-role.sql"
  "$@" psql -X -v ON_ERROR_STOP=1 -v "checkpoint_role=assistant_checkpoint_$GENESY_ENV" \
    -v "checkpoint_mode=$mode" -U genesy -d genesy < "$role_sql" > /dev/null
}

# notify <check> <start|ok|fail> [message]: always logged; also pinged when OPS_CONF/ping_base
# holds a dead-man URL (healthchecks.io style: <base>/<check>, /start, /fail). A failed ping
# never fails the caller.
notify() {
  local check="$1" status="$2" msg="${3:-}" url
  echo "notify: $check $status${msg:+: $msg}" >&2
  [[ -s "$OPS_CONF/ping_base" ]] || return 0
  url="$(tr -d '[:space:]' < "$OPS_CONF/ping_base")/$check"
  case "$status" in start) url+=/start ;; fail) url+=/fail ;; esac
  curl -fsS -m 10 --retry 3 -o /dev/null --data-raw "$msg" "$url" || echo "notify: ping failed" >&2
}

# schema_version: newest applied migration of the env's live database, 0 on a fresh one.
schema_version() {
  local psql=("${COMPOSE[@]}" exec -T db psql -U genesy -d genesy -Atc)
  if [[ "$("${psql[@]}" "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")" == t ]]; then
    "${psql[@]}" "SELECT coalesce(max(version), 0) FROM _sqlx_migrations WHERE success"
  else
    echo 0
  fi
}

# hosts_down: prints " host=code" for every public host of the env that answers wrong: API
# /healthz must be 200, apps 2xx/3xx (the shop 503 too, until its app key is set). Asks the
# local Traefik (--resolve), so DNS and certificates don't matter here.
# With ROUTES_OFF (no DNS yet) Traefik has no routes, so it checks containers_down instead.
hosts_down() {
  local host path want code
  [[ -z "${ROUTES_OFF:-}" ]] || { containers_down; return; }
  for host in $PUBLIC_HOSTS; do
    path=/ want='2..|3..'
    [[ "$host" == *api.* ]] && path=/healthz want=200
    [[ "$host" == *shop.* ]] && want='2..|3..|503'
    code="$(curl -sk -o /dev/null -m 10 -w '%{http_code}' --resolve "$host:443:127.0.0.1" "https://$host$path" || true)"
    [[ "$code" =~ ^($want)$ ]] || printf ' %s=%s' "$host" "${code:-none}"
  done
}

# containers_down: prints " service=state[/health]" for every service of the stack (profile
# services aside) that is not running, or whose healthcheck is not healthy.
containers_down() {
  local ps svc state health
  ps="$("${COMPOSE[@]}" ps -a --format '{{.Service}} {{.State}} {{.Health}}')"
  for svc in $("${COMPOSE[@]}" config --services); do
    read -r state health < <(awk -v s="$svc" '$1 == s { print $2, $3; exit }' <<< "$ps")
    [[ "$state" == running && "${health:-healthy}" == healthy ]] \
      || printf ' %s=%s' "$svc" "${state:-missing}${health:+/$health}"
  done
}

# latest_dump <env>: newest finished dump of that env, empty when none.
latest_dump() {
  ls -1t "$BACKUPS/$1/genesy-$1-"*.dump 2> /dev/null | head -1
}
