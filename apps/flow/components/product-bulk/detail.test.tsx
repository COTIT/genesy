import type { ComponentProps } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { NextIntlClientProvider } from "next-intl"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { describe, expect, it } from "vitest"
import messages from "../../messages/en/product-bulk.json"
import { SourceMapping } from "./detail"
import type { BulkRun } from "@/lib/product-bulk"

function markup(mediaType: string, headerRow: number) {
  const client = new QueryClient()
  const source = {
    id: "source",
    name: "Catalog",
    media_type: mediaType,
    state: "done",
    size_bytes: 100,
    columns: { "": ["Name"] },
    issues: [],
  }
  const run = {
    revision: 1,
    mapping: {
      "source:": { header_row: headerRow, columns: { Name: "name" } },
    },
  } as unknown as BulkRun
  const owner: ComponentProps<typeof SourceMapping>["owner"] = {
    workspace: "workspace",
    user: "user",
    ready: true,
    key: ["product-bulk", "workspace", "user"],
    rights: { read: true, may: () => true },
  }
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <NextIntlClientProvider locale="en" timeZone="UTC" messages={messages}>
        <SourceMapping
          source={source}
          run={run}
          owner={owner}
          disabled={false}
          submit={() => {}}
        />
      </NextIntlClientProvider>
    </QueryClientProvider>
  )
}

describe("source mapping recovery", () => {
  it.each(["application/pdf", "text/plain"])(
    "keeps %s mapping actionable without a spreadsheet header",
    (mediaType) => {
      const out = markup(mediaType, 1)
      expect(out).not.toContain('type="number"')
      const button = out.match(
        /<button\b[^>]*>Apply mapping and validate<\/button>/
      )?.[0]
      expect(button).toBeDefined()
      expect(button).not.toMatch(/\sdisabled(?:=|\s|>)/)
      expect(button).not.toContain('aria-disabled="true"')
    }
  )
  it("permits an explicit headerless spreadsheet mapping", () => {
    const out = markup("text/csv", 0)
    expect(out).toContain('value="0"')
    const button = out.match(
      /<button\b[^>]*>Apply mapping and validate<\/button>/
    )?.[0]
    expect(button).toBeDefined()
    expect(button).not.toMatch(/\sdisabled(?:=|\s|>)/)
    expect(button).not.toContain('aria-disabled="true"')
  })
})
