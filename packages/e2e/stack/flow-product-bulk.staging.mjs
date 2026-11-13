// Explicit staging acceptance only; each execution owns a new synthetic workspace.
import assert from "node:assert/strict"
import { createHash, randomUUID } from "node:crypto"
import { mkdtemp, readFile, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { chromium, expect } from "@playwright/test"
import {
  checkGolden,
  checkLedger,
  fixtureMapping,
  literalRows,
  ocrCap,
} from "./flow-product-bulk.staging-ocr.mjs"

assert.equal(
  process.env.GENESY_STAGING_BULK_SMOKE,
  "1",
  "Explicit staging acceptance opt-in required"
)
assert.ok(
  [undefined, "1"].includes(process.env.GENESY_STAGING_BULK_OCR),
  "OCR requires explicit opt-in value 1"
)
const ocrBudget =
  process.env.GENESY_STAGING_BULK_OCR === "1"
    ? ocrCap(process.env.GENESY_STAGING_BULK_OCR_CAP_USD)
    : undefined
const api = "https://staging-api.genesy.app"
const flow = "https://staging-flow.genesy.app"
const output = await mkdtemp(join(tmpdir(), "genesy-product-bulk-staging-"))
const suffix = randomUUID().replaceAll("-", "")
const email = `bulk-stage-${suffix}@e2e.test`
const password = `BulkStage!${suffix}Aa1`
async function json(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    signal: AbortSignal.timeout(30_000),
  })
  assert.ok(
    response.ok,
    `${options.method ?? "GET"} ${new URL(url).pathname}: HTTP ${response.status}`
  )
  return (await response.json()).data
}
const session = await json(`${api}/v1/create-workspace`, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({
    first_name: "Bulk",
    last_name: "Staging QA",
    email,
    password,
    workspace_name: `bulk-stage-${suffix}`,
  }),
})
const workspace = session.workspace_ids[0]
assert.ok(workspace)
const headers = {
  Authorization: `Bearer ${session.access_token}`,
  "Content-Type": "application/json",
}
const endpoint = (path) => `${api}/v1/workspace/${workspace}${path}`
const call = (method, path, data) =>
  json(endpoint(path), {
    method,
    headers,
    ...(data === undefined ? {} : { body: JSON.stringify(data) }),
  })
