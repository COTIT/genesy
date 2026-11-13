#!/usr/bin/env bash
# Parses every compose chain deploy.sh can build, with the example env files, and checks what
# the optional overlays switch on. Needs the docker CLI only (no daemon, no build):
#   bash deploy/compose.test.sh
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
command -v docker > /dev/null || { echo "compose.test.sh: skipped (no docker CLI)"; exit 0; }

p="$HERE/production" s="$HERE/staging"
cp "$p/.env.production.example" "$t/prod.env" && echo "GENESY_REF=test" >> "$t/prod.env"
cp "$s/.env.staging.example" "$t/staging.env" && echo "GENESY_REF=test" >> "$t/staging.env"
# value <env file> <files…> -- <service> <var>: one resolved environment value, empty if unset.
value() {
  local env_file="$1" args=()
  shift
  while [[ "$1" != -- ]]; do args+=(-f "$1"); shift; done
  docker compose "${args[@]}" --env-file "$env_file" --profile cli --profile seed config --format json \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['services']['$2'].get('environment',{}).get('$3') or '')"
}

for chain in "$p/docker-compose.production.yml" \
  "$p/docker-compose.production.yml $p/docker-compose.stripe.yml"; do
  # shellcheck disable=SC2046,SC2086  # paths hold no spaces
  docker compose $(printf -- '-f %s ' $chain) --env-file "$t/prod.env" config -q || fail "production chain: $chain"
done
for chain in "$s/docker-compose.staging.yml" "$s/docker-compose.staging.yml $s/docker-compose.stripe.yml" \
  "$s/docker-compose.staging.yml $s/docker-compose.local.yml"; do
  # shellcheck disable=SC2046,SC2086  # paths hold no spaces
  docker compose $(printf -- '-f %s ' $chain) --env-file "$t/staging.env" config -q || fail "staging chain: $chain"
done

