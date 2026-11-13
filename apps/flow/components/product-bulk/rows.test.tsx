import { renderToStaticMarkup } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"
import messages from "../../messages/en/product-bulk.json"
import { BulkRowCard } from "./rows"
import type { BulkRow } from "@/lib/product-bulk"

const render = (extra: Partial<BulkRow>) =>
  renderToStaticMarkup(
    <NextIntlClientProvider locale="en" messages={messages}>
      <BulkRowCard
        row={{
          id: "row",
          ordinal: 1,
          operation: "update",
          included: true,
          status: "ready",
          target: {},
          values: {},
          issues: [],
          ...extra,
        }}
        editable={false}
        fields={[]}
        nativeFields={[]}
        raw={null}
        onEdit={() => undefined}
      />
    </NextIntlClientProvider>
  )
describe("frozen canonical product comparison", () => {
  it("displays native normalized values instead of the extracted request", () => {
    const out = render({
      values: { price: "3.155" },
      before: { name: "Rice", price: "3.00" },
      after: { name: "Rice", price: "3.16", affected_variants: 2 },
    })
    expect(out).toContain("Rice")
    expect(out).toContain("3.00")
    expect(out).toContain("3.16")
    expect(out).toContain("affected_variants")
    expect(out).not.toContain("3.155")
  })
  it("shows delete targets and canonical create defaults even with no source value", () => {
    expect(
      render({
        operation: "delete",
        before: { name: "Rice", price: "3.00" },
        after: null,
      })
    ).toContain("3.00")
    expect(
      render({
        operation: "create",
        before: null,
        after: { name: "New", currency_id: "resolved-currency" },
      })
    ).toContain("resolved-currency")
  })
})
