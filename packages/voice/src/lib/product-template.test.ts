import { describe, expect, it } from "vitest"
import { templateCsv, templateXlsx } from "./product-template"

describe("product templates", () => {
  it("escapes CSV cells and protects formula-looking values", () => {
    expect(
      templateCsv([
        { key: "ean13", example: "0012345678901" },
        { key: "name", example: '=HYPERLINK("bad")' },
      ])
    ).toBe('"ean13","name"\r\n"0012345678901","\'=HYPERLINK(""bad"")"\r\n')
  })
  it("keeps identifiers and formula-looking samples literal in XLSX", async () => {
    const bytes = await templateXlsx(
      [
        { key: "ean13", example: "0012345678901" },
        { key: "name", example: "=1+1" },
      ],
      "Review before importing"
    )
    const { Workbook } = await import("exceljs")
    const book = new Workbook()
    await book.xlsx.load(new Uint8Array(bytes).buffer)
    expect(book.getWorksheet("Products")?.getCell("A2").value).toBe(
      "0012345678901"
    )
    expect(book.getWorksheet("Products")?.getCell("B2").value).toBe("=1+1")
    expect(book.getWorksheet("Instructions")?.getCell("A1").value).toBe(
      "Review before importing"
    )
  })
})
