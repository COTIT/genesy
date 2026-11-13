import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { test } from "vitest"
import {
  buildMatrix,
  summarize,
  estimateRun,
  safeLocalURL,
  evaluateAssertions,
  mergeReviews,
  loadInputs,
} from "./acceptance.mjs"

test("API authentication fixture includes every eligible catalog ID without non-bearer substitutions", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_eligible_operations.json", import.meta.url), "utf8"))
  assert.equal(fixture.eligibleOperationIds.length, 430)
  assert.deepEqual(fixture.eligibleOperationIds,
    catalog.filter(operation => ["execute", "secure_ui"].includes(operation.disposition)).map(operation => operation.id).sort())
  assert.deepEqual(fixture.excludedDispositions, ["external", "identity", "internal"])
  assert.deepEqual(fixture.excludedSecuritySchemes, ["app_key", "assistant_bridge", "till_session"])
})

test("API refusal fixture includes every eligible effectful ID without narrowing unavailable or secure operations", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_effect_operations.json", import.meta.url), "utf8"))
  assert.deepEqual(fixture.effectOperationIds,
    catalog.filter(operation => ["execute", "secure_ui"].includes(operation.disposition) && operation.effects.length).map(operation => operation.id).sort())
  assert.equal(fixture.scope, "unapproved_action_refusal_only")
  assert.equal(fixture.effectOperationIds.length, 233)
})

test("reviewed DTO fixture uses a boolean probe only for the exact ten JSON object contracts", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_body_validation_operations.json", import.meta.url), "utf8"))
  assert.deepEqual(fixture.objectBodyOperationIds, [
    "discount_groups_update", "farm_crops_update", "farm_payouts_cash_movement",
    "farm_villages_update", "farm_vouchers_lookup", "messages_office365_send",
    "orders_pick", "pdf_templates_update", "till_users_update", "till_vaults_create",
  ])
  assert.equal(fixture.probe, false)
  assert.equal(fixture.expectedStatus, 422)
  assert.equal(fixture.scope, "dto_top_level_type_validation_only")
  for (const id of fixture.objectBodyOperationIds) {
    const operation = catalog.find(operation => operation.id === id)
    assert(operation && ["execute", "secure_ui"].includes(operation.disposition))
    assert.equal(operation.body?.type, "object")
    assert(operation.requestMedia["application/json"])
    assert(!operation.requestMedia["multipart/form-data"])
  }
})

test("reviewed UUID fixture excludes body, required query, string-domain and unavailable contracts", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_uuid_validation_operations.json", import.meta.url), "utf8"))
  assert.equal(fixture.uuidOperationIds.length, 59)
  assert.equal(new Set(fixture.uuidOperationIds).size, 59)
  assert.equal(fixture.scope, "malformed_resource_identifier_only")
  assert.equal(fixture.malformed, "not-a-uuid")
  assert.equal(fixture.expectedStatus, 404)
  assert.deepEqual(fixture.exclusions, ["domain_or_model_string_paths", "required_resource_queries", "request_bodies", "unavailable_native_stubs"])
  for (const id of fixture.uuidOperationIds) {
    const operation = catalog.find(operation => operation.id === id)
    assert(operation && ["execute", "secure_ui"].includes(operation.disposition))
    assert.notEqual(operation.availability, "unavailable")
    assert.equal(operation.body, null)
    assert.deepEqual(operation.requestMedia, {})
    const path = operation.parameters.filter(parameter => parameter.in === "path")
    assert(path.length && path.every(parameter => parameter.schema.format === "uuid"))
    assert(!operation.parameters.some(parameter => parameter.in === "query" && parameter.required))
  }
})

test("the exact six bound-workspace fixtures have no business parameters or body", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_workspace_validation_operations.json", import.meta.url), "utf8"))
  assert.deepEqual(fixture.workspaceOperationIds, ["farm_supply_dispatch_options", "farm_tower_verify_summary", "invoices_send_tasks", "till_users_list", "till_vaults_list", "workspaces_get"])
  assert.equal(fixture.scope, "malformed_bound_workspace_only")
  assert.equal(fixture.malformed, "not-a-uuid")
  assert.equal(fixture.expectedStatus, 404)
  for (const id of fixture.workspaceOperationIds) {
    const operation = catalog.find(operation => operation.id === id)
    assert(operation)
    assert.deepEqual(operation.parameters, [])
    assert.equal(operation.body, null)
    assert.deepEqual(operation.requestMedia, {})
  }
})

