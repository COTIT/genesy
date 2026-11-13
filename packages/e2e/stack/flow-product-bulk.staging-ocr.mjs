import assert from "node:assert/strict"

export const PRIOR_USD = "0.110652"
const GLOBAL_MICROS = 3_000_000n
const REMAINING_MICROS = 2_889_348n

function micros(value) {
  assert.equal(typeof value, "string", "USD amounts must be exact strings")
  assert.match(value, /^(0|[1-9]\d*)(\.\d{1,6})?$/, "Invalid exact USD amount")
  const [whole, fraction = ""] = value.split(".")
  return BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0"))
}
function usd(value) {
  return `${value / 1_000_000n}.${String(value % 1_000_000n).padStart(6, "0")}`
}
export function ocrCap(value) {
  const cap = micros(value)
  assert.ok(
    cap > 0n && cap <= REMAINING_MICROS,
    "OCR cap exceeds approved remaining $2.889348"
  )
  return usd(cap)
}
export function checkLedger(spend, cap) {
  const actual = micros(spend.actual_usd)
  const reserved = micros(spend.reserved_usd)
  assert.ok(
    actual + reserved <= micros(cap),
    "Actual plus reserved exceeds OCR cap"
  )
  assert.ok(
    micros(PRIOR_USD) + actual + reserved <= GLOBAL_MICROS,
    "Aggregate provider budget exceeds $3"
  )
  return {
    prior_actual_usd: PRIOR_USD,
    cap_usd: cap,
    actual_usd: usd(actual),
    reserved_usd: usd(reserved),
    aggregate_actual_usd: usd(micros(PRIOR_USD) + actual),
    aggregate_actual_plus_reserved_usd: usd(
      micros(PRIOR_USD) + actual + reserved
    ),
  }
}

// These are the operator's explicit fixture interpretations, including Code→barcode and Price→net price.
export function fixtureMapping(columns) {
  const targets = {
    name: "name",
    product: "name",
    productname: "name",
    barcode: "barcode",
    code: "barcode",
    price: "price",
    priceeur: "price",
    netprice: "price",
    netpriceeur: "price",
    description: "description",
  }
  const mapping = Object.fromEntries(
    columns.map((column) => {
      const target = targets[column.toLowerCase().replace(/[^a-z]/g, "")]
      assert.ok(target, `Unexpected fixture column ${column}`)
      return [column, target]
    })
  )
  for (const required of ["name", "barcode", "price"])
    assert.ok(
      Object.values(mapping).includes(required),
      `Missing fixture ${required} column`
    )
  assert.equal(
    new Set(Object.values(mapping)).size,
    columns.length,
    "Duplicate fixture columns"
  )
  return mapping
}
export function literalRows(rows) {
  return rows.map((row) => {
    assert.ok(
      row.raw && !row.raw_truncated,
      "Exact fixture cells must remain available"
    )
    const mapping = fixtureMapping(row.raw.columns)
    const values = Object.fromEntries(
      row.raw.columns.map((column, index) => [
        mapping[column],
        row.raw.cells[index],
      ])
    )
    return {
      ...values,
      source_id: row.source_id,
      page: row.page,
      row: row.line,
    }
  })
}
function decimal(value) {
  assert.equal(
    typeof value,
    "string",
    "Prices must remain exact decimal strings"
  )
  assert.match(value, /^\d+(\.\d+)?$/)
  return value.includes(".")
    ? value.replace(/0+$/, "").replace(/\.$/, "")
    : value
}
export function checkGolden(actual, expected, canonicalPrices = false) {
  assert.equal(actual.length, expected.length, "Missing or extra OCR records")
  const found = new Map()
  for (const record of actual) {
    const key = `${record.source_id}:${record.barcode}`
    assert.ok(!found.has(key), "Duplicate/context-only OCR product")
    found.set(key, record)
  }
  for (const truth of expected) {
    const record = found.get(`${truth.source_id}:${truth.barcode}`)
    assert.ok(record, `Missing literal barcode ${truth.barcode}`)
    assert.equal(record.name, truth.name, "Product name changed")
    assert.equal(
      canonicalPrices ? decimal(record.price) : record.price,
      canonicalPrices ? decimal(truth.price) : truth.price,
      "Price changed"
    )
    if (truth.description !== undefined)
      assert.equal(
        record.description.replace(/\s+/g, " ").trim(),
        truth.description.replace(/\s+/g, " ").trim(),
        "Description changed beyond whitespace"
      )
    if (truth.page !== undefined)
      assert.equal(record.page, truth.page, "Primary page ownership changed")
    if (truth.row !== undefined)
      assert.equal(record.row, truth.row, "Primary body row changed")
    if (truth.currency_id !== undefined)
      assert.equal(
        record.currency_id,
        truth.currency_id,
        "Wrong native currency"
      )
  }
}
