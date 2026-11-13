export type ProductTemplateField = {
  key: string
  label?: string
  type?: string
  required?: boolean
  example?: unknown
  options?: { id: string; label: string }[]
}

/** CSV templates contain literal text, including untrusted definition labels/examples. */
export function templateCsv(fields: ProductTemplateField[]): string {
  const cell = (value: unknown) => {
    const text =
      value == null
        ? ""
        : typeof value === "object"
          ? JSON.stringify(value)
          : String(value)
    const safe = /^[=+\-@\t\r\n]/.test(text) ? `'${text}` : text
    return `"${safe.replaceAll('"', '""')}"`
  }
  return (
    [
      fields.map((field) => cell(field.key)),
      fields.map((field) => cell(field.example)),
    ]
      .map((row) => row.join(","))
      .join("\r\n") + "\r\n"
  )
}

/** Lazy ExcelJS keeps workbook code out of normal import-page startup. */
export async function templateXlsx(
  fields: ProductTemplateField[],
  instructions: string
): Promise<Uint8Array> {
  const { Workbook } = await import("exceljs")
  const workbook = new Workbook()
  const sheet = workbook.addWorksheet("Products")
  sheet.addRow(fields.map((field) => field.key))
  sheet.getRow(1).font = { bold: true }
  sheet.addRow(
    fields.map((field) =>
      field.example == null
        ? ""
        : typeof field.example === "object"
          ? JSON.stringify(field.example)
          : String(field.example)
    )
  )
  sheet.views = [{ state: "frozen", ySplit: 1 }]
  fields.forEach((_field, index) => {
    sheet.getColumn(index + 1).width = 24
    sheet.getColumn(index + 1).numFmt = "@"
  })
  const help = workbook.addWorksheet("Instructions")
  help.addRow([instructions])
  help.addRow(["Key", "Label", "Type", "Required", "Options"])
  fields.forEach((field) =>
    help.addRow([
      field.key,
      field.label ?? "",
      field.type ?? "text",
      Boolean(field.required),
      field.options
        ?.map((option) => `${option.id}: ${option.label}`)
        .join("; ") ?? "",
    ])
  )
  help.getColumn(1).width = 50
  help.getColumn(2).width = 30
  help.getColumn(5).width = 60
  return new Uint8Array(await workbook.xlsx.writeBuffer())
}
