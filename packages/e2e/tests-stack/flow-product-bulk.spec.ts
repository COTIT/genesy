import { randomUUID } from "node:crypto"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { createRequire } from "node:module"
import type {
  APIRequestContext,
  Locator,
  Page,
  TestInfo,
} from "@playwright/test"
import { chromium } from "@playwright/test"
import {
  expect,
  expectNoHorizontalScroll,
  expectSingleScroller,
  markInjected,
  test,
  watchProblems,
} from "../src/test"

type Run = {
  id: string
  revision: number
  state: string
  counts: {
    total: number
    ready: number
    applied: number
    errors: number
    stale: number
  }
  sources: {
    id: string
    state: string
    columns: Record<string, string[]>
    header_rows?: Record<string, number>
    progress?: { rows_read?: number; pages_done?: number; total_pages?: number }
    issues?: unknown[]
  }[]
  reviews: { id: string; scope: string; state: string }[]
  issues?: { code: string; message: string }[]
  permissions?: { resume?: boolean }
}
type Row = {
  id: string
  operation: string
  status: string
  after: Record<string, unknown> | null
  receipt: unknown
  values: Record<string, unknown>
  issues: { code: string; message: string }[]
  raw?: { cells: string[]; columns: string[]; formula_columns: number[] } | null
}
type Product = {
  id: string
  name: string
  price: string
  ean13: string | null
  barcode?: string | null
  reference?: string | null
  custom_fields?: Record<string, unknown>
}
type Servers = { api: string; flow: string }

async function servers(info: TestInfo): Promise<Servers> {
  const override = info.config.metadata.productBulkApi
  const baseURL = info.project.use.baseURL
  const result =
    typeof override === "string" && baseURL
      ? { api: override, flow: baseURL }
      : (await import("../src/stack")).urls
  for (const value of [result.api, result.flow]) {
    const url = new URL(value)
    if (
      url.protocol !== "http:" ||
      !["127.0.0.1", "localhost"].includes(url.hostname)
    )
      throw new Error("Product bulk QA requires an isolated local HTTP stack")
  }
  return result
}

// The standalone local API may deliberately omit Office; this accepts its real readiness
// refusal only. All product import requests and browser errors remain guarded.
test.use({
  // eslint-disable-next-line no-empty-pattern -- Playwright reads fixture dependency names.
  allow: async ({}, use, info) => {
    const urls = await servers(info)
    await use({
      responses:
        typeof info.config.metadata.productBulkApi === "string"
          ? [502, 503].map((status) => ({
              url: `${urls.flow}/api/voice/readiness`,
              status,
            }))
          : [],
    })
  },
})

async function settled(
  account: Owner,
  id: string,
  states = ["ready", "needs_input", "paused", "failed"]
) {
  let run: Run | undefined
  await expect
    .poll(
      async () => {
        run = await account.call<Run>("GET", `/product-bulk-runs/${id}`)
        return states.includes(run.state)
      },
      { timeout: 45_000 }
    )
    .toBe(true)
  return run!
}
async function applyWrite(page: Page, account: Owner, id: string, applied = 1) {
  await freeze(review(page, "write"))
  await expect(
    review(page, "write").getByRole("button", {
      name: "Approve creates and updates",
      exact: true,
    })
  ).toBeVisible()
  await approve(page, "write")
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .applied,
      { timeout: 30_000 }
    )
    .toBe(applied)
}

test("upload boundaries reject unsupported, empty, oversized and malformed sources without product writes", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  await login(page, urls, account)
  let releaseIdentity!: () => void
  const identityGate = new Promise<void>((resolve) => {
    releaseIdentity = resolve
  })
  let identityFetched = false
  const identityRoute = "**/v1/me"
  await page.route(identityRoute, async (route) => {
    const response = await route.fetch()
    identityFetched = true
    await identityGate
    await route.fulfill({ response })
  })
  try {
    await page.goto(`${urls.flow}/imports/products`)
    await expect.poll(() => identityFetched).toBe(true)
    await expect(
      page.locator("[data-product-bulk-list]").getByRole("status")
    ).toBeVisible()
    await expect(page.getByLabel("Source files", { exact: true })).toHaveCount(
      0
    )
  } finally {
    releaseIdentity()
    await page.unrouteAll({ behavior: "wait" })
  }
  const input = page.getByLabel("Source files", { exact: true })
  await expect(input).toBeVisible()
  for (const file of [
    {
      name: "unsafe.exe",
      mimeType: "application/octet-stream",
      buffer: Buffer.from("MZ unsafe"),
    },
    { name: "empty.csv", mimeType: "text/csv", buffer: Buffer.alloc(0) },
    {
      name: "oversized.csv",
      mimeType: "text/csv",
      buffer: Buffer.alloc(10 * 1024 * 1024 + 1, 65),
    },
  ]) {
    await test.step(`frontend rejects ${file.name}`, async () => {
      await input.setInputFiles(file)
      await expect
        .poll(() =>
          input.evaluate((element: HTMLInputElement) =>
            Array.from(element.files ?? []).map((selected) => selected.name)
          )
        )
        .toEqual([file.name])
      await expect(
        page.getByRole("button", { name: "Save draft", exact: true })
      ).toBeDisabled()
      await expect(page.getByText(/supported, nonempty files/)).toBeVisible()
    })
  }
  await screenshot(page, info, "frontend-file-boundary")
  const run = await account.call<Run>("POST", "/product-bulk-runs", {
    name: "Backend upload boundary",
  })
  for (const file of [
    {
      name: "unsafe.exe",
      type: "application/octet-stream",
      bytes: Buffer.from("MZ unsafe"),
    },
    { name: "empty.csv", type: "text/csv", bytes: Buffer.alloc(0) },
    {
      name: "oversized.csv",
      type: "text/csv",
      bytes: Buffer.alloc(10 * 1024 * 1024 + 1, 65),
    },
  ]) {
    const result = await account.upload(
      run.id,
      file.name,
      file.bytes,
      file.type
    )
    expect([413, 415, 422], `${file.name} response`).toContain(result.status())
    expect(
      (await account.call<Run>("GET", `/product-bulk-runs/${run.id}`)).sources
    ).toHaveLength(0)
  }
  const malformed = await account.call<Run>("POST", "/product-bulk-runs", {
    name: "Malformed XLSX boundary",
  })
  const badZip = await account.upload(
    malformed.id,
    "broken.xlsx",
    Buffer.from([0x50, 0x4b, 3, 4, 0, 0, 0]),
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
  )
  if (badZip.ok()) {
    const result = await settled(account, malformed.id)
    expect(result.counts.ready).toBe(0)
    expect(result.state).not.toBe("ready")
  } else expect([413, 415, 422]).toContain(badZip.status())
  const malformedCsv = await account.call<Run>("POST", "/product-bulk-runs", {
    name: "Malformed CSV boundary",
  })
  const csvResult = await account.upload(
    malformedCsv.id,
    "ragged.csv",
    Buffer.from("name,price\nRagged valid prefix,1.25,extra\n"),
    "text/csv"
  )
  if (csvResult.ok())
    expect((await settled(account, malformedCsv.id)).counts.ready).toBe(0)
  else expect([413, 415, 422]).toContain(csvResult.status())
  expect(await account.call<Product[]>("GET", "/products")).toHaveLength(0)
})

