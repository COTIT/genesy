"use client"

import Link from "next/link"
import { useTranslations } from "next-intl"
import { Button, buttonVariants } from "@genesy/ui/components/button"
import {
  errorDetails,
  displayValue,
  runState,
  type BulkRunSummary,
} from "@/lib/product-bulk"

export const panel =
  "flex min-w-0 flex-col gap-4 rounded-2xl border border-(--hair) bg-card p-4 md:p-6"
export const selectClass =
  "min-h-11 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
export const fieldClass = "flex min-w-0 flex-col gap-2 text-sm"

export function BulkError({
  error,
  retry,
}: {
  error: unknown
  retry?: () => void
}) {
  const t = useTranslations("productBulk")
  if (!error) return null
  return (
    <div
      role="alert"
      className="flex flex-wrap items-center gap-3 rounded-xl border border-destructive p-4 text-sm"
    >
      <p className="min-w-0 flex-1 break-words">
        {errorDetails(error) || t("error")}
      </p>
      {retry ? (
        <Button variant="outline" className="min-h-11" onClick={retry}>
          {t("retry")}
        </Button>
      ) : null}
    </div>
  )
}
export function BulkNav() {
  const t = useTranslations("productBulk")
  return (
    <nav aria-label={t("title")} className="flex flex-wrap gap-2">
      <Link
        href="/imports/products"
        className={buttonVariants({
          variant: "outline",
          className: "min-h-11",
        })}
      >
        {t("title")}
      </Link>
      <Link
        href="/imports/products/fields"
        className={buttonVariants({ variant: "ghost", className: "min-h-11" })}
      >
        {t("fields.title")}
      </Link>
    </nav>
  )
}
export function RunProgress({ run }: { run: BulkRunSummary }) {
  const t = useTranslations("productBulk")
  const extracting = ["extracting", "extract"].includes(run.state)
  const measuredPhase = ["preparing", "validating", "running"].includes(
    run.state
  )
  const done =
    run.state === "preparing" || run.state === "validating"
      ? run.progress?.validated
      : (run.progress?.applied ?? run.progress?.done)
  const total = run.progress?.total
  const state = runState(run)
  return (
    <div className="flex flex-col gap-2 text-sm">
      <p aria-live="polite" className="font-medium">
        {t.has(`states.${state}`) ? t(`states.${state}`) : state}
        {run.reason ? ` · ${run.reason}` : ""}
      </p>
      {run.issues?.length ? (
        <ul className="text-sm text-destructive">
          {run.issues.map((issue, index) => (
            <li key={index}>
              {issue &&
              typeof issue === "object" &&
              !Array.isArray(issue) &&
              typeof issue.message === "string"
                ? issue.message
                : displayValue(issue)}
            </li>
          ))}
        </ul>
      ) : null}
      {measuredPhase &&
      typeof done === "number" &&
      typeof total === "number" &&
      total > 0 ? (
        <>
          <label className="flex flex-col gap-1">
            {typeof run.progress?.phase === "string"
              ? run.progress?.phase
              : t("phaseProgress")}
            <progress
              value={done}
              max={total}
              className="h-2 w-full accent-(--gold)"
            />
          </label>
          <p className="tabular-nums">{t("progress", { done, total })}</p>
        </>
      ) : null}
      <dl className="flex flex-wrap gap-x-5 gap-y-2 tabular-nums">
        {Object.entries(run.counts ?? {}).map(([key, value]) => (
          <div key={key} className="flex gap-2">
            <dt className="text-muted-foreground">
              {t.has(`counts.${key}`) ? t(`counts.${key}`) : key}
            </dt>
            <dd>{value.toLocaleString()}</dd>
          </div>
        ))}
      </dl>
      {extracting ? (
        <div className="flex flex-col gap-3">
          {run.sources?.map((source) =>
            source.progress ? (
              <div key={source.id} className="flex flex-col gap-1 text-sm">
                <p className="font-medium break-words">{source.name}</p>
                {typeof source.progress.pages_done === "number" &&
                typeof source.progress.total_pages === "number" &&
                source.progress.total_pages > 0 ? (
                  <label className="flex flex-col gap-1">
                    {t("sourcePages", {
                      done: source.progress.pages_done,
                      total: source.progress.total_pages,
                    })}
                    <progress
                      value={source.progress.pages_done}
                      max={source.progress.total_pages}
                      className="h-2 w-full accent-(--gold)"
                    />
                  </label>
                ) : null}
                {typeof source.progress.rows_read === "number" ? (
                  <p className="tabular-nums">
                    {t("sourceRows", { count: source.progress.rows_read })}
                  </p>
                ) : null}
              </div>
            ) : null
          )}
        </div>
      ) : null}
      {Object.keys(run.spend ?? {}).length ? (
        <details>
          <summary className="min-h-11 cursor-pointer py-3">
            {t("spend")}
          </summary>
          <dl className="grid gap-2">
            {Object.entries(run.spend ?? {}).map(([key, value]) => (
              <div className="flex flex-wrap justify-between gap-2" key={key}>
                <dt>{key}</dt>
                <dd className="break-all tabular-nums">
                  {displayValue(value)}
                </dd>
              </div>
            ))}
          </dl>
        </details>
      ) : null}
    </div>
  )
}
