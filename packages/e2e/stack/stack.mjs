// Shared bits of the full-stack harness (STACK.md): ports, paths, the state file, processes.
import { spawn, spawnSync } from "node:child_process"
import { existsSync, mkdirSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import process from "node:process"
import { fileURLToPath } from "node:url"

/** Shifts every stack port, so two checkouts (or a stack beside the fake-API suite) never clash. */
export const OFFSET = Number(process.env.E2E_STACK_OFFSET ?? 0)
export const PORTS = {
  postgres: 55400 + OFFSET,
  api: 8400 + OFFSET,
  shop: 4300 + OFFSET,
  office: 4301 + OFFSET,
  field: 4302 + OFFSET,
  till: 4303 + OFFSET,
  landing: 4304 + OFFSET,
  flow: 4305 + OFFSET,
  mail: 2525 + OFFSET,
  files: 55410 + OFFSET,
}
export const URLS = Object.fromEntries(
  Object.entries(PORTS)
    .filter(([name]) => !["postgres", "mail", "files"].includes(name))
    .map(([name, port]) => [name, `http://127.0.0.1:${port}`])
)

export const E2E = dirname(dirname(fileURLToPath(import.meta.url)))
export const ROOT = dirname(dirname(E2E))
export const API_DIR = join(ROOT, "apps/api")
/** Per offset: state, seed logins and logs of one stack. Gitignored, never committed. */
export const DIR = join(E2E, ".stack", String(OFFSET))
export const STATE = join(DIR, "state.json")
export const SEED = join(DIR, "seed.json")
export const LOGS = join(DIR, "logs")
/** Every mail the API sent, one .eml each (stack/mail-sink.mjs). */
export const MAIL = join(DIR, "mail")

/** Only ever this container: the stack never touches the dev Postgres or any other. */
export const CONTAINER = `genesy-e2e-stack-${OFFSET}`
export const DB = "genesy"
/** Copy of the freshly seeded database, for `stack:reset`. */
export const SEED_DB = "genesy_seed"
export const DATABASE_URL = `postgres://postgres@127.0.0.1:${PORTS.postgres}/${DB}`

/** The stack's local S3 store (Garage v2.4.1, 2026-09-08): stack only; staging and production use B2. */
export const FILES_CONTAINER = `genesy-e2e-files-${OFFSET}`
export const FILES_IMAGE =
  "dxflrs/garage:v2.4.1@sha256:9c96caa2612d3411acc5b0e6701fb238dbfba33e533a6d7d3d811a4b12d0d020"
export const FILES_BUCKET = "genesy-e2e"
/** Where the API writes and browsers read signed links (one origin: the shop's CSP img-src). */
export const FILES_ORIGIN = `http://127.0.0.1:${PORTS.files}`

export const readState = () => (existsSync(STATE) ? JSON.parse(readFileSync(STATE, "utf8")) : null)
export function writeState(state) {
  mkdirSync(DIR, { recursive: true })
  writeFileSync(STATE, JSON.stringify(state, null, 2))
}
export const clearState = () => rmSync(STATE, { force: true })

/** Runs a command to the end, output inherited; throws on a non-zero exit. */
export function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { stdio: "inherit", ...opts })
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} exited ${r.status ?? r.signal}`)
  return r
}

/** `run` without blocking the event loop (so the app builds beside it keep their queue moving). */
export const exec = (cmd, args, opts = {}) =>
  new Promise((resolve, reject) =>
    spawn(cmd, args, { stdio: "inherit", ...opts }).on("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`${cmd} ${args.join(" ")} exited ${code}`))
    )
  )

/** Starts a long-running process in its own group (so `stop` takes its children too), logs to a file. */
export function start(name, cmd, args, opts = {}) {
  mkdirSync(LOGS, { recursive: true })
  const log = openSync(join(LOGS, `${name}.log`), "w")
  const child = spawn(cmd, args, { detached: true, stdio: ["ignore", log, log], ...opts })
  child.unref()
  return child.pid
}

export function stop(pid) {
  try {
    process.kill(-pid, "SIGTERM")
  } catch {
    // Already gone.
  }
}

export const alive = (pid) => {
  // kill(0) would signal our own process group: no pid is never alive.
  if (!pid) return false
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/** Polls `url` until it answers < 500, or throws after `seconds` (pointing at the log). */
export async function waitFor(name, url, seconds = 120) {
  const until = Date.now() + seconds * 1000
  while (Date.now() < until) {
    try {
      const r = await fetch(url, { redirect: "manual" })
      if (r.status < 500) return
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
  throw new Error(`${name} did not answer ${url} within ${seconds} s (see ${join(LOGS, `${name}.log`)})`)
}

/** SQL inside the stack's own container (the host may have no psql). */
export const psql = (sql) =>
  run("docker", ["exec", CONTAINER, "psql", "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-qc", sql])

/** The API's env for the stack: debug build, fake payments, rate limits out of the way. */
export function apiEnv(state) {
  const env = {
    ...process.env,
    DATABASE_URL,
    BIND_ADDR: `127.0.0.1:${PORTS.api}`,
    RUST_LOG: process.env.RUST_LOG ?? "genesy_360_api=info,tower_http=warn",
    FRONTEND_URL: URLS.office,
    API_PUBLIC_URL: URLS.api,
    // The stack's own mail sink: mail lands in MAIL as files, none leaves the machine.
    SMTP_URL: `smtp://127.0.0.1:${PORTS.mail}`,
    SMTP_ALLOW_INSECURE: "true",
    MAIL_FROM: "Genesy e2e <no-reply@e2e.test>",
    PAYMENTS_PROVIDER: "fake",
    GENESY_ALLOW_DEMO_SEED: "1",
    SIGNUP_ENABLED: "true",
    PRODUCT_BULK_ENABLED: "true",
    SKIP_BOOT_MIGRATIONS: "true",
    CREDENTIALS_KEY: state.credentialsKey,
    // The stack's own Garage: product images and other uploads work as on staging.
    B2_UPLOADS_ENDPOINT: FILES_ORIGIN,
    B2_UPLOADS_REGION: "garage",
    B2_UPLOADS_BUCKET: FILES_BUCKET,
    B2_UPLOADS_KEY_ID: state.files.keyId,
    B2_UPLOADS_APPLICATION_KEY: state.files.secret,
    // Every spec logs in from 127.0.0.1: the per-IP limits would lock the suite out.
    RATE_LIMIT_BURST: "10000",
    RATE_LIMIT_REPLENISH_SECS: "1",
    PRIVATE_RATE_LIMIT_BURST: "10000",
    PRIVATE_IP_RATE_LIMIT_BURST: "100000",
    INVITES_PER_HOUR: "10000",
    ...(state.voice ? { ASSISTANT_BRIDGE_SECRET: state.voice.bridgeSecret } : {}),
  }
  // Synthetic bulk fixtures never use a developer's live extraction credentials.
  delete env.ANTHROPIC_API_KEY
  delete env.ANTHROPIC_API_KEY_FILE
  return env
}