test("unknown columns require explicit ignore and remain literal data", async ({
  page,
  request,
}, info) => {
  test.setTimeout(180_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  await login(page, urls, account)
  const tag = randomUUID().slice(0, 8)
  const unknown = await upload(
    page,
    urls,
    "Unknown column QA",
    `name,price,mystery\nUnknown ${tag},1.25,<script>window.__bulkPwned=1</script>\n`,
    "create"
  )
  await expect
    .poll(
      async () =>
        Object.keys(
          (await account.call<Run>("GET", `/product-bulk-runs/${unknown}`))
            .sources[0]?.columns ?? {}
        ).length,
      { timeout: 30_000 }
    )
    .toBeGreaterThan(0)
  expect(
    (await account.call<Run>("GET", `/product-bulk-runs/${unknown}`)).counts
      .applied
  ).toBe(0)
  const initial = await account.call<Run>(
    "GET",
    `/product-bulk-runs/${unknown}`
  )
  expect(initial.sources[0]!.header_rows?.[""]).toBe(0)
  await expect(
    page
      .locator('[id^="bulk-source-"]')
      .getByRole("spinbutton", { name: "Header row", exact: true })
  ).toHaveValue("0")
  const initialColumns = Object.values(initial.sources[0]!.columns)[0]!
  if (!initialColumns.includes("name"))
    await mapColumns(
      page,
      account,
      unknown,
      initialColumns,
      ["name", "price", ""],
      1
    )
  const headerColumns = Object.values(
    (await account.call<Run>("GET", `/product-bulk-runs/${unknown}`))
      .sources[0]!.columns
  )[0]!
  if (headerColumns.includes("name"))
    await mapColumns(
      page,
      account,
      unknown,
      headerColumns,
      ["name", "price", ""],
      1
    )
  expect(
    (await account.call<Run>("GET", `/product-bulk-runs/${unknown}`))
      .sources[0]!.header_rows?.[""]
  ).toBe(1)
  await expect(
    page
      .locator('[id^="bulk-source-"]')
      .getByRole("spinbutton", { name: "Header row", exact: true })
  ).toHaveValue("1")
  expect(
    (await account.call<Run>("GET", `/product-bulk-runs/${unknown}`)).counts
      .ready
  ).toBe(1)
  const originalCells = page.locator("[data-bulk-row]").first()
  await originalCells
    .locator("summary")
    .filter({ hasText: "Original source cells" })
    .click()
  await expect(
    originalCells
      .locator("dd")
      .filter({ hasText: "<script>window.__bulkPwned=1</script>" })
  ).toHaveText("<script>window.__bulkPwned=1</script>")
  expect(
    await page.evaluate(
      () => (window as unknown as { __bulkPwned?: boolean }).__bulkPwned
    )
  ).toBeUndefined()
})

test("duplicate headers cannot silently select one source value", async ({
  page,
  request,
}, info) => {
  const urls = await servers(info)
  const account = await owner(request, urls)
  await login(page, urls, account)
  const tag = randomUUID().slice(0, 8)
  const duplicateHeader = await upload(
    page,
    urls,
    "Duplicate header QA",
    `name,price,price\nHeader ${tag},1.25,99.99\n`,
    "create"
  )
  const headerRun = await settled(account, duplicateHeader)
  expect(headerRun.counts.ready).toBe(0)
  const headerRows = await account.call<{ rows: Row[] }>(
    "GET",
    `/product-bulk-runs/${duplicateHeader}/rows`
  )
  expect(
    JSON.stringify([
      ...(headerRun.issues ?? []),
      ...headerRun.sources.flatMap((source) => source.issues ?? []),
      ...headerRows.rows.flatMap((row) => row.issues),
    ])
  ).toMatch(/duplicate|ambiguous|header/i)
  expect(await account.call<Product[]>("GET", "/products")).toHaveLength(0)
  const [refreshed] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "GET" &&
        new URL(response.url()).pathname.endsWith(
          `/product-bulk-runs/${duplicateHeader}`
        )
    ),
    page.getByRole("button", { name: "Refresh", exact: true }).click(),
  ])
  expect(refreshed.status()).toBe(200)
  await expect(page.locator('[id^="bulk-source-"]')).toContainText(
    headerRun.sources[0]!.state
  )
  await expect(
    page.locator('[id^="bulk-source-"]').getByRole("alert")
  ).toBeVisible()
  await screenshot(page, info, "duplicate-header-errors")
})

