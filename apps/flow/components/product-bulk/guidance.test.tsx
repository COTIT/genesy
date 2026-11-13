import { renderToStaticMarkup } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"
import messages from "../../messages/en/product-bulk.json"
import { MigrationGuidance, MigrationReviewHandoff } from "./guidance"

const html = (node: React.ReactNode) =>
  renderToStaticMarkup(
    <NextIntlClientProvider locale="en" messages={messages}>
      {node}
    </NextIntlClientProvider>
  )
describe("guided migration decisions", () => {
  it("does not interrupt automatic preparation with an approval request", () => {
    const out = html(<MigrationGuidance run={{ state: "preparing" }} />)
    expect(out).toContain("No answer is needed")
    expect(out).not.toContain("href=")
    expect(out).toContain('aria-live="polite"')
  })
  it("guides unresolved rows before final approval", () => {
    const out = html(
      <MigrationGuidance
        run={{ state: "ready", counts: { ready: 4, errors: 1 } }}
      />
    )
    expect(out).toContain('href="#bulk-rows-title"')
    expect(out).not.toContain('href="#bulk-approval-title"')
  })
  it("shows the exact Flow review without an Office login or automatic approval", () => {
    const out = html(
      <MigrationReviewHandoff href="/imports/products/01950000-0000-7000-8000-000000000001" />
    )
    expect(out).toContain('href="/imports/products/')
    expect(out).toContain("Review product changes")
    expect(out).not.toContain('target="_blank"')
    expect(out).not.toContain("Office")
  })
})
