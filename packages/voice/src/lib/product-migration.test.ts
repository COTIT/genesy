import { describe, expect, it } from "vitest"
import { migrationGuide } from "./product-migration"

describe("migration guidance from real run facts", () => {
  it("lets preparation continue without an unnecessary user decision", () => {
    for (const state of ["extracting", "preparing", "queued", "running"])
      expect(migrationGuide({ state })).toEqual({
        step: "working",
        needs_user: false,
      })
  })
  it("asks about unresolved data before presenting approval", () => {
    expect(
      migrationGuide({ state: "ready", counts: { ready: 10, errors: 1 } }).step
    ).toBe("clarify")
    expect(
      migrationGuide({ state: "ready", counts: { ready: 10, errors: 0 } }).step
    ).toBe("approval")
  })
  it("keeps provider recovery and partial outcomes distinct from success", () => {
    expect(migrationGuide({ state: "paused" }).step).toBe("paused")
    expect(
      migrationGuide({ state: "completed", counts: { applied: 3, stale: 1 } })
        .step
    ).toBe("recovery")
    expect(
      migrationGuide({ state: "completed", counts: { applied: 4 } }).step
    ).toBe("receipt")
    expect(migrationGuide({ state: "ready", counts: {} }).step).toBe("source")
  })
})