test("duplicate exact source keys remain unresolved and preserve the existing product", async ({
  page,
  request,
}, info) => {
  const urls = await servers(info)
  const account = await owner(request, urls)
  await login(page, urls, account)
  const tag = randomUUID().slice(0, 8)
  const product = await account.call<{ product_id: string }>(
    "POST",
    "/products",
    {
      name: `Key ${tag}`,
      barcode: `KEY-${tag}`,
      price: "2.00",
      has_stock: false,
    }
  )
  const duplicateKey = await upload(
    page,
    urls,
    "Duplicate key QA",
    `_operation,_key,price\nupdate,KEY-${tag},8.00\nupdate,KEY-${tag},9.00\n`,
    "update",
    "barcode"
  )
  await mapColumns(page, account, duplicateKey, ["_operation", "_key", "price"])
  const keyRun = await settled(account, duplicateKey)
  expect(keyRun.counts.ready).toBe(0)
  expect(keyRun.counts.errors).toBe(2)
  expect(
    decimal(
      (await account.call<Product>("GET", `/products/${product.product_id}`))
        .price
    )
  ).toBe("2")
  await expect(
    review(page, "write").getByRole("button", {
      name: "Freeze review",
      exact: true,
    })
  ).toBeDisabled()
  await screenshot(page, info, "duplicate-source-key-errors")
  const excludedRow = page.locator("[data-bulk-row]").nth(1)
  await excludedRow
    .locator("summary")
    .filter({ hasText: "Correct this row" })
    .click()
  const [excludeResponse] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "PATCH" &&
        new URL(response.url()).pathname.includes(
          `/product-bulk-runs/${duplicateKey}/rows/`
        )
    ),
    excludedRow
      .getByRole("checkbox", { name: "Include in review", exact: true })
      .click(),
  ])
  expect(excludeResponse.status()).toBe(200)
  expect((await settled(account, duplicateKey)).counts.ready).toBe(1)
  await applyWrite(page, account, duplicateKey)
  expect(
    decimal(
      (await account.call<Product>("GET", `/products/${product.product_id}`))
        .price
    )
  ).toBe("8")
})

test("blank updates preserve values and explicit clear changes only named native and custom fields", async ({
  page,
  request,
}, info) => {
  test.setTimeout(180_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  const field = await account.call<{ id: string }>("POST", "/product-fields", {
    key: "qa_reference",
    label: "QA reference",
    type: "text",
    status: "active",
    required: false,
    nullable: true,
  })
  const target = await account.call<{ product_id: string }>(
    "POST",
    "/products",
    {
      name: "Original name",
      price: "2.34",
      reference: "KEEP-REFERENCE",
      custom_fields: { [field.id]: "KEEP-CUSTOM" },
      has_stock: false,
    }
  )
  await login(page, urls, account)
  const preserve = await upload(
    page,
    urls,
    "Blank preserve QA",
    `_operation,_key,name,price,reference\nupdate,${target.product_id},Preserved name,,\n`,
    "update"
  )
  await mapColumns(page, account, preserve, [
    "_operation",
    "_key",
    "name",
    "price",
    "reference",
  ])
  await applyWrite(page, account, preserve)
  const kept = await account.call<Product>(
    "GET",
    `/products/${target.product_id}`
  )
  expect(kept.name).toBe("Preserved name")
  expect(decimal(kept.price)).toBe("2.34")
  expect(kept.reference).toBe("KEEP-REFERENCE")
  expect(kept.custom_fields?.[field.id]).toBe("KEEP-CUSTOM")
  const clear = await upload(
    page,
    urls,
    "Explicit clear QA",
    `_operation,_key,_clear\nupdate,${target.product_id},"reference,custom_fields.${field.id}"\n`,
    "update"
  )
  await mapColumns(page, account, clear, ["_operation", "_key", "_clear"])
  await applyWrite(page, account, clear)
  const cleared = await account.call<Product>(
    "GET",
    `/products/${target.product_id}`
  )
  expect(cleared.reference).toBeNull()
  expect(cleared.custom_fields?.[field.id] ?? null).toBeNull()
  expect(cleared.name).toBe("Preserved name")
  expect(decimal(cleared.price)).toBe("2.34")
  await screenshot(page, info, "explicit-clear-receipt")
})

test("real request interruption and reload recover; stale review is rejected and lost approval reply cannot duplicate create", async ({
  page,
  request,
}, info) => {
  test.setTimeout(180_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  const tag = randomUUID().slice(0, 8)
  await login(page, urls, account)
  const id = await upload(
    page,
    urls,
    "Recovery QA",
    `name,price\nRecovery ${tag},2.00\n`,
    "create"
  )
  const mappingPath = `/v1/workspace/${account.workspace}/product-bulk-runs/${id}/mapping`
  markInjected(page, { test: (url) => new URL(url).pathname === mappingPath })
  const intercept = `**${mappingPath}`
  await page.route(intercept, (route) => route.abort("failed"))
  const source = page.locator('[id^="bulk-source-"]')
  await source
    .getByRole("combobox", { name: "name", exact: true })
    .selectOption("name")
  await source
    .getByRole("combobox", { name: "price", exact: true })
    .selectOption("price")
  await source
    .getByRole("button", { name: "Apply mapping and validate", exact: true })
    .click()
  await expect(
    page.locator("[data-product-bulk-detail]").getByRole("alert")
  ).toBeVisible()
  await page.unroute(intercept)
  await page.reload()
  await mapColumns(page, account, id, ["name", "price"])
  await freeze(review(page, "write"))
  await expect(
    review(page, "write").getByRole("button", {
      name: "Approve creates and updates",
      exact: true,
    })
  ).toBeVisible()
  const old = await account.call<Run>("GET", `/product-bulk-runs/${id}`)
  const oldReview = old.reviews.find(
    (entry) => entry.scope === "write" && entry.state === "pending"
  )!
  const row = page.locator("[data-bulk-row]").first()
  await row.locator("summary").filter({ hasText: "Correct this row" }).click()
  await row.getByRole("textbox", { name: "price", exact: true }).fill("4.25")
  const [edited] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "PATCH" &&
        new URL(response.url()).pathname.includes(
          `/product-bulk-runs/${id}/rows/`
        )
    ),
    row
      .getByRole("button", {
        name: "Save correction and validate",
        exact: true,
      })
      .click(),
  ])
  expect(edited.status()).toBe(200)
  await settled(account, id)
  expect(
    (
      await account.raw("POST", `/product-bulk-runs/${id}/approve`, {
        review_id: oldReview.id,
        revision: old.revision,
      })
    ).status()
  ).toBe(409)
  expect(
    await account.call<Product[]>(
      "GET",
      `/products?q=${encodeURIComponent(`Recovery ${tag}`)}`
    )
  ).toHaveLength(0)
  await expect(
    review(page, "write").getByRole("button", {
      name: "Freeze review",
      exact: true,
    })
  ).toBeVisible()
  await freeze(review(page, "write"))
  const approvePath = `/v1/workspace/${account.workspace}/product-bulk-runs/${id}/approve`
  markInjected(page, { test: (url) => new URL(url).pathname === approvePath })
  let lostReplyStatus: number | undefined
  await page.route(`**${approvePath}`, async (route) => {
    // The real backend consumes the grant, then only its response is lost.
    const response = await route.fetch()
    expect(response.status()).toBe(200)
    await route.abort("failed")
    lostReplyStatus = response.status()
  })
  await approve(page, "write")
  await expect.poll(() => lostReplyStatus).toBe(200)
  await page.unroute(`**${approvePath}`)
  await page.reload()
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .applied,
      { timeout: 30_000 }
    )
    .toBe(1)
  const products = await account.call<Product[]>(
    "GET",
    `/products?q=${encodeURIComponent(`Recovery ${tag}`)}`
  )
  expect(products).toHaveLength(1)
  expect(decimal(products[0]!.price)).toBe("4.25")
  await screenshot(page, info, "lost-reply-recovered-receipt")
})