async function until(read, predicate, attempts = 90) {
  let result
  for (let attempt = 0; attempt < attempts; attempt++) {
    result = await read()
    if (predicate(result)) return result
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
  throw new Error(
    `Staging condition timed out; last state ${result?.state ?? "unknown"}`
  )
}
const currencies = await call("GET", "/currencies")
if (!currencies.some((c) => c.default_currency))
  await call("POST", "/currencies", {
    name: "Euro",
    asset_type: "EUR",
    decimals: 2,
    default_currency: true,
    active: true,
  })
await call("POST", "/settings/vat-groups", {
  name: "Staging zero VAT",
  vat_amount: "0",
  default_group: true,
  country: "NL",
})
// A zero cap prevents this native CSV acceptance check from spending on Claude.
await call("POST", "/product-bulk-runs/settings", { per_run_cap_usd: "0.00" })
const seed = (name) =>
  call("POST", "/products", { name, price: "2.00", has_stock: false })
const updated = await seed("Staging blank-preservation")
const deleted = await seed("Staging separate-delete")
const conflict = await seed("Staging conflict")
const browser = await chromium.launch()
const report = {
  synthetic_only: true,
  workspace,
  checks: [],
  started_at: new Date().toISOString(),
}
async function scannedAcceptance(page, approve) {
  const evidence = { cap_usd: ocrBudget, status: "started", sources: [] }
  report.ocr = evidence
  let path
  let completed = false
  let cleanupError
  try {
    const beforeProducts = await call("GET", "/products")
    for (const code of ["USD", "EUR"]) {
      const available = await call("GET", "/currencies")
      if (
        !available.some(
          (currency) => currency.asset_type.toUpperCase() === code
        )
      )
        await call("POST", "/currencies", {
          name: code,
          asset_type: code,
          decimals: 2,
          active: true,
          default_currency: false,
        })
    }
    const available = await call("GET", "/currencies")
    const usd = available.find(
      (currency) => currency.asset_type.toUpperCase() === "USD"
    )
    const eur = available.find(
      (currency) => currency.asset_type.toUpperCase() === "EUR"
    )
    await call("POST", `/currencies/${usd.id}`, { default_currency: true })
    assert.equal(
      (await call("GET", "/currencies")).find(
        (currency) => currency.default_currency
      ).id,
      usd.id
    )
    await call("POST", "/product-bulk-runs/settings", {
      per_run_cap_usd: ocrBudget,
    })
    const run = await call("POST", "/product-bulk-runs", {
      name: "Staging OCR golden truth",
      mode: "create",
      match_key: "id",
    })
    path = `/product-bulk-runs/${run.id}`
    evidence.run_id = run.id
    const fixtures = []
    for (const name of ["scanned-catalog", "seven-page-raster-catalog"]) {
      const bytes = await readFile(
        new URL(`../fixtures/product-bulk/${name}.pdf`, import.meta.url)
      )
      const truth = JSON.parse(
        await readFile(
          new URL(
            `../fixtures/product-bulk/${name}.truth.json`,
            import.meta.url
          )
        )
      )
      const sha256 = createHash("sha256").update(bytes).digest("hex")
      if (truth.pdf_sha256) assert.equal(sha256, truth.pdf_sha256)
      const form = new FormData()
      form.append(
        "file",
        new Blob([bytes], { type: "application/pdf" }),
        `${name}.pdf`
      )
      const uploaded = await json(endpoint(`${path}/sources`), {
        method: "POST",
        headers: { Authorization: headers.Authorization },
        body: form,
      })
      const source = uploaded.sources.find(
        (source) => source.name === `${name}.pdf`
      )
      assert.ok(source)
      fixtures.push({
        source_id: source.id,
        records: (truth.records ?? truth).map((record) => ({
          ...record,
          source_id: source.id,
          page: record.page ?? 1,
        })),
      })
      evidence.sources.push({ id: source.id, name: `${name}.pdf`, sha256 })
    }
    const expected = fixtures.flatMap((fixture) => fixture.records)
    assert.equal(expected.length, 9)
    const readRun = async () => {
      const run = await call("GET", path)
      evidence.ledger = checkLedger(run.spend, ocrBudget)
      assert.ok(
        !["paused", "failed", "cancelled", "expired"].includes(run.state),
        `OCR stopped: ${run.state}`
      )
      assert.ok(
        run.sources.every((source) => source.state !== "error"),
        "OCR source failed; inspect recorded source issues"
      )
      evidence.source_status = run.sources
      return run
    }
    let current = await until(
      readRun,
      (run) =>
        run.sources.length === 2 &&
        run.sources.every((source) => source.state === "done") &&
        run.counts.total === 9 &&
        !["preparing", "extracting"].includes(run.state),
      300
    )
    assert.equal(
      evidence.ledger.reserved_usd,
      "0.000000",
      "Provider usage must settle before review"
    )
    let rows = (await call("GET", `${path}/rows?page_size=100`)).rows
    checkGolden(literalRows(rows), expected)
    assert.equal(
      current.counts.ready,
      0,
      "Unconfirmed PDF money must not be reviewable"
    )
    const seven = fixtures[1].source_id
    assert.ok(
      rows
        .filter((row) => row.source_id === seven)
        .every(
          (row) =>
            row.status === "error" &&
            row.issues.some((issue) => issue.fields?.currency_id)
        ),
      "Explicit EUR source must not inherit USD"
    )
    evidence.literal_rows = literalRows(rows)
    const stable = current.sources.map((source) => ({
      id: source.id,
      generation: source.generation,
      progress: source.progress,
    }))
    const rowIds = rows.map((row) => row.id).sort()
    const settledSpend = current.spend
    for (const source of current.sources)
      current = await call("PATCH", `${path}/mapping`, {
        revision: current.revision,
        source_id: source.id,
        header_row: 1,
        columns: fixtureMapping(source.columns[""]),
      })
    current = await until(
      readRun,
      (run) => !["preparing", "extracting"].includes(run.state)
    )
    rows = (await call("GET", `${path}/rows?page_size=100`)).rows
    assert.deepEqual(
      rows.map((row) => row.id).sort(),
      rowIds,
      "PDF mapping must not replace literal rows"
    )
    assert.deepEqual(
      current.sources.map((source) => ({
        id: source.id,
        generation: source.generation,
        progress: source.progress,
      })),
      stable,
      "PDF mapping must not restart extraction"
    )
    assert.deepEqual(
      current.spend,
      settledSpend,
      "PDF mapping must not spend again"
    )
    for (const source of current.sources)
      assert.equal(current.mapping[`${source.id}:`].header_row, 0)
    assert.ok(
      rows.every(
        (row) =>
          row.included &&
          row.status === "error" &&
          row.issues.some((issue) => issue.fields?.currency_id)
      ),
      "Mapping alone must not bypass currency"
    )
    const unrelated = rows[0]
    current = await call("PATCH", `${path}/rows/${unrelated.id}`, {
      revision: current.revision,
      values: {
        ...unrelated.values,
        name: `${unrelated.values.name} temporary`,
      },
    })
    current = await until(
      readRun,
      (run) => !["preparing", "extracting"].includes(run.state)
    )
    rows = (await call("GET", `${path}/rows?page_size=100`)).rows
    assert.equal(
      rows.find((row) => row.id === unrelated.id).status,
      "error",
      "Unrelated edit must not confirm currency"
    )
    const literal = literalRows(rows)
    checkGolden(literal, expected)
    for (const [index, row] of rows.entries()) {
      const { name, barcode, price, description } = literal[index]
      current = await call("PATCH", `${path}/rows/${row.id}`, {
        revision: current.revision,
        values: {
          name,
          barcode,
          price,
          ...(description === undefined ? {} : { description }),
          currency_id: eur.id,
        },
      })
    }
    current = await until(
      readRun,
      (run) => run.counts.ready === 9 && run.state === "ready"
    )
    rows = (await call("GET", `${path}/rows?page_size=100`)).rows
    const canonicalTruth = expected.map((record) => ({
      ...record,
      currency_id: eur.id,
    }))
    checkGolden(
      rows.map((row) => ({
        ...row.after,
        source_id: row.source_id,
        page: row.page,
        row: row.line,
      })),
      canonicalTruth,
      true
    )
    assert.deepEqual(
      (await call("GET", "/products")).map((product) => product.id).sort(),
      beforeProducts.map((product) => product.id).sort(),
      "OCR preparation must not write products"
    )
    await page.setViewportSize({ width: 1280, height: 900 })
    await page.goto(`${flow}/imports/products/${run.id}`)
    await expect(page.locator("[data-bulk-row]")).toHaveCount(9)
    const review = page
      .getByRole("heading", { name: "Creates and updates", exact: true })
      .locator("..")
    await review
      .getByRole("button", { name: "Freeze review", exact: true })
      .click()
    await expect(
      review.getByRole("button", {
        name: "Approve creates and updates",
        exact: true,
      })
    ).toBeVisible()
    await page.screenshot({
      path: join(output, "ocr-canonical-review.png"),
      fullPage: true,
    })
    await approve("write")
    current = await until(readRun, (run) => run.state === "completed")
    assert.equal(current.counts.applied, 9)
    rows = (await call("GET", `${path}/rows?page_size=100`)).rows
    const products = []
    for (const row of rows) {
      assert.equal(row.receipt?.status, "completed")
      const product = await call("GET", `/products/${row.receipt.product_id}`)
      assert.equal(product.currency, "EUR")
      products.push({
        ...product,
        currency_id: product.relationships?.currency?.id,
        source_id: row.source_id,
        page: row.page,
        row: row.line,
      })
    }
    checkGolden(products, canonicalTruth, true)
    assert.equal(new Set(rows.map((row) => row.receipt.product_id)).size, 9)
    assert.equal(evidence.ledger.reserved_usd, "0.000000")
    evidence.final_counts = current.counts
    evidence.native_products = products.map(
      ({ id, name, barcode, price, currency, currency_id, source_id }) => ({
        id,
        name,
        barcode,
        price,
        currency,
        currency_id,
        source_id,
      })
    )
    evidence.receipts = rows.map((row) => ({
      row_id: row.id,
      receipt: row.receipt,
    }))
    evidence.status = "passed"
    completed = true
    await page.screenshot({
      path: join(output, "ocr-native-receipts.png"),
      fullPage: true,
    })
  } catch (error) {
    evidence.error = error.message
    throw error
  } finally {
    // Stop new reservations even on a failed acceptance; retain actual/unknown usage in the server ledger.
    try {
      await call("POST", "/product-bulk-runs/settings", {
        per_run_cap_usd: "0.00",
      })
      if (path) {
        if (!completed) await call("POST", `${path}/cancel`)
        const latest = await call("GET", path)
        evidence.ledger = checkLedger(latest.spend, ocrBudget)
        evidence.source_status = latest.sources
        evidence.status = completed ? "passed" : "failed"
      }
    } catch (error) {
      cleanupError = error
      evidence.cleanup_error = error.message
      evidence.status = "failed"
    }
  }
  if (cleanupError) throw cleanupError
}
try {
  const page = await browser.newPage({
    viewport: { width: 1280, height: 900 },
    reducedMotion: "reduce",
  })
  const pageErrors = []
  page.on("pageerror", (error) => pageErrors.push(error.message))
  await page.goto(`${flow}/login`)
  await page.getByLabel("Email", { exact: true }).fill(email)
  await page.getByLabel("Password", { exact: true }).fill(password)
  await page.getByRole("button", { name: "Log in", exact: true }).click()
  await expect(page).toHaveURL(`${flow}/`)
  await page.goto(`${flow}/imports/products`)
  await page
    .getByLabel("Name", { exact: true })
    .fill(`Staging acceptance ${suffix.slice(0, 8)}`)
  await page
    .getByRole("combobox", { name: "Import mode", exact: true })
    .selectOption("upsert")
  await page
    .getByRole("combobox", { name: "Exact matching key", exact: true })
    .selectOption("id")
  const csv = `_operation,_key,name,price,barcode\ncreate,,Staging created,3.00,0000000042\nupdate,${updated.product_id},,7.00,\nupdate,${conflict.product_id},,6.00,\ndelete,${deleted.product_id},,,\n`
  await page.getByLabel("Source files", { exact: true }).setInputFiles({
    name: "staging.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(csv),
  })
  await page.getByRole("button", { name: "Save draft", exact: true }).click()
  await expect(page).toHaveURL(/\/imports\/products\/[0-9a-f-]{36}$/)
  const id = new URL(page.url()).pathname.split("/").at(-1)
  report.run_id = id
  const path = `/product-bulk-runs/${id}`
  await until(
    () => call("GET", path),
    (run) => run.counts.ready === 4
  )
  const scope = (kind) =>
    page
      .getByRole("heading", {
        name: kind === "write" ? "Creates and updates" : "Explicit deletions",
        exact: true,
      })
      .locator("..")
  await scope("write")
    .getByRole("button", { name: "Freeze review", exact: true })
    .click()
  await expect(
    scope("write").getByRole("button", {
      name: "Approve creates and updates",
      exact: true,
    })
  ).toBeVisible()
  await scope("delete")
    .getByRole("button", { name: "Freeze review", exact: true })
    .click()
  await expect(
    scope("delete").getByRole("button", {
      name: "Approve explicit deletions",
      exact: true,
    })
  ).toBeVisible()
  await call("PUT", `/products/${conflict.product_id}`, {
    name: "Changed after staging review",
  })
  async function approve(kind) {
    const label =
      kind === "write"
        ? "Approve creates and updates"
        : "Approve explicit deletions"
    await scope(kind).getByRole("button", { name: label, exact: true }).click()
    const dialog = page.getByRole("alertdialog", { name: label, exact: true })
    await dialog
      .getByRole("button", { name: "Approve and queue", exact: true })
      .click()
    await expect(dialog).not.toBeVisible()
  }
  await approve("write")
  await until(
    () => call("GET", path),
    (run) => run.counts.applied === 2 && run.counts.stale === 1
  )
  assert.equal(
    (await call("GET", "/products")).some((p) => p.id === deleted.product_id),
    true
  )
  report.checks.push(
    "CSV upload, frozen review, independent conflict, delete waits for separate approval"
  )
  await page.goto(`${flow}/`)
  await page.goto(`${flow}/imports/products/${id}`)
  await page.reload()
  await approve("delete")
  const final = await until(
    () => call("GET", path),
    (run) => run.state === "completed"
  )
  assert.equal(final.counts.applied, 3)
  assert.equal(final.counts.stale, 1)
  const products = await call("GET", "/products")
  const changed = products.find((p) => p.id === updated.product_id)
  assert.equal(changed.name, "Staging blank-preservation")
  assert.equal(Number(changed.price), 7)
  assert.equal(
    products.find((p) => p.name === "Staging created").barcode,
    "0000000042"
  )
  assert.equal(
    products.some((p) => p.id === deleted.product_id),
    false
  )
  assert.equal(
    Number(products.find((p) => p.id === conflict.product_id).price),
    2
  )
  const rows = await call("GET", `${path}/rows?page_size=100`)
  assert.equal(
    rows.rows.filter((row) => row.receipt?.status === "completed").length,
    3
  )
  const foreign = await fetch(`${api}/v1/workspace/${randomUUID()}${path}`, {
    headers,
  })
  assert.ok([403, 404].includes(foreign.status))
  report.checks.push(
    "Navigation/reload restore, three native receipts, leading zeros, blank preservation, tenant refusal"
  )
  const reportResponse = await fetch(endpoint(`${path}/report`), { headers })
  assert.equal(reportResponse.status, 200)
  await writeFile(join(output, "outcomes.csv"), await reportResponse.text())
  await page.screenshot({
    path: join(output, "receipt-desktop.png"),
    fullPage: true,
  })
  await page.setViewportSize({ width: 375, height: 812 })
  await page.reload()
  const width = await page.evaluate(() => {
    // eslint-disable-next-line no-undef -- Executes inside Chromium, not Node.
    const main = document.querySelector("main")
    return main ? { scroll: main.scrollWidth, client: main.clientWidth } : null
  })
  assert.ok(
    width && width.scroll <= width.client + 1,
    "Mobile main must not overflow"
  )
  await page.screenshot({
    path: join(output, "receipt-mobile.png"),
    fullPage: true,
  })
  assert.deepEqual(pageErrors, [])
  assert.equal(Number(final.spend?.actual_usd ?? 0), 0, "No provider spend")
  assert.equal(
    Number(final.spend?.reserved_usd ?? 0),
    0,
    "No pending provider reservation"
  )
  report.checks.push(
    "Downloadable report, desktop/mobile receipt, no browser exceptions"
  )
  report.final_counts = final.counts
  report.spend = final.spend
  if (ocrBudget) await scannedAcceptance(page, approve)
  assert.deepEqual(pageErrors, [])
  report.finished_at = new Date().toISOString()
  report.status = "passed"
} catch (error) {
  report.status = "failed"
  report.error = error.message
  throw error
} finally {
  await browser.close()
  await writeFile(
    join(output, "report.json"),
    JSON.stringify(report, null, 2) + "\n"
  )
  console.log(
    `Staging acceptance ${report.status}: ${join(output, "report.json")}`
  )
}