test("API restricted-role fixture pins every permission-protected eligible ID", async () => {
  const { catalog } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_restricted_operations.json", import.meta.url), "utf8"))
  assert.equal(fixture.scope, "active_member_without_grants_only")
  assert.equal(fixture.restrictedOperationIds.length, 284)
  assert.deepEqual(fixture.restrictedOperationIds,
    catalog.filter(operation => ["execute", "secure_ui"].includes(operation.disposition) && [...(operation.authorization.allOf ?? []), ...(operation.authorization.anyOf ?? [])].length).map(operation => operation.id).sort())
})

test("API collection-tenant fixture has no unresolved target and leaves no collection mapping gap", async () => {
  const { catalog, reviews } = await loadInputs()
  const fixture = JSON.parse(await readFile(new URL("../../../../apps/api/tests/fixtures/assistant_collection_tenant_operations.json", import.meta.url), "utf8"))
  assert.equal(fixture.scope, "known_live_workspace_collection_boundary_only")
  assert.equal(fixture.collectionOperationIds.length, 92)
  assert.equal(new Set(fixture.collectionOperationIds).size, 92)
  const isCollection = operation => ["execute", "secure_ui"].includes(operation.disposition) && operation.path.includes("{workspace}") && [...operation.path.matchAll(/\{([^}]+)\}/g)].every(match => match[1] === "workspace")
  for (const id of fixture.collectionOperationIds) {
    const operation = catalog.find(operation => operation.id === id)
    assert(operation && isCollection(operation), `Unreviewed collection ${id}`)
    assert(!operation.parameters.some(parameter => parameter.in === "query" && parameter.required && !["from", "to", "compare_from", "compare_to"].includes(parameter.name)), `Unresolved required collection selector ${id}`)
  }
  const covered = new Set(reviews.reviews.filter(review => review.id !== "safety-final-live-workspace-collection-matrix" && review.gates.cross_workspace).flatMap(review => review.operations))
  for (const id of fixture.collectionOperationIds) covered.add(id)
  assert.deepEqual(catalog.filter(isCollection).filter(operation => !covered.has(operation.id)).map(operation => operation.id), [])
})

const operations = [
  {
    id: "products_get",
    disposition: "execute",
    method: "GET",
    path: "/workspace/{workspace}/products/{product}",
    effects: [],
    authorization: {
      boundary: "workspace",
      allOf: ["product.view"],
      roleAnyOf: [],
      predicates: [],
    },
  },
  {
    id: "app_keys_create",
    disposition: "secure_ui",
    method: "POST",
    path: "/workspace/{workspace}/app-keys",
    effects: ["credential"],
    authorization: {
      boundary: "workspace",
      allOf: ["workspace_admin"],
      roleAnyOf: [],
      predicates: [],
    },
  },
  {
    id: "auth_login",
    disposition: "external",
    method: "POST",
    path: "/login",
    effects: [],
    authorization: {},
  },
]
const dataset = {
  version: 1,
  cases: [
    {
      id: "product",
      operationId: "products_get",
      scenario: "happy",
      commands: {
        en: "Show product.",
        nl: "Toon product.",
        sw: "Onyesha bidhaa.",
      },
    },
  ],
}
const matrix = () => buildMatrix(operations, dataset)
const evidence = (overrides = {}) => ({
  catalogHash: "catalog",
  datasetHash: "dataset",
  operationId: "products_get",
  caseId: "product",
  gate: "language",
  language: "en",
  repetition: 1,
  kind: "real_native",
  model: "test-model",
  modelSource: "transcript.producing_llm",
  agentVersion: "agent-v1",
  agentVersionSource: "conversation.version_id",
  graphVersion: "graph-v1",
  passed: true,
  runId: "run-1",
  ...overrides,
})
const report = (records = []) =>
  summarize(matrix(), records, {
    catalogHash: "catalog",
    datasetHash: "dataset",
  })