test("cross-workspace read, source and approval are refused without leaking cached run contents", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const urls = await servers(info)
  const first = await owner(request, urls)
  const other = await owner(request, urls)
  const secretName = `Private run ${randomUUID()}`
  const run = await first.call<Run>("POST", "/product-bulk-runs", {
    name: secretName,
  })
  expect(
    (
      await first.upload(
        run.id,
        "private.csv",
        Buffer.from("name,price\nPrivate source product,1.00\n"),
        "text/csv"
      )
    ).status()
  ).toBe(200)
  await settled(first, run.id)
  const privateRow = (
    await first.call<{ rows: Row[] }>(
      "GET",
      `/product-bulk-runs/${run.id}/rows`
    )
  ).rows[0]!
  const privateSourceDenied = await other.raw(
    "GET",
    `/product-bulk-runs/${run.id}/rows/${privateRow.id}/raw`,
    undefined,
    first.workspace
  )
  expect(privateSourceDenied.status()).toBe(404)
  expect(await privateSourceDenied.text()).not.toContain(
    "Private source product"
  )
  expect(
    (
      await other.raw(
        "GET",
        `/product-bulk-runs/${run.id}`,
        undefined,
        first.workspace
      )
    ).status()
  ).toBe(404)
  expect(
    (
      await other.raw(
        "POST",
        `/product-bulk-runs/${run.id}/approve`,
        { review_id: randomUUID(), revision: run.revision },
        first.workspace
      )
    ).status()
  ).toBe(404)
  await login(page, urls, first)
  await page.goto(`${urls.flow}/imports/products/${run.id}`)
  await expect(
    page.getByRole("heading", { name: secretName, exact: true })
  ).toBeVisible()
  await tabTo(page, page.getByRole("button", { name: "Log out", exact: true }))
  await page.keyboard.press("Enter")
  await expect(page).toHaveURL(/\/login$/)
  await login(page, urls, other)
  const forbiddenPath = `/v1/workspace/${other.workspace}/product-bulk-runs/${run.id}`
  markInjected(page, { test: (url) => new URL(url).pathname === forbiddenPath })
  const [response] = await Promise.all([
    page.waitForResponse(
      (response) => new URL(response.url()).pathname === forbiddenPath
    ),
    page.goto(`${urls.flow}/imports/products/${run.id}`),
  ])
  expect(response.status()).toBe(404)
  await expect(page.locator("body")).not.toContainText(secretName)
  await expect(page.locator("main").getByRole("alert")).toBeVisible()
  await screenshot(page, info, "foreign-run-refused")
})

test("XLSX literal and formatted identifiers remain exact or visibly unresolved; cached formulas are labelled", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  const { Workbook } = createRequire(
    new URL("../../voice/package.json", import.meta.url)
  )("exceljs") as typeof import("../../voice/node_modules/exceljs")
  const book = new Workbook()
  const sheet = book.addWorksheet("Catalog")
  sheet.addRow(["name", "barcode", "price"])
  sheet.addRow(["Literal identifier", "000123", "12.50"])
  sheet.addRow(["Formatted identifier", 456, "3.00"])
  sheet.getCell("B3").numFmt = "000000"
  sheet.addRow(["Cached formula", "000789", { formula: "1/0", result: 2 }])
  const bytes = Buffer.from(await book.xlsx.writeBuffer())
  await info.attach("identifiers-formulas.xlsx", {
    body: bytes,
    contentType:
      "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  })
  await login(page, urls, account)
  const id = await uploadFile(page, urls, "XLSX shape QA", {
    name: "identifiers.xlsx",
    mimeType:
      "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    buffer: bytes,
  })
  await mapColumns(page, account, id, ["name", "barcode", "price"])
  const rows = await account.call<{ rows: Row[] }>(
    "GET",
    `/product-bulk-runs/${id}/rows?page_size=100`
  )
  const literal = rows.rows.find(
    (row) => row.values.name === "Literal identifier"
  )!
  expect(literal.values.barcode).toBe("000123")
  const formatted = rows.rows.find(
    (row) => row.values.name === "Formatted identifier"
  )!
  expect(
    formatted.values.barcode === "000456" ||
      (formatted.status !== "ready" && formatted.issues.length > 0)
  ).toBe(true)
  const formula = rows.rows.find((row) => row.values.name === "Cached formula")!
  expect(decimal(String(formula.values.price))).toBe("2")
  expect(formula.raw?.formula_columns).toContain(2)
  const card = page.locator(`[data-bulk-row="${formula.id}"]`)
  await card
    .locator("summary")
    .filter({ hasText: "Original source cells" })
    .click()
  await expect(
    card.getByText("Cached formula value; formula was not executed.", {
      exact: true,
    })
  ).toBeVisible()
  expect(await account.call<Product[]>("GET", "/products")).toHaveLength(0)
  await screenshot(page, info, "xlsx-identity-and-formula-flags")
})

test("raster-only scanned PDF pauses honestly without provider credentials and never fabricates product rows", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const fixture = info.config.metadata.scannedCatalogPath
  test.skip(
    typeof fixture !== "string" ||
      info.config.metadata.scannedNoCredentials !== true,
    "Provide verified scanned fixture and an explicitly credential-free local provider configuration"
  )
  const urls = await servers(info)
  const account = await owner(request, urls)
  const bytes = await readFile(String(fixture))
  await info.attach("scanned-catalog.pdf", {
    body: bytes,
    contentType: "application/pdf",
  })
  await login(page, urls, account)
  const id = await uploadFile(page, urls, "Scanned PDF provider pause QA", {
    name: "scanned-catalog.pdf",
    mimeType: "application/pdf",
    buffer: bytes,
  })
  const run = await settled(account, id)
  expect(["paused", "needs_input", "failed"]).toContain(run.state)
  expect(JSON.stringify(run)).toMatch(/provider|credential|config|Claude/i)
  expect(run.counts.applied).toBe(0)
  expect(
    (
      await account.call<{ rows: Row[] }>(
        "GET",
        `/product-bulk-runs/${id}/rows`
      )
    ).rows
  ).toHaveLength(0)
  expect(await account.call<Product[]>("GET", "/products")).toHaveLength(0)
  await page.reload()
  await expect(
    page.getByRole("button", { name: "Resume preparation", exact: true })
  ).toBeVisible()
  await expect(page.locator('[data-migration-step="paused"]')).toContainText(
    "Preparation is paused."
  )
  await screenshot(page, info, "scanned-pdf-provider-pause")
  const [resumed] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname.endsWith(
          `/product-bulk-runs/${id}/resume`
        )
    ),
    page
      .getByRole("button", { name: "Resume preparation", exact: true })
      .click(),
  ])
  expect(resumed.status()).toBe(200)
  expect((await settled(account, id, ["paused"])).counts.applied).toBe(0)
  expect(
    (
      await account.call<{ rows: Row[] }>(
        "GET",
        `/product-bulk-runs/${id}/rows`
      )
    ).rows
  ).toHaveLength(0)
})

