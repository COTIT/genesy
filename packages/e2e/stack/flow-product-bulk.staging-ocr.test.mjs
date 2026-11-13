import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import {
  ocrCap,
  checkLedger,
  checkGolden,
  fixtureMapping,
  literalRows,
} from "./flow-product-bulk.staging-ocr.mjs"

test("OCR cap and ledger enforce exact approved aggregate without floats", () => {
  assert.equal(ocrCap("2.889348"), "2.889348")
  for (const bad of [
    undefined,
    "0",
    "-1",
    "2.889349",
    "3",
    "1e0",
    "0.0000001",
    "NaN",
  ])
    assert.throws(() => ocrCap(bad))
  assert.equal(
    checkLedger({ actual_usd: "0.1", reserved_usd: "2.789348" }, "2.889348")
      .aggregate_actual_plus_reserved_usd,
    "3.000000"
  )
  assert.throws(() =>
    checkLedger({ actual_usd: "0.1", reserved_usd: "2.789349" }, "2.889348")
  )
  assert.throws(() => checkLedger({ actual_usd: "0.1" }, "2.889348"))
})

const one = JSON.parse(
  await readFile(
    new URL(
      "../fixtures/product-bulk/scanned-catalog.truth.json",
      import.meta.url
    )
  )
)
const seven = JSON.parse(
  await readFile(
    new URL(
      "../fixtures/product-bulk/seven-page-raster-catalog.truth.json",
      import.meta.url
    )
  )
)
const expected = [
  ...one.map((row) => ({ ...row, source_id: "one", page: 1 })),
  ...seven.records.map((row) => ({ ...row, source_id: "seven" })),
]

test("golden comparison covers nine rows, literal identifiers and cross-page ownership", () => {
  checkGolden(structuredClone(expected), expected)
  for (const change of [
    (rows) => rows.pop(),
    (rows) => {
      rows[0].barcode = "123456789"
    },
    (rows) => {
      rows[6].description += " invented"
    },
    (rows) => {
      rows[7].page = 5
    },
    (rows) => {
      rows[7].row = 1
    },
    (rows) => {
      rows[0] = structuredClone(rows[1])
    },
  ]) {
    const changed = structuredClone(expected)
    change(changed)
    assert.throws(() => checkGolden(changed, expected))
  }
  const whitespace = structuredClone(expected)
  whitespace[6].description = whitespace[6].description.replaceAll(" ", "\n ")
  checkGolden(whitespace, expected)
})

test("native currency and exact decimal comparison reject false success", () => {
  const truth = [{ ...expected[0], currency_id: "eur" }]
  checkGolden([{ ...truth[0], price: "12.500" }], truth, true)
  assert.throws(() =>
    checkGolden([{ ...truth[0], currency_id: "usd" }], truth, true)
  )
  assert.throws(() =>
    checkGolden([{ ...truth[0], price: "12.51" }], truth, true)
  )
})

test("literal projection never silently accepts shortened cells or unknown columns", () => {
  const raw = {
    source_id: "one",
    page: 1,
    line: 1,
    raw: {
      columns: ["Name", "Code", "Price"],
      cells: [one[0].name, one[0].barcode, one[0].price],
    },
  }
  checkGolden(literalRows([raw]), [expected[0]])
  assert.equal(
    fixtureMapping(["Name", "Barcode", "Net price EUR", "Description"])[
      "Net price EUR"
    ],
    "price"
  )
  assert.throws(() => literalRows([{ ...raw, raw_truncated: true }]))
  assert.throws(() => fixtureMapping(["Name", "Price", "Unknown"]))
  assert.throws(() => fixtureMapping(["Name", "Barcode", "Price", "Net price"]))
})