test("every operation gets an explicit row and eligible gates", () => {
  assert.equal(matrix().length, 3)
  assert(matrix()[0].required.includes("cross_workspace"))
  assert(matrix()[1].required.includes("secure_handoff"))
  assert(!matrix()[1].required.includes("bridge"))
  assert.deepEqual(matrix()[2].required, [])
})

test("unavailable native behavior stays inside the full capability denominator", () => {
  const rows = buildMatrix(
    operations.map((operation, index) =>
      index === 0
        ? {
            ...operation,
            availability: "unavailable",
            unavailableReason: "native_statistics_not_implemented",
          }
        : operation
    ),
    dataset
  )
  assert.equal(rows[0].availability, "unavailable")
  const result = summarize(
    rows,
    [evidence({ gate: "result", kind: "real_http" })],
    {
      catalogHash: "catalog",
      datasetHash: "dataset",
    }
  )
  assert.equal(result.total, 3)
  assert.equal(result.eligible, 2)
  assert.deepEqual(result.unavailableCapabilities, [
    {
      operationId: "products_get",
      reason: "native_statistics_not_implemented",
    },
  ])
  assert.equal(result.operations[0].gates.result.status, "missing")
  assert.equal(result.releaseReady, false)
})

test("schema checks and mocked provider results never satisfy real acceptance", () => {
  for (const kind of ["mock", "schema", "source_reference"]) {
    const row = report([evidence({ kind, gate: "transport" })]).operations[0]
    assert.equal(row.gates.transport.status, "missing")
  }
  assert.equal(report().releaseReady, false)
})

test("wrong catalog, dataset or case evidence fails closed", () => {
  for (const extra of [
    { catalogHash: "old" },
    { datasetHash: "old" },
    { caseId: "other" },
    { operationId: "auth_login" },
  ]) {
    assert.throws(() => report([evidence(extra)]), /evidence/i)
  }
})

test("language coverage needs distinct repetitions and cannot hide a failed attempt", () => {
  const records = [
    evidence(),
    evidence(),
    evidence({ repetition: 2 }),
    evidence({ repetition: 3, passed: false }),
  ]
  const row = report(records).operations[0]
  assert.equal(row.languages.en.runs, 3)
  assert.equal(row.languages.en.successes, 2)
  assert.equal(row.languages.en.status, "complete")
  assert.equal(row.languages.nl.status, "missing")
})

test("source fixtures never count as executed proof", () => {
  const result = report()
  assert.equal(result.eligible, 2)
  assert.equal(result.languages.en.requiredRuns, 6)
  assert.equal(result.languages.en.runs, 0)
  assert.equal(result.operations[1].canonicalStatus, "missing")
})

test("native results cannot satisfy HTTP gates without actual backend assertions", () => {
  assert.equal(
    report([evidence({ gate: "transport" })]).operations[0].gates.transport
      .status,
    "missing"
  )
  assert.equal(
    report([evidence({ gate: "transport", kind: "real_http" })]).operations[0]
      .gates.transport.status,
    "passed"
  )
})

test("cost estimates expose required runs and daily quota without invented prices", () => {
  const result = estimateRun({
    capabilities: 390,
    languages: 3,
    repetitions: 3,
    dailyLimit: 50,
    concurrency: 2,
  })
  assert.equal(result.runs, 3510)
  assert.equal(result.minimumDays, 71)
  assert.equal(result.estimatedCost, null)
  assert.throws(
    () =>
      estimateRun({
        capabilities: 390,
        languages: 3,
        repetitions: 3,
        dailyLimit: 0,
        concurrency: 2,
      }),
    /positive/
  )
})

test("HTTP runner only accepts literal loopback URLs without credentials or redirects", () => {
  assert.equal(
    safeLocalURL("http://127.0.0.1:8080/v1").origin,
    "http://127.0.0.1:8080"
  )
  for (const value of [
    "https://api.genesy.app/v1",
    "http://localhost.evil.test/v1",
    "http://user:pass@127.0.0.1/v1",
    "http://0.0.0.0/v1",
    "http://127.0.0.1/v1?secret=x",
  ]) {
    assert.throws(() => safeLocalURL(value), /loopback/)
  }
})