async function owner(request: APIRequestContext, urls: Servers) {
  const suffix = randomUUID().replaceAll("-", "")
  const email = `bulk-${suffix}@e2e.test`
  const password = `BulkQa!${suffix}Aa1`
  const signup = await request.post(`${urls.api}/v1/create-workspace`, {
    data: {
      first_name: "Bulk",
      last_name: "QA",
      email,
      password,
      workspace_name: `bulkqa-${suffix}`,
    },
  })
  expect(signup.status(), "create isolated QA workspace").toBe(201)
  const login = (await signup.json()).data as {
    access_token: string
    workspace_ids: string[]
  }
  const workspace = login.workspace_ids[0]!
  const headers = { Authorization: `Bearer ${login.access_token}` }
  const url = (path: string) => `${urls.api}/v1/workspace/${workspace}${path}`
  const call = async <T>(
    method: string,
    path: string,
    body?: unknown
  ): Promise<T> => {
    const response = await request.fetch(url(path), {
      method,
      headers,
      data: body,
    })
    expect(
      response.ok(),
      `${method} ${path}: ${response.status()} ${response.ok() ? "" : await response.text()}`
    ).toBe(true)
    return (await response.json()).data as T
  }
  const currencies = await call<{ id: string; default_currency: boolean }[]>(
    "GET",
    "/currencies"
  )
  if (!currencies.some((currency) => currency.default_currency))
    await call("POST", "/currencies", {
      name: "Euro",
      asset_type: "EUR",
      decimals: 2,
      default_currency: true,
      active: true,
    })
  await call("POST", "/settings/vat-groups", {
    name: "QA zero VAT",
    vat_amount: "0",
    default_group: true,
    country: "NL",
  })
  await call("POST", "/product-bulk-runs/settings", { per_run_cap_usd: "1.00" })
  return {
    email,
    password,
    workspace,
    call,
    raw: (method: string, path: string, body?: unknown, scope = workspace) =>
      request.fetch(`${urls.api}/v1/workspace/${scope}${path}`, {
        method,
        headers,
        data: body,
      }),
    upload: (id: string, name: string, buffer: Buffer, mimeType: string) =>
      request.post(url(`/product-bulk-runs/${id}/sources`), {
        headers,
        multipart: { file: { name, buffer, mimeType } },
      }),
    status: async (path: string) =>
      (await request.get(url(path), { headers })).status(),
  }
}
type Owner = Awaited<ReturnType<typeof owner>>

async function login(page: Page, urls: Servers, account: Owner) {
  await page.goto(`${urls.flow}/login`)
  await page.getByLabel("Email", { exact: true }).fill(account.email)
  await page.getByLabel("Password", { exact: true }).fill(account.password)
  await page.getByRole("button", { name: "Log in", exact: true }).click()
  await expect(page).toHaveURL(`${urls.flow}/`)
}

async function screenshot(page: Page, info: TestInfo, name: string) {
  const path = info.outputPath(`${name}.png`)
  await page.screenshot({ path, fullPage: true })
  await info.attach(name, { path, contentType: "image/png" })
}
async function tabTo(page: Page, target: Locator) {
  await expect(target).toBeAttached()
  for (let step = 0; step < 160; step++) {
    if (await target.evaluate((element) => element === document.activeElement))
      return
    await page.keyboard.press("Tab")
  }
  await expect(target, "control is reachable by Tab").toBeFocused()
}
async function editableField(page: Page, row: Locator, name: string) {
  const field = row.getByRole("textbox", { name, exact: true })
  if (await field.count()) return field
  const selector = row.getByRole("combobox", {
    name: "Add native product field",
    exact: true,
  })
  await expect(selector.locator(`option[value="${name}"]`)).toBeAttached()
  await tabTo(page, selector)
  await page.keyboard.press("Home")
  await page.keyboard.type(name)
  await page.keyboard.press("Tab")
  await expect(selector).toHaveValue(name)
  await tabTo(
    page,
    row.getByRole("button", { name: "Add", exact: true }).first()
  )
  await page.keyboard.press("Enter")
  await expect(field).toBeAttached()
  return field
}

test.afterEach(async ({ page }, info) => {
  if (info.status === info.expectedStatus) return
  await screenshot(page, info, "failure")
  await info.attach("page.html", {
    body: await page.content(),
    contentType: "text/html",
  })
})