# Operator env files cannot change the realm pinned by each deployment's shared API anchor.
printf '\nGLOBADYME_ENVIRONMENT=sandbox\n' >> "$t/prod.env"
printf '\nGLOBADYME_ENVIRONMENT=production\n' >> "$t/staging.env"
for service in api cli; do
  [[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- "$service" GLOBADYME_ENVIRONMENT)" == production ]] || fail "production $service payment realm"
  [[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" "$p/docker-compose.stripe.yml" -- "$service" GLOBADYME_ENVIRONMENT)" == production ]] || fail "production Stripe $service payment realm"
done
for service in api seed; do
  [[ "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- "$service" GLOBADYME_ENVIRONMENT)" == sandbox ]] || fail "staging $service payment realm"
  [[ "$(value "$t/staging.env" "$s/docker-compose.staging.yml" "$s/docker-compose.stripe.yml" -- "$service" GLOBADYME_ENVIRONMENT)" == sandbox ]] || fail "staging Stripe $service payment realm"
done

# Without the uploads overlay: no bucket, no key file, no CSP origin.
[[ -z "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- api B2_UPLOADS_BUCKET)" ]] || fail "uploads on by default"
[[ -z "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- shop FILES_ORIGIN)" ]] || fail "FILES_ORIGIN set by default"
[[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- api SKIP_BOOT_MIGRATIONS)" == true ]] || fail "boot migrations not skipped"

# New home renderer stays opt-in in both environments.
[[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- shop SHOP_DESIGN_ENABLED)" == false ]] || fail "production design renderer not disabled by default"
[[ "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- shop SHOP_DESIGN_ENABLED)" == false ]] || fail "staging design renderer not disabled by default"
# Expired preview links send people back to their own environment's Flow.
[[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- shop FLOW_URL)" == https://flow.genesy.app ]] || fail "production shop FLOW_URL"
[[ "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- shop FLOW_URL)" == https://staging-flow.genesy.app ]] || fail "staging shop FLOW_URL"
# Flow's full-page storefront previews open this environment's own Shop.
[[ "$(value "$t/prod.env" "$p/docker-compose.production.yml" -- flow SHOP_PUBLIC_URL)" == https://shop.genesy.app ]] || fail "production flow SHOP_PUBLIC_URL"
[[ "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- flow SHOP_PUBLIC_URL)" == https://staging-shop.genesy.app ]] || fail "staging flow SHOP_PUBLIC_URL"

# With it, but the bucket and key id not set: refuses, naming a missing variable (which one
# Compose reports first differs between versions).
if docker compose -f "$p/docker-compose.production.yml" -f "$p/docker-compose.uploads.yml" \
  --env-file "$t/prod.env" config -q 2> "$t/err"; then fail "uploads without a bucket parsed"; fi
grep -qE "set B2_UPLOADS_(BUCKET|KEY_ID) in .env.production" "$t/err" || fail "unclear error: $(cat "$t/err")"

# With it and the bucket set: key from the secret file, CSP origin = endpoint, stripe still stacks.
printf 'B2_UPLOADS_BUCKET=genesy-prod\nB2_UPLOADS_KEY_ID=005abc\n' >> "$t/prod.env"
on=("$p/docker-compose.production.yml" "$p/docker-compose.stripe.yml" "$p/docker-compose.uploads.yml")
[[ "$(value "$t/prod.env" "${on[@]}" -- api B2_UPLOADS_BUCKET)" == genesy-prod ]] || fail "bucket not passed"
[[ "$(value "$t/prod.env" "${on[@]}" -- api B2_UPLOADS_APPLICATION_KEY_FILE)" == /run/secrets/b2_uploads_key ]] || fail "key file not passed"
[[ "$(value "$t/prod.env" "${on[@]}" -- cli B2_UPLOADS_KEY_ID)" == 005abc ]] || fail "cli has no uploads"
[[ "$(value "$t/prod.env" "${on[@]}" -- api PAYMENTS_PROVIDER)" == stripe ]] || fail "uploads overlay dropped stripe"
for app in shop office flow; do
  [[ "$(value "$t/prod.env" "${on[@]}" -- $app FILES_ORIGIN)" == https://s3.eu-central-003.backblazeb2.com ]] || fail "$app FILES_ORIGIN"
done
docker compose -f "${on[0]}" -f "${on[1]}" -f "${on[2]}" --env-file "$t/prod.env" config --format json \
  | python3 -c "import json,sys; s=json.load(sys.stdin)['services']['api']['secrets']; n={x['source'] for x in s}; sys.exit(0 if {'b2_uploads_key','stripe_secret_key','database_url'} <= n else 1)" \
  || fail "api secrets lost in the merge"

# Staging uploads: its own bucket through the same kind of overlay. Off without it; with it the
# API and the seed get the bucket and key file, the seed fetches photos, and the shop and office
# CSP allow the endpoint. Production's files never name the staging bucket.
[[ -z "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- api B2_UPLOADS_BUCKET)" ]] || fail "staging uploads on by default"
[[ -z "$(value "$t/staging.env" "$s/docker-compose.staging.yml" -- shop FILES_ORIGIN)" ]] || fail "staging FILES_ORIGIN set by default"
if docker compose -f "$s/docker-compose.staging.yml" -f "$s/docker-compose.uploads.yml" \
  --env-file "$t/staging.env" config -q 2> "$t/err"; then fail "staging uploads without a bucket parsed"; fi
grep -qE "set B2_UPLOADS_(BUCKET|KEY_ID) in .env.staging" "$t/err" || fail "unclear error: $(cat "$t/err")"
printf 'B2_UPLOADS_BUCKET=genesy-staging-files\nB2_UPLOADS_KEY_ID=005stg\n' >> "$t/staging.env"
son=("$s/docker-compose.staging.yml" "$s/docker-compose.stripe.yml" "$s/docker-compose.uploads.yml")
sv() { value "$t/staging.env" "${son[@]}" -- "$@"; }
[[ "$(sv api B2_UPLOADS_BUCKET)" == genesy-staging-files ]] || fail "staging bucket not passed"
[[ "$(sv api B2_UPLOADS_APPLICATION_KEY_FILE)" == /run/secrets/b2_uploads_key ]] || fail "staging key file not passed"
[[ "$(sv seed B2_UPLOADS_KEY_ID)" == 005stg ]] || fail "staging seed has no uploads"
[[ "$(sv api PAYMENTS_PROVIDER)" == stripe ]] || fail "staging uploads overlay dropped stripe"
for app in shop office flow; do
  [[ "$(sv $app FILES_ORIGIN)" == https://s3.eu-central-003.backblazeb2.com ]] || fail "staging $app FILES_ORIGIN"
done
docker compose -f "${son[0]}" -f "${son[1]}" -f "${son[2]}" --env-file "$t/staging.env" --profile seed config --format json \
  | python3 -c "import json,sys; s=json.load(sys.stdin)['services']; n={x['source'] for x in s['api']['secrets']}; sys.exit(0 if s['seed']['command']==['seed-demo','--images'] and {'b2_uploads_key','stripe_secret_key','database_url'} <= n else 1)" \
  || fail "staging seed command or api secrets wrong"
! grep -rq "genesy-staging" "$p" || fail "production names a staging bucket"
# lib.sh chains the overlay for staging exactly when the key file and the overlay both exist.
lib="$t/root/deploy" && mkdir -p "$lib/staging/secrets" && cp "$HERE/ops/lib.sh" "$lib/" \
  && cp "$s/docker-compose.uploads.yml" "$lib/staging/" && touch "$lib/staging/.env.staging"
chain() { GENESY_ROOT="$t/root" bash -c ". '$lib/lib.sh' && env_setup staging && printf '%s ' \"\${COMPOSE[@]}\""; }
[[ "$(chain)" != *uploads* ]] || fail "staging uploads chained without a key"
touch "$lib/staging/secrets/b2_uploads_key"
[[ "$(chain)" == *"$lib/staging/docker-compose.uploads.yml"* ]] || fail "staging uploads not chained"

# The env file can retain an operator key; only the API's file credential enters the container.
mkdir -p "$t/secrets"
printf 'fixture-file-key' > "$t/secrets/anthropic_api_key"
for env_file in "$t/prod.env" "$t/staging.env"; do
  printf '\nSECRETS_DIR=%s\nANTHROPIC_API_KEY=fixture-env-key\n' "$t/secrets" >> "$env_file"
done
bulk_check() {
  local env_file="$1" enabled="$2" args=()
  shift 2
  for f in "$@"; do args+=(-f "$f"); done
  docker compose "${args[@]}" --env-file "$env_file" --profile cli --profile seed config --format json | python3 -c '
import json, sys
config = json.load(sys.stdin)
api = config["services"]["api"]
env = api["environment"]
assert env["PRODUCT_BULK_ENABLED"] == sys.argv[1], "bulk admission flag"
assert env["ANTHROPIC_API_KEY_FILE"] == "/run/secrets/anthropic_api_key", "bulk file credential"
assert "ANTHROPIC_API_KEY" not in env, "plaintext/file credential collision"
assert env["B2_UPLOADS_APPLICATION_KEY_FILE"] == "/run/secrets/b2_uploads_key", "uploads credential lost"
assert env["PAYMENTS_PROVIDER"] == "stripe", "Stripe overlay lost"
assert {x["source"] for x in api["secrets"]} >= {"database_url", "credentials_key", "b2_uploads_key", "stripe_secret_key", "anthropic_api_key"}, "API secret merge lost"
assert config["secrets"]["anthropic_api_key"]["file"] == sys.argv[2] + "/anthropic_api_key", "bulk secret location"
for service, value in config["services"].items():
    assert "ANTHROPIC_API_KEY" not in value.get("environment", {}), service + " receives plaintext key"
    if service != "api":
        assert "ANTHROPIC_API_KEY_FILE" not in value.get("environment", {}), service + " receives provider config"
        assert "anthropic_api_key" not in {x["source"] for x in value.get("secrets", [])}, service + " receives provider secret"
' "$enabled" "$t/secrets" || fail "product bulk configuration: $env_file $enabled"
}
bulk_on=("${on[@]}" "$p/docker-compose.assistant.yml" "$p/docker-compose.product-bulk.yml")
bulk_son=("${son[@]}" "$s/docker-compose.assistant.yml" "$s/docker-compose.product-bulk.yml")
bulk_check "$t/prod.env" false "${bulk_on[@]}"
bulk_check "$t/staging.env" false "${bulk_son[@]}"
for env_file in "$t/prod.env" "$t/staging.env"; do
  printf 'PRODUCT_BULK_ENABLED=true\n' >> "$env_file"
done
bulk_check "$t/prod.env" true "${bulk_on[@]}"
bulk_check "$t/staging.env" true "${bulk_son[@]}"
for env_file in "$t/prod.env" "$t/staging.env"; do
  printf 'PRODUCT_BULK_ENABLED=false\n' >> "$env_file"
done
bulk_check "$t/prod.env" false "${bulk_on[@]}"
bulk_check "$t/staging.env" false "${bulk_son[@]}"

# Flow: its own public host; voice goes to Office over `internal` only. With every overlay on
# it still holds no secret, and Office gains no public route.
# flow_check <env file> <flow host> <office host> <middlewares or -> <files…>
flow_check() {
  local env_file="$1" host="$2" office="$3" mw="$4" args=()
  shift 4
  for f in "$@"; do args+=(-f "$f"); done
  docker compose "${args[@]}" --env-file "$env_file" config --format json | python3 -c '
import json, re, sys
host, office, mw = sys.argv[1:4]
s = json.load(sys.stdin)["services"]
f = s.get("flow")
assert f, "no flow service"
assert f["image"] == "ghcr.io/cotit/genesy-flow:test", f["image"]
assert set(f["networks"]) == {"internal", "edge"}, f["networks"]
env = f.get("environment", {})
# FILES_ORIGIN (CSP img-src for signed product images) comes only with the uploads overlay.
assert set(env) - {"FILES_ORIGIN"} == {"API_URL", "VOICE_UPSTREAM_URL", "OFFICE_PUBLIC_URL", "SHOP_PUBLIC_URL"}, sorted(env)
assert env.get("FILES_ORIGIN", "https://s3.eu-central-003.backblazeb2.com") == "https://s3.eu-central-003.backblazeb2.com", env
assert env["VOICE_UPSTREAM_URL"] == "http://office:3000" and env["API_URL"] == "http://api:8080/v1", env
# Links in Flow's assistant open this environment's own Office, never another one.
assert env["OFFICE_PUBLIC_URL"] == "https://" + office, env
assert not f.get("secrets"), f.get("secrets")
assert not [k for k in env if re.search("SECRET|KEY|TOKEN|ELEVENLABS|BRIDGE|CHECKPOINT|LANGSMITH|ASSISTANT", k)]
assert f.get("cap_drop") == ["ALL"] and "no-new-privileges:true" in f.get("security_opt", []), "not hardened"
assert f.get("restart") == "unless-stopped", f.get("restart")
l = f["labels"]
rules = {k: v for k, v in l.items() if k.endswith(".rule")}
assert list(rules.values()) == ["Host(`" + host + "`)"], rules
router = next(iter(rules))[: -len(".rule")]
assert l.get(router + ".middlewares", "-") == mw, l.get(router + ".middlewares")
assert [v for k, v in l.items() if k.endswith(".loadbalancer.server.port")] == ["3000"]
o = [v for k, v in s["office"]["labels"].items() if k.endswith(".rule")]
assert o == ["Host(`" + office + "`)"], o
' "$host" "$office" "$mw" || fail "flow service wrong: $env_file $*"
}
flow_check "$t/prod.env" flow.genesy.app office.genesy.app - "$p/docker-compose.production.yml"
flow_check "$t/prod.env" flow.genesy.app office.genesy.app - "${bulk_on[@]}"
flow_check "$t/staging.env" staging-flow.genesy.app staging-office.genesy.app \
  genesy-staging-allow,genesy-staging-noindex "$s/docker-compose.staging.yml"
flow_check "$t/staging.env" staging-flow.genesy.app staging-office.genesy.app \
  genesy-staging-allow,genesy-staging-noindex "${bulk_son[@]}"

# Assistant-only and full overlay chains mount webhook authentication in Office alone.
webhook_check() {
  local env_file="$1" args=()
  shift
  for f in "$@"; do args+=(-f "$f"); done
  docker compose "${args[@]}" --env-file "$env_file" --profile cli --profile seed config --format json | python3 -c '
import json, sys
config = json.load(sys.stdin)
name = "elevenlabs_webhook_secret"
assert config.get("secrets", {}).get(name, {}).get("file", "").endswith("/secrets/" + name), "webhook secret file missing"
office = config["services"]["office"]
assert office["environment"].get("ELEVENLABS_WEBHOOK_SECRET_FILE") == "/run/secrets/" + name, "Office webhook secret path missing"
assert {item["source"] for item in office["secrets"]} >= {name, "elevenlabs_api_key", "assistant_bridge_secret", "assistant_checkpoint_url", "langsmith_api_key"}, "Office secret mount lost"
for service, value in config["services"].items():
    assert "ELEVENLABS_WEBHOOK_SECRET" not in value.get("environment", {}), service + " exposes plaintext webhook config"
    if service != "office":
        assert "ELEVENLABS_WEBHOOK_SECRET_FILE" not in value.get("environment", {}), service + " receives webhook config"
        assert name not in {item["source"] for item in value.get("secrets", [])}, service + " receives webhook secret"
' || fail "webhook secret isolation: $env_file $*"
}
webhook_check "$t/prod.env" "$p/docker-compose.production.yml" "$p/docker-compose.assistant.yml"
webhook_check "$t/prod.env" "${bulk_on[@]}"
webhook_check "$t/staging.env" "$s/docker-compose.staging.yml" "$s/docker-compose.assistant.yml"
webhook_check "$t/staging.env" "${bulk_son[@]}"

# Field in production: RUAHA's own domain serves it; the old genesy host 301s there, keeping the path.
docker compose -f "$p/docker-compose.production.yml" --env-file "$t/prod.env" config --format json | python3 -c '
import json, sys
l = json.load(sys.stdin)["services"]["field"]["labels"]
r, mw = "traefik.http.routers.genesy-production-", "traefik.http.middlewares.genesy-production-field-to-ruaha.redirectregex."
assert l[r + "field.rule"] == "Host(`app.ruahaenergy.com`)", l[r + "field.rule"]
assert r + "field.middlewares" not in l, "app.ruahaenergy.com must not redirect"
assert l[r + "field-old.rule"] == "Host(`field.genesy.app`)", l.get(r + "field-old.rule")
assert l[r + "field-old.service"] == "genesy-production-field"
assert l[r + "field-old.middlewares"] == "genesy-production-field-to-ruaha"
assert l[r + "field-old.tls.certresolver"] == l[r + "field.tls.certresolver"]
assert l[mw + "regex"] == r"^https?://field\.genesy\.app/(.*)", l[mw + "regex"]
# `config` prints $ re-escaped as $$; the container label itself holds ${1}.
assert l[mw + "replacement"] == "https://app.ruahaenergy.com/$${1}", l[mw + "replacement"]
assert l[mw + "permanent"] == "true"
assert l["traefik.http.services.genesy-production-field.loadbalancer.server.port"] == "8080"
' || fail "production field hosts wrong"
GENESY_ROOT="$t/root" bash -c ". '$lib/lib.sh'; mkdir -p '$lib/production'; touch '$lib/production/.env.production'; env_setup production; echo \$PUBLIC_HOSTS" \
  | grep -qw "app.ruahaenergy.com" || fail "app.ruahaenergy.com not checked by deploy health / watch"

echo "compose.test.sh: all passed"
