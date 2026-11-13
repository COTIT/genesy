import { describe, expect, it } from "vitest"
import {
  bulkKey,
  bulkPath,
  bulkPollInterval,
  canEditBulk,
  mappingColumns,
  mappingParts,
  reviewFor,
  templateFields,
  validFiles,
  type BulkRun,
  type ProductField,
} from "./product-bulk"

const run = (extra: Partial<BulkRun> = {}): BulkRun => ({
  id: "run",
  name: "Import",
  revision: 2,
  state: "ready",
  counts: {},
  progress: {},
  sources: [],
  mapping: {},
  reviews: [],
  spend: {},
  mode: "update",
  match_key: "id",
  permissions: { edit: true, approve: true, cancel: true },
  ...extra,
})
describe("product bulk UI boundaries", () => {
  it("preserves explicit transforms through saved mapping edits and rejects unknown columns", () => {
    const existing = {
      Price: {
        target: "price",
        transform: { kind: "decimal", decimal_separator: "," },
      },
      ID: "_key",
    }
    const parts = mappingParts(existing)
    expect(mappingColumns(parts.columns, parts.transforms)).toEqual(existing)
    expect(mappingColumns({ Price: "price", Extra: "" }, "{}")).toEqual({
      Price: "price",
      Extra: "",
    })
    expect(() => mappingColumns({ Price: "price" }, "[]")).toThrow()
    expect(() =>
      mappingColumns({ Price: "price" }, '{"Other":{"kind":"decimal"}}')
    ).toThrow()
    expect(() =>
      mappingColumns({ Price: "price" }, '{"Price":"eval"}')
    ).toThrow()
  })
  it("isolates user/workspace caches and escapes route path segments", () => {
    expect(bulkKey("one", "alice")).not.toEqual(bulkKey("two", "alice"))
    expect(bulkKey("one", "alice")).not.toEqual(bulkKey("one", "bob"))
    expect(bulkPath("workspace/other")).toBe(
      "/workspace/workspace%2Fother/product-bulk-runs"
    )
  })
  it("polls active phases at2s, queued at10s, never hidden or terminal", () => {
    expect(bulkPollInterval(["running"], true)).toBe(2000)
    expect(bulkPollInterval(["queued"], true)).toBe(10000)
    expect(bulkPollInterval(["queued", "extracting"], true)).toBe(2000)
    expect(bulkPollInterval(["running"], false)).toBe(false)
    expect(bulkPollInterval(["completed", "cancelled", "failed"], true)).toBe(
      false
    )
  })
  it("never edits a queued group even if a stale server edit flag remains true", () => {
    expect(canEditBulk(run())).toBe(true)
    expect(canEditBulk(run({ state: "running" }))).toBe(false)
    expect(
      canEditBulk(
        run({
          reviews: [
            {
              id: "r",
              revision: 2,
              scope: "delete",
              state: "approved",
              expires_at: "2030-01-01",
              row_count: 1,
              fingerprint: "hash",
            },
          ],
        })
      )
    ).toBe(false)
    expect(
      canEditBulk(
        run({ permissions: { edit: false, approve: true, cancel: true } })
      )
    ).toBe(false)
  })
  it("requires current, unexpired, matching-scope review", () => {
    const current = {
      id: "r",
      revision: 2,
      scope: "write" as const,
      state: "pending",
      expires_at: "2030-01-01",
      row_count: 1,
      fingerprint: "hash",
    }
    const state = run({
      reviews: [{ ...current, revision: 1, id: "stale" }, current],
    })
    expect(reviewFor(state, "write", Date.parse("2029-01-01"))?.id).toBe("r")
    expect(reviewFor(state, "delete", Date.parse("2029-01-01"))).toBeUndefined()
    expect(reviewFor(state, "write", Date.parse("2030-01-02"))).toBeUndefined()
  })
  it("checks file count, size and supported extensions before uploading", () => {
    const file = (name: string, size: number) => ({ name, size }) as File
    expect(validFiles([file("catalog.XLSX", 10 * 1024 * 1024)])).toBe(true)
    expect(validFiles([file("catalog.csv", 10 * 1024 * 1024 + 1)])).toBe(false)
    expect(validFiles([file("macro.xlsm", 100)])).toBe(false)
    expect(validFiles([file("notes.txt", 100)])).toBe(false)
    expect(validFiles([file("empty.csv", 0)])).toBe(false)
    expect(
      validFiles(Array.from({ length: 11 }, () => file("one.csv", 1)))
    ).toBe(false)
  })
  it("exports only active typed custom keys with stable IDs", () => {
    const field = {
      id: "field-id",
      label: "Source",
      status: "active",
      type: "select",
      required: false,
      options: [{ id: "option-id", label: "Vendor" }],
    } as ProductField
    expect(
      templateFields({
        fields: [{ key: "ean13" }],
        custom_fields: [
          field,
          { ...field, id: "archived", status: "archived" },
        ],
      })
    ).toEqual([
      ...["_operation", "_key", "_product_id", "_clear"].map((key) => ({
        key,
        type: "text",
      })),
      { key: "ean13" },
      {
        key: "custom_fields.field-id",
        label: "Source",
        type: "select",
        required: false,
        options: [{ id: "option-id", label: "Vendor" }],
      },
    ])
  })
})