async function upload(
  page: Page,
  urls: Servers,
  name: string,
  csv: string,
  mode = "upsert",
  matchKey = "id"
) {
  return uploadFile(
    page,
    urls,
    name,
    { name: "catalog.csv", mimeType: "text/csv", buffer: Buffer.from(csv) },
    mode,
    matchKey
  )
}
async function uploadFile(
  page: Page,
  urls: Servers,
  name: string,
  file: { name: string; mimeType: string; buffer: Buffer },
  mode = "create",
  matchKey = "id"
) {
  await page.goto(`${urls.flow}/imports/products`)
  await page.getByLabel("Name", { exact: true }).fill(name)
  await page
    .getByRole("combobox", { name: "Import mode", exact: true })
    .selectOption(mode)
  await page
    .getByRole("combobox", { name: "Exact matching key", exact: true })
    .selectOption(matchKey)
  await page.getByLabel("Source files", { exact: true }).setInputFiles(file)
  await page.getByRole("button", { name: "Save draft", exact: true }).click()
  await expect(page).toHaveURL(/\/imports\/products\/[0-9a-f-]{36}$/)
  return new URL(page.url()).pathname.split("/").at(-1)!
}

async function mapColumns(
  page: Page,
  account: Owner,
  id: string,
  columns: string[],
  targets?: string[],
  headerRow?: number
) {
  await expect
    .poll(
      async () => {
        const run = await account.call<Run>("GET", `/product-bulk-runs/${id}`)
        return Object.keys(run.sources[0]?.columns ?? {}).length > 0
      },
      { timeout: 30_000 }
    )
    .toBe(true)
  const source = page.locator('[id^="bulk-source-"]')
  await expect(source).toBeVisible()
  if (headerRow !== undefined)
    await source
      .getByLabel("Header row", { exact: true })
      .fill(String(headerRow))
  for (const [index, column] of columns.entries())
    await source
      .getByRole("combobox", { name: column, exact: true })
      .selectOption(targets?.[index] ?? column)
  const [response] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "PATCH" &&
        new URL(response.url()).pathname.endsWith(
          `/product-bulk-runs/${id}/mapping`
        )
    ),
    source
      .getByRole("button", { name: "Apply mapping and validate", exact: true })
      .click(),
  ])
  expect(response.status()).toBe(200)
  const revision = (await response.json()).data.revision as number
  await expect
    .poll(
      async () => {
        const run = await account.call<Run>("GET", `/product-bulk-runs/${id}`)
        const rows = await account.call<{ rows: Row[] }>(
          "GET",
          `/product-bulk-runs/${id}/rows?page_size=100`
        )
        return (
          ["ready", "needs_input"].includes(run.state) &&
          rows.rows.every((row) => !["staged", "pending"].includes(row.status))
        )
      },
      { timeout: 30_000 }
    )
    .toBe(true)
  await expect(page.locator("[data-product-bulk-detail]")).toContainText(
    new RegExp(`Revision ${revision}\\b`)
  )
}

function review(page: Page, scope: "write" | "delete") {
  return page
    .getByRole("heading", {
      name: scope === "write" ? "Creates and updates" : "Explicit deletions",
      exact: true,
    })
    .locator("..")
}
async function freeze(card: Locator) {
  await card.getByRole("button", { name: "Freeze review", exact: true }).click()
}
async function approve(page: Page, scope: "write" | "delete") {
  const label =
    scope === "write"
      ? "Approve creates and updates"
      : "Approve explicit deletions"
  await review(page, scope)
    .getByRole("button", { name: label, exact: true })
    .click()
  const dialog = page.getByRole("alertdialog", { name: label, exact: true })
  await expect(dialog).toBeVisible()
  await dialog
    .getByRole("button", { name: "Approve and queue", exact: true })
    .click()
  await expect(dialog).not.toBeVisible()
}
const decimal = (value: string) =>
  value.includes(".") ? value.replace(/0+$/, "").replace(/\.$/, "") : value

