"use client"

import Link from "next/link"
import { useState } from "react"
import { X } from "lucide-react"
import { useTranslations } from "next-intl"
import { Button, buttonVariants } from "@genesy/ui/components/button"
import { runState, TERMINAL_STATES } from "@/lib/product-bulk"
import { useBulkRuns } from "@/lib/use-product-bulk"

/** Server state restores after reload; voice state never owns this work. */
export function ProductBulkIndicator() {
  const t = useTranslations("productBulk")
  const { query } = useBulkRuns()
  const [dismissed, setDismissed] = useState<string[]>([])
  const runs =
    query.data?.runs.filter((run) => !TERMINAL_STATES.has(run.state)) ?? []
  const latest = query.data?.runs[0]
  const run =
    runs[0] ?? (latest && !dismissed.includes(latest.id) ? latest : undefined)
  if (!run) return null
  const state = runState(run)
  return (
    <div
      className="flex min-w-0 flex-wrap items-center gap-2 rounded-xl border border-(--hair) bg-card p-2"
      data-product-bulk-indicator
    >
      <p className="mr-auto min-w-0 truncate text-sm">
        <span className="font-semibold">{run.name || t("title")}</span>
        {" · "}
        <span aria-live="polite">
          {t.has(`states.${state}`) ? t(`states.${state}`) : state}
        </span>
        {runs.length > 1 ? (
          <span className="tabular-nums">
            {" "}
            · {t("activeRuns", { count: runs.length })}
          </span>
        ) : null}
      </p>
      <Link
        href={`/imports/products/${encodeURIComponent(run.id)}`}
        className={buttonVariants({
          variant: "outline",
          className: "min-h-11 shrink-0",
        })}
      >
        {t("open")}
      </Link>
      {TERMINAL_STATES.has(run.state) ? (
        <Button
          variant="ghost"
          size="icon"
          className="min-h-11 min-w-11 shrink-0"
          aria-label={t("dismiss")}
          onClick={() => setDismissed([...dismissed, run.id])}
        >
          <X aria-hidden />
        </Button>
      ) : null}
    </div>
  )
}