/** Office's restricted checkpoint login on the stack (deploy/assistant-role.sql, environment "local"). */
export const CHECKPOINT_ROLE = "assistant_checkpoint_local"

/**
 * `VOICE=1 stack:up`: Office's voice assistant on, for these workspaces only. The provider key and
 * agent id come from the caller's env (ELEVENLABS_API_KEY, ELEVENLABS_AGENT_ID) and are never stored.
 */
export function voiceEnv(state, workspaces) {
  if (!state.voice) return {}
  return {
    ASSISTANT_ENABLED: "true",
    ASSISTANT_WORKSPACE_ALLOWLIST: workspaces.join(","),
    ASSISTANT_BRIDGE_SECRET: state.voice.bridgeSecret,
    ASSISTANT_CHECKPOINT_URL: `postgres://${CHECKPOINT_ROLE}:${state.voice.checkpointPassword}@127.0.0.1:${PORTS.postgres}/${DB}`,
    LANGSMITH_TRACING: "false",
  }
}

/** After migrations: the checkpoint role and grants, then its login password (inside the container). */
export function voiceRole(state) {
  const sql = readFileSync(join(ROOT, "deploy/assistant-role.sql"), "utf8")
  run("docker", [
    "exec", "-i", CONTAINER, "psql", "-U", "postgres", "-d", DB, "-v", "ON_ERROR_STOP=1", "-q",
    "-v", `checkpoint_role=${CHECKPOINT_ROLE}`, "-v", "checkpoint_mode=bootstrap", "-f", "-",
  ], { input: sql, stdio: ["pipe", "ignore", "inherit"] })
  // Hex only, so it is safe inside the quoted literal.
  run("docker", ["exec", "-i", CONTAINER, "psql", "-U", "postgres", "-d", DB, "-v", "ON_ERROR_STOP=1", "-q", "-f", "-"], {
    input: `ALTER ROLE ${CHECKPOINT_ROLE} LOGIN PASSWORD '${state.voice.checkpointPassword}';`,
    stdio: ["pipe", "ignore", "inherit"],
  })
}

export const apiBin = () =>
  join(process.env.CARGO_TARGET_DIR ?? join(API_DIR, "target"), "debug", "genesy-360-api")

/** The mail sink the API sends to (before the seed: signups mail a verification link). */
export const startMail = () =>
  start("mail", process.execPath, [join(E2E, "stack", "mail-sink.mjs")], {
    env: { ...process.env, MAIL_DIR: MAIL, MAIL_PORT: String(PORTS.mail) },
  })

export const startApi = (state) => start("api", apiBin(), [], { env: apiEnv(state), cwd: DIR })

/** Each seeded workspace's id, by account (`seed.workspaces.AFM`), from its first member's login. */
export async function workspaceIds(seed) {
  const ids = {}
  for (const [name, app, role] of [["AFM", "office (AFM)", "owner"], ["RUAHA", "office (RUAHA)", "admin"]]) {
    const who = seed.credentials.find((c) => c.app === app && c.role === role)
    const r = await fetch(`${URLS.api}/v1/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ email: who.login, password: who.secret }),
    })
    if (!r.ok) throw new Error(`login ${app} ${role}: ${r.status}`)
    ids[name] = (await r.json()).data.workspace_ids[0]
  }
  return ids
}