test("actual 200% browser zoom preserves real import correction and review keyboard access", async ({
  request,
}, info) => {
  test.skip(
    info.config.metadata.actualBrowserZoom !== true,
    "Requires the full Chromium browser with an isolated persistent zoom profile"
  )
  test.setTimeout(120_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  const profile = await mkdtemp("/private/tmp/product-bulk-zoom-profile-")
  await mkdir(`${profile}/Default`)
  // Chromium ChromeZoomLevelPrefs uses x for the default storage partition; Blink uses factor=1.2^level.
  await writeFile(
    `${profile}/Default/Preferences`,
    JSON.stringify({
      partition: { default_zoom_level: { x: Math.log(2) / Math.log(1.2) } },
    })
  )
  const context = await chromium.launchPersistentContext(profile, {
    channel: "chromium",
    headless: true,
    viewport: null,
    args: ["--window-size=1280,800"],
    locale: "en-US",
  })
  const page = context.pages()[0]!
  const cdp = await context.newCDPSession(page)
  const capture = async (name: string) => {
    const path = info.outputPath(`${name}.png`)
    const { data } = await cdp.send("Page.captureScreenshot", {
      format: "png",
      fromSurface: true,
      captureBeyondViewport: false,
    })
    await writeFile(path, Buffer.from(data, "base64"))
    await info.attach(name, { path, contentType: "image/png" })
  }
  try {
    await page.goto("chrome://settings/appearance")
    const zoom = page.locator("select#zoomLevel")
    await expect(zoom).toHaveValue("2")
    await expect(zoom.locator("option:checked")).toHaveText("200%")
    await zoom.scrollIntoViewIfNeeded()
    await capture("actual-200-browser-settings")
    const problems = watchProblems(
      page,
      {
        responses: [502, 503].map((status) => ({
          url: `${urls.flow}/api/voice/readiness`,
          status,
        })),
      },
      urls.flow
    )
    await login(page, urls, account)
    const name = `Browser zoom ${randomUUID().slice(0, 8)}`
    const id = await upload(
      page,
      urls,
      "Actual 200% zoom QA",
      `name,price\n${name},not-a-decimal\n`,
      "create"
    )
    await mapColumns(page, account, id, ["name", "price"])
    expect((await settled(account, id)).counts.errors).toBe(1)
    const row = page.locator("[data-bulk-row]").first()
    await tabTo(
      page,
      row.locator("summary").filter({ hasText: "Correct this row" })
    )
    await page.keyboard.press("Enter")
    for (const [key, value] of [
      ["name", name],
      ["price", "5.00"],
    ]) {
      const field = await editableField(page, row, key!)
      await tabTo(page, field)
      await page.keyboard.press("ControlOrMeta+A")
      await page.keyboard.type(value!)
    }
    await tabTo(
      page,
      row.getByRole("button", {
        name: "Save correction and validate",
        exact: true,
      })
    )
    await page.keyboard.press("Enter")
    await expect
      .poll(
        async () =>
          (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
            .ready,
        { timeout: 30_000 }
      )
      .toBe(1)
    await expect(page.locator('[data-migration-step="approval"]')).toBeVisible()
    await expectNoHorizontalScroll(page)
    await expectSingleScroller(page)
    const metrics = await page.evaluate(() => ({
      innerWidth,
      outerWidth,
      innerHeight,
      outerHeight,
      devicePixelRatio,
      pinchScale: visualViewport?.scale,
      cssZoom: getComputedStyle(document.documentElement).zoom,
      mainWidth: document.querySelector("main")?.clientWidth,
      mainScrollWidth: document.querySelector("main")?.scrollWidth,
    }))
    const layout = await cdp.send("Page.getLayoutMetrics")
    expect(metrics.outerWidth).toBe(1280)
    expect(metrics.innerWidth).toBe(640)
    expect(metrics.pinchScale).toBe(1)
    expect(metrics.cssZoom).toBe("1")
    expect(metrics.mainScrollWidth).toBeLessThanOrEqual(metrics.mainWidth! + 1)
    expect(layout.cssVisualViewport.zoom).toBe(2)
    expect(layout.cssVisualViewport.scale).toBe(1)
    const metricsPath = info.outputPath("actual-browser-zoom-metrics.json")
    await writeFile(
      metricsPath,
      JSON.stringify(
        {
          browser: context.browser()?.version(),
          settingsZoom: "200%",
          metrics,
          layout,
        },
        null,
        2
      )
    )
    await info.attach("actual-browser-zoom-metrics.json", {
      path: metricsPath,
      contentType: "application/json",
    })
    const freezeButton = review(page, "write").getByRole("button", {
      name: "Freeze review",
      exact: true,
    })
    await tabTo(page, freezeButton)
    await expect(freezeButton).toBeInViewport()
    await page.keyboard.press("Enter")
    await expect(
      review(page, "write").getByRole("button", {
        name: "Approve creates and updates",
        exact: true,
      })
    ).toBeVisible()
    expect(await account.call<Product[]>("GET", "/products")).toHaveLength(0)
    await capture("actual-200-import-review")
    expect(problems, "zoomed page problems").toEqual([])
  } catch (error) {
    await capture("actual-200-failure")
    throw error
  } finally {
    await context.close()
    await rm(profile, { recursive: true, force: true })
  }
})

test("real CSV: canonical review, background write, reload, separate deletion and persisted products", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  const suffix = randomUUID().slice(0, 8)
  const updated = await account.call<{ product_id: string }>(
    "POST",
    "/products",
    { name: `Updated ${suffix}`, price: "2.00", has_stock: false }
  )
  const deleted = await account.call<{ product_id: string }>(
    "POST",
    "/products",
    { name: `Deleted ${suffix}`, price: "4.00", has_stock: false }
  )
  const conflicted = await account.call<{ product_id: string }>(
    "POST",
    "/products",
    { name: `Conflicted ${suffix}`, price: "2.00", has_stock: false }
  )
  const createdName = `Created ${suffix}`
  await login(page, urls, account)
  const id = await upload(
    page,
    urls,
    `CSV QA ${suffix}`,
    `_operation,_key,name,price,ean13\ncreate,,${createdName},3.155,0012345678901\nupdate,${updated.product_id},,7.335,\nupdate,${conflicted.product_id},,6.755,\ndelete,${deleted.product_id},,,\n`
  )
  await mapColumns(page, account, id, [
    "_operation",
    "_key",
    "name",
    "price",
    "ean13",
  ])
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .ready,
      { timeout: 30_000 }
    )
    .toBe(4)
  await expectNoHorizontalScroll(page)
  await screenshot(page, info, "mapped-csv")
  const beforeRows = await account.call<{ rows: Row[] }>(
    "GET",
    `/product-bulk-runs/${id}/rows?page_size=100`
  )
  const updateRow = beforeRows.rows.find((row) => row.operation === "update")!
  const canonicalPrice = String(updateRow.after!.price)
  const prepared = await account.call<Run>("GET", `/product-bulk-runs/${id}`)
  expect(prepared.sources[0]?.progress?.rows_read).toBe(4)
  await expect(page.getByRole("progressbar")).toHaveCount(0)
  await expect(page.locator('[data-migration-step="approval"]')).toContainText(
    "Your validated changes are ready for exact review."
  )
  const priceRow = page
    .locator(`[data-bulk-row="${updateRow.id}"]`)
    .getByRole("row")
    .filter({
      has: page.getByRole("rowheader", { name: "price", exact: true }),
    })
  await expect(priceRow.getByRole("cell").nth(1)).toHaveText(canonicalPrice)
  await freeze(review(page, "write"))
  await expect(
    review(page, "write").getByRole("button", {
      name: "Approve creates and updates",
      exact: true,
    })
  ).toBeVisible()
  await freeze(review(page, "delete"))
  await expect(
    review(page, "delete").getByRole("button", {
      name: "Approve explicit deletions",
      exact: true,
    })
  ).toBeVisible()
  await screenshot(page, info, "separate-reviews")
  expect(
    (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts.applied
  ).toBe(0)
  // A real foreground edit makes only this frozen row stale; independent rows must continue.
  await account.call("POST", `/products/${conflicted.product_id}`, {
    price: "9.99",
  })
  await approve(page, "write")
  // Leave immediately after acknowledgement; no browser replay drives the worker.
  await page.goto(`${urls.flow}/imports/products`)
  await page.reload()
  await expect(
    page.getByRole("heading", { name: `CSV QA ${suffix}`, exact: true })
  ).toBeVisible()
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .applied,
      { timeout: 30_000 }
    )
    .toBe(2)
  const persisted = await account.call<Product>(
    "GET",
    `/products/${updated.product_id}`
  )
  expect(decimal(persisted.price)).toBe(decimal(canonicalPrice))
  expect(
    decimal(
      (await account.call<Product>("GET", `/products/${conflicted.product_id}`))
        .price
    )
  ).toBe("9.99")
  expect(
    (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts.stale
  ).toBe(1)
  const products = await account.call<Product[]>(
    "GET",
    `/products?q=${encodeURIComponent(createdName)}`
  )
  expect(
    products.filter((product) => product.name === createdName)
  ).toHaveLength(1)
  expect(products.find((product) => product.name === createdName)?.ean13).toBe(
    "0012345678901"
  )
  expect(await account.status(`/products/${deleted.product_id}`)).toBe(200)
  await page.goto(`${urls.flow}/imports/products/${id}`)
  await page.reload()
  await approve(page, "delete")
  await expect
    .poll(() => account.status(`/products/${deleted.product_id}`), {
      timeout: 30_000,
    })
    .toBe(404)
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .applied,
      { timeout: 30_000 }
    )
    .toBe(3)
  await page.reload()
  const rows = page.getByRole("region", {
    name: "Rows and change report",
    exact: true,
  })
  await rows
    .getByRole("combobox", { name: "Status", exact: true })
    .selectOption("applied")
  await expect(rows.locator("[data-bulk-row]")).toHaveCount(3)
  await rows
    .locator("summary")
    .filter({ hasText: "Committed result" })
    .first()
    .click()
  const reportDownload = page.waitForEvent("download")
  await page
    .getByRole("button", { name: "Download full report", exact: true })
    .click()
  const downloaded = await reportDownload
  expect(downloaded.suggestedFilename()).toMatch(/\.csv$/)
  const reportPath = info.outputPath("receipts.csv")
  await downloaded.saveAs(reportPath)
  const csv = await readFile(reportPath, "utf8")
  expect(csv).toContain("receipt")
  expect(csv).toContain(conflicted.product_id)
  expect(csv).toContain("stale")
  await expectNoHorizontalScroll(page)
  await screenshot(page, info, "persisted-receipts")
})

