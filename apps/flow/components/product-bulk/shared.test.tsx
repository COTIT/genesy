import { renderToStaticMarkup } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"
import messages from "../../messages/en/product-bulk.json"
import { RunProgress } from "./shared"
import type { BulkRun } from "@/lib/product-bulk"

const html = (run: Partial<BulkRun>) =>
  renderToStaticMarkup(
    <NextIntlClientProvider locale="en" messages={messages}>
      <RunProgress
        run={
          {
            state: "running",
            counts: { total: 50000, applied: 1234 },
            progress: {},
            spend: {},
            ...run,
          } as BulkRun
        }
      />
    </NextIntlClientProvider>
  )
describe("honest accessible import progress", () => {
  it("renders the sparse list response without inventing detail counters", () => {
    const out = html({
      state: "completed",
      counts: undefined,
      progress: undefined,
      spend: undefined,
    })
    expect(out).toContain("Completed")
    expect(out).not.toContain("<progress")
    expect(out).not.toContain("50,000")
  })
  it("shows phase counters with native labelled progress semantics", () => {
    const out = html({ progress: { phase: "Execution", done: 12, total: 100 } })
    expect(out).toContain('<progress value="12" max="100"')
    expect(out).toContain("12 of 100 in this phase")
    expect(out).toContain('aria-live="polite"')
    expect(out).toContain("Execution")
    expect(out).not.toContain("ETA")
  })
  it("does not present committed counts as preparation progress once ready", () => {
    const out = html({
      state: "ready",
      progress: { validated: 1, applied: 0, total: 1 },
    })
    expect(out).toContain("Ready for review")
    expect(out).not.toContain("<progress")
  })
  it("does not invent a percentage when total is unknown", () => {
    const out = html({ state: "extracting", progress: { done: 12 } })
    expect(out).not.toContain("<progress")
    expect(out).toContain("Extracting")
    expect(out).not.toContain("%")
  })
  it("uses source units during extraction and validation counters during preparation", () => {
    const extracting = html({
      state: "extracting",
      progress: { done: 0, total: 20 },
      sources: [
        {
          id: "source",
          name: "Catalog",
          state: "extracting",
          columns: {},
          issues: [],
          size_bytes: 100,
          progress: { pages_done: 3, total_pages: 9 },
        },
      ],
    })
    expect(extracting).toContain('value="3" max="9"')
    expect(extracting).not.toContain('value="0" max="20"')
    const preparing = html({
      state: "preparing",
      progress: { done: 0, total: 50000, validated: 120 },
    })
    expect(preparing).toContain('value="120" max="50000"')
  })
})
