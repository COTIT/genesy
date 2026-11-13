"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { useQuery } from "@tanstack/react-query"
import { Button } from "@genesy/ui/components/button"
import {
  bulkRequest,
  type BulkRow,
  type RawCellsPage,
} from "@/lib/product-bulk"
import { useBulkOwner, usePageVisible } from "@/lib/use-product-bulk"
import { BulkError } from "./shared"

export function RawCells({
  row,
  runId,
  owner,
}: {
  row: BulkRow
  runId: string
  owner: ReturnType<typeof useBulkOwner>
}) {
  const t = useTranslations("productBulk")
  const visible = usePageVisible()
  const [open, setOpen] = useState(false)
  const [full, setFull] = useState(false)
  const [cursors, setCursors] = useState([0])
  const after = cursors[cursors.length - 1] ?? 0
  const query = useQuery({
    queryKey: [...owner.key, "raw", runId, row.id, after],
    queryFn: ({ signal }) =>
      bulkRequest<RawCellsPage>(
        owner.workspace,
        `/${encodeURIComponent(runId)}/rows/${encodeURIComponent(row.id)}/raw?after=${after}&page_size=8`,
        "GET",
        undefined,
        signal
      ),
    enabled: open && full && visible,
    retry: false,
  })
  if (!row.raw) return null
  const cells = full
    ? (query.data?.cells ?? [])
    : row.raw.cells.map((value, index) => ({
        index,
        value,
        column: row.raw?.columns[index] || `#${index + 1}`,
        formula: row.raw?.formula_columns.includes(index) ?? false,
      }))
  return (
    <details onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary className="min-h-11 cursor-pointer py-3 text-sm font-medium">
        {t("raw.title")}
      </summary>
      {open ? (
        <div className="flex flex-col gap-3">
          <p className="text-xs text-muted-foreground">{t("raw.help")}</p>
          {row.raw_truncated && !full ? (
            <p className="text-sm">{t("raw.truncated")}</p>
          ) : null}
          <dl className="grid gap-3 sm:grid-cols-2">
            {cells.map((cell) => (
              <div
                key={cell.index}
                className="min-w-0 rounded-lg border border-(--hair) p-3"
              >
                <dt className="mb-2 text-sm font-medium break-words">
                  {cell.column}
                  {cell.formula ? (
                    <span className="ml-2 font-normal text-muted-foreground">
                      {t("raw.formula")}
                    </span>
                  ) : null}
                </dt>
                <dd
                  className="max-h-64 overflow-auto text-sm break-words whitespace-pre-wrap focus-visible:outline-2 focus-visible:outline-ring"
                  tabIndex={0}
                  aria-label={cell.column}
                >
                  {cell.value || "—"}
                </dd>
              </div>
            ))}
          </dl>
          {!full ? (
            <Button
              variant="outline"
              className="min-h-11 self-start"
              onClick={() => setFull(true)}
            >
              {t("raw.exact")}
            </Button>
          ) : (
            <>
              <BulkError
                error={query.error}
                retry={() => void query.refetch()}
              />
              {query.isPending ? <p role="status">{t("loading")}</p> : null}
              <nav aria-label={t("raw.pages")} className="flex flex-wrap gap-3">
                <Button
                  className="min-h-11"
                  variant="outline"
                  disabled={cursors.length === 1 || query.isFetching}
                  onClick={() => setCursors(cursors.slice(0, -1))}
                >
                  {t("previous")}
                </Button>
                <Button
                  className="min-h-11"
                  variant="outline"
                  disabled={query.data?.next == null || query.isFetching}
                  onClick={() => {
                    if (query.data?.next != null)
                      setCursors([...cursors, query.data.next])
                  }}
                >
                  {t("next")}
                </Button>
              </nav>
            </>
          )}
        </div>
      ) : null}
    </details>
  )
}
