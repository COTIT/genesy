"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { useQuery } from "@tanstack/react-query"
import { Button } from "@genesy/ui/components/button"
import {
  bulkRequest,
  templateFields,
  type BulkTemplate,
} from "@/lib/product-bulk"
import { useBulkOwner } from "@/lib/use-product-bulk"
import { BulkError, panel } from "./shared"

export function download(bytes: BlobPart, name: string, type: string) {
  const url = URL.createObjectURL(new Blob([bytes], { type }))
  const anchor = document.createElement("a")
  anchor.href = url
  anchor.download = name
  anchor.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
export function TemplateDownloads({
  workspace,
  ready,
}: {
  workspace: string
  ready: boolean
}) {
  const t = useTranslations("productBulk")
  const owner = useBulkOwner()
  const [error, setError] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const template = useQuery({
    queryKey: [...owner.key, "template"],
    queryFn: ({ signal }) =>
      bulkRequest<BulkTemplate>(
        workspace,
        "/template",
        "GET",
        undefined,
        signal
      ),
    enabled: ready,
    retry: false,
  })
  const save = async (format: "csv" | "xlsx") => {
    if (!template.data) return
    setBusy(true)
    setError(null)
    try {
      const { templateCsv, templateXlsx } =
        await import("@genesy/voice/lib/product-template")
      const fields = templateFields(template.data)
      if (format === "csv")
        download(
          templateCsv(fields),
          "products-template.csv",
          "text/csv;charset=utf-8"
        )
      else
        download(
          new Uint8Array(await templateXlsx(fields, t("templateHelp"))).buffer,
          "products-template.xlsx",
          "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        )
    } catch (error) {
      setError(error)
    } finally {
      setBusy(false)
    }
  }
  return (
    <section className={panel}>
      <h2 className="text-lg font-semibold">{t("templates")}</h2>
      <p className="max-w-[65ch] text-sm text-muted-foreground">
        {t("templateHelp")}
      </p>
      <div className="flex flex-wrap gap-2">
        {(["csv", "xlsx"] as const).map((format) => (
          <Button
            key={format}
            className="min-h-11"
            variant="outline"
            disabled={!template.data || busy}
            onClick={() => void save(format)}
          >
            {t("downloadTemplate", { format: format.toUpperCase() })}
          </Button>
        ))}
      </div>
      <BulkError
        error={template.error || error}
        retry={() => {
          setError(null)
          void template.refetch()
        }}
      />
    </section>
  )
}