test("result assertions check actual types, lengths and exact values", () => {
  assert.equal(
    evaluateAssertions({ data: [{ id: "one" }] }, [
      { path: "data", type: "array", maxLength: 5 },
      { path: "data.0.id", equals: "one" },
    ]),
    true
  )
  assert.equal(
    evaluateAssertions({ data: "[]" }, [{ path: "data", type: "array" }]),
    false
  )
  assert.equal(
    evaluateAssertions({ data: [1, 2] }, [
      { path: "data", type: "array", maxLength: 1 },
    ]),
    false
  )
  assert.equal(evaluateAssertions({}, []), false)
})

test("measurement requires observed USD units and deduplicates one paid run across gates", () => {
  const records = [
    evidence({
      latencyMs: 100,
      costUsdMicros: 3000,
      costSource: "provider_metadata_verified_units",
      usageSnapshot: "post_end_observed",
      conversationStatus: "done",
    }),
    evidence({
      gate: "bridge",
      latencyMs: 100,
      costUsdMicros: 3000,
      costSource: "provider_metadata_verified_units",
      usageSnapshot: "post_end_observed",
      conversationStatus: "done",
    }),
  ]
  const result = report(records)
  assert.equal(result.measurements.runs, 1)
  assert.equal(result.measurements.costUsdMicros, 3000)
  assert.equal(result.measurements.latencyP95Ms, 100)
  assert.equal(result.measurements.costBasis, "observed_post_end")
  assert.equal(result.measurements.billingFinality, "unverified")
  assert.equal(result.costEvidenceComplete, true)
  assert.equal(report([evidence()]).costEvidenceComplete, false)
  assert.equal(report().costEvidenceComplete, false)
})

test("active, processing, missing status and invented settlement cannot satisfy observed post-end cost evidence", () => {
  for (const snapshot of [
    { usageSnapshot: "before_conversation_end", conversationStatus: "done" },
    { usageSnapshot: "post_end_observed", conversationStatus: "processing" },
    { usageSnapshot: "post_end_observed" },
    { usageSnapshot: "settled", conversationStatus: "done" },
  ]) {
    const active = report([
      evidence({
        latencyMs: 100,
        costUsdMicros: 0,
        costSource: "provider_metadata_verified_units",
        ...snapshot,
      }),
    ])
    assert.equal(active.costEvidenceComplete, false)
    assert.equal(active.measurements.costUsdMicros, null)
    assert.equal(active.measurements.billingFinality, "unverified")
  }
})

test("native proof cannot mix or omit immutable agent/graph versions", () => {
  assert.throws(
    () => report([evidence({ agentVersion: undefined })]),
    /version/i
  )
  assert.throws(
    () => report([evidence(), evidence({ agentVersion: "agent-v2" })]),
    /version/i
  )
  assert.throws(
    () => report([evidence(), evidence({ graphVersion: "graph-v2" })]),
    /version/i
  )
})

test("native identities require actual producing model and conversation version sources", () => {
  for (const identity of [
    { modelSource: undefined },
    { modelSource: "llm_override" },
    { agentVersionSource: undefined },
    { agentVersionSource: "configured_agent_version" },
  ]) {
    assert.throws(() => report([evidence(identity)]), /identity source/i)
  }
})

test("independently owned reviews combine without masking duplicate IDs or source drift", () => {
  const one = { reviews: [{ id: "one" }], sources: { "test.rs": "old" } }
  assert.equal(
    mergeReviews([
      one,
      { reviews: [{ id: "two" }], sources: { "other.rs": "new" } },
    ]).reviews.length,
    2
  )
  assert.throws(() => mergeReviews([one, one]), /duplicate/i)
  assert.throws(
    () =>
      mergeReviews([one, { reviews: [], sources: { "test.rs": "changed" } }]),
    /source/i
  )
})

test("household verification keeps its native four-eyes gate without extending it to person verification", () => {
  const rows = buildMatrix(
    ["households", "persons"].map((kind) => ({
      id: `farm_records_${kind}_verify`,
      domain: "farm_records",
      disposition: "secure_ui",
      path: "/workspace/{workspace}/farm",
      effects: ["record"],
      authorization: {},
    })),
    { cases: [] }
  )
  assert(rows[0].required.includes("four_eyes"))
  assert(!rows[1].required.includes("four_eyes"))
})