test("invalid source values stay unresolved; exact cells, keyboard correction, mobile and zoom reflow", async ({
  page,
  request,
}, info) => {
  test.setTimeout(120_000)
  const urls = await servers(info)
  const account = await owner(request, urls)
  await login(page, urls, account)
  const name = `Unresolved ${randomUUID().slice(0, 8)}`
  const originalDescription = `Original ${"x".repeat(300)}`
  const id = await upload(
    page,
    urls,
    "Source correction QA",
    `name,price,description\n${name},not-a-decimal,${originalDescription}\n`,
    "create"
  )
  await mapColumns(page, account, id, ["name", "price", "description"])
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .errors,
      { timeout: 30_000 }
    )
    .toBeGreaterThan(0)
  await expect(page.locator('[data-migration-step="clarify"]')).toContainText(
    "Some details need a decision."
  )
  const row = page.locator("[data-bulk-row]").first()
  await tabTo(
    page,
    row.locator("summary").filter({ hasText: "Original source cells" })
  )
  await page.keyboard.press("Enter")
  await expect(
    row.locator("dd").filter({ hasText: "not-a-decimal" })
  ).toHaveText("not-a-decimal")
  await expect(
    row.getByText(/Preview is shortened to eight cells/)
  ).toBeVisible()
  await tabTo(
    page,
    row.getByRole("button", { name: "Load exact source values", exact: true })
  )
  await page.keyboard.press("Enter")
  await expect(
    row.getByRole("navigation", { name: "Source cell pages", exact: true })
  ).toBeVisible()
  await expect(
    row.locator("dd").filter({ hasText: "not-a-decimal" })
  ).toHaveText("not-a-decimal")
  await expect(
    row.locator("dd").filter({ hasText: originalDescription })
  ).toHaveText(originalDescription)
  await screenshot(page, info, "source-error-exact-cells")
  await tabTo(
    page,
    row.locator("summary").filter({ hasText: "Correct this row" })
  )
  await page.keyboard.press("Enter")
  for (const [key, value] of [
    ["name", name],
    ["price", "5.00"],
    ["description", originalDescription],
  ]) {
    const field = await editableField(page, row, key!)
    await tabTo(page, field)
    await page.keyboard.press("ControlOrMeta+A")
    await page.keyboard.type(value!)
  }
  await tabTo(
    page,
    row.getByRole("button", {
      name: "Save correction and validate",
      exact: true,
    })
  )
  await page.keyboard.press("Enter")
  await expect
    .poll(
      async () =>
        (await account.call<Run>("GET", `/product-bulk-runs/${id}`)).counts
          .ready,
      { timeout: 30_000 }
    )
    .toBe(1)
  expect(
    await account.call<Product[]>(
      "GET",
      `/products?q=${encodeURIComponent(name)}`
    )
  ).toHaveLength(0)
  await expectNoHorizontalScroll(page)
  await expect(page.locator('[data-migration-step="approval"]')).toContainText(
    "Your validated changes are ready for exact review."
  )
  await screenshot(page, info, "viewport-corrected-review")
  // 1280×800 at 200% zoom has the same 640×400 CSS viewport; matches existing Flow reflow checks.
  await page.setViewportSize({ width: 640, height: 400 })
  await expectNoHorizontalScroll(page)
  await expect
    .poll(() =>
      page
        .locator("main")
        .evaluate((main) => main.scrollWidth <= main.clientWidth + 1)
    )
    .toBe(true)
  const freezeButton = review(page, "write").getByRole("button", {
    name: "Freeze review",
    exact: true,
  })
  await tabTo(page, freezeButton)
  await expect(freezeButton).toBeInViewport()
  await page.keyboard.press("Enter")
  await expect(
    review(page, "write").getByRole("button", {
      name: "Approve creates and updates",
      exact: true,
    })
  ).toBeVisible()
  await screenshot(page, info, "zoom-200-equivalent-review")
})
