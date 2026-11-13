"use client"

import { useEffect, useState } from "react"
import { useRouter } from "next/navigation"
import { useTranslations } from "next-intl"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { apiRequest } from "@genesy/api-client/mutator"
import { sessionStore } from "@genesy/api-client/session"
import { Button } from "@genesy/ui/components/button"
import { Input } from "@genesy/ui/components/input"
import { Textarea } from "@genesy/ui/components/textarea"
import { ConfirmDialog } from "@genesy/ui/components/confirm-dialog"
import {
  bulkPath,
  bulkPollInterval,
  bulkRequest,
  canEditBulk,
  displayValue,
  mappingColumns,
  mappingParts,
  reviewFor,
  templateFields,
  TERMINAL_STATES,
  validFiles,
  type BulkRun,
  type BulkReview,
  type BulkSource,
  type BulkTemplate,
  type SavedMapping,
  type MappingColumns,
} from "@/lib/product-bulk"
import { useBulkOwner, usePageVisible } from "@/lib/use-product-bulk"
import {
  BulkError,
  BulkNav,
  RunProgress,
  fieldClass,
  panel,
  selectClass,
} from "./shared"
import { BulkRows } from "./rows"
import { download } from "./templates"
import { ScopeRequest } from "./scope"
import { MigrationGuidance } from "./guidance"

type Owner = ReturnType<typeof useBulkOwner>
export function ProductBulkDetail({ id }: { id: string }) {
  const owner = useBulkOwner()
  return (
    <Detail
      key={`${owner.workspace}:${owner.user}:${id}`}
      id={id}
      owner={owner}
    />
  )
}
function Detail({ id, owner }: { id: string; owner: Owner }) {
  const t = useTranslations("productBulk")
  const client = useQueryClient()
  const router = useRouter()
  const visible = usePageVisible()
  const [text, setText] = useState("")
  const [files, setFiles] = useState<File[]>([])
  const query = useQuery({
    queryKey: [...owner.key, "run", id],
    queryFn: ({ signal }) =>
      bulkRequest<BulkRun>(owner.workspace, `/${id}`, "GET", undefined, signal),
    enabled: owner.ready && visible,
    refetchInterval: (query) =>
      bulkPollInterval(
        query.state.data ? [query.state.data.state] : [],
        visible
      ),
    refetchOnWindowFocus: "always",
    retry: false,
  })
  const run = query.data
  const action = useMutation({
    mutationFn: ({
      suffix,
      method = "POST",
      body,
    }: {
      suffix: string
      method?: string
      body?: unknown
    }) =>
      bulkRequest<BulkRun | BulkReview>(
        owner.workspace,
        `/${id}${suffix}`,
        method,
        body
      ),
    onSuccess: async (result, input) => {
      await client.invalidateQueries({ queryKey: owner.key })
      // Restore the explicit edit's location after its revision remount; polling never moves focus.
      const anchor = input.suffix.startsWith("/rows/")
        ? `bulk-row-${input.suffix.slice(6)}`
        : input.suffix === "/mapping"
          ? `bulk-source-${(input.body as { source_id: string }).source_id}`
          : null
      if (anchor)
        requestAnimationFrame(() =>
          document.getElementById(anchor)?.focus({ preventScroll: true })
        )
      if (
        input.suffix === "/remainder" &&
        sessionStore.getState().workspaceId === owner.workspace
      )
        router.push(`/imports/products/${result.id}`)
      if (input.suffix === "/stage") setText("")
    },
    onError: () => void client.invalidateQueries({ queryKey: owner.key }),
  })
  const upload = useMutation({
    mutationFn: async () => {
      for (const file of files) {
        const form = new FormData()
        form.set("file", file)
        await bulkRequest(owner.workspace, `/${id}/sources`, "POST", form)
      }
    },
    onSuccess: () => {
      setFiles([])
      void client.invalidateQueries({ queryKey: owner.key })
    },
    onError: () => void client.invalidateQueries({ queryKey: owner.key }),
  })
  const report = useMutation({
    mutationFn: () =>
      apiRequest<Blob>({
        url: bulkPath(owner.workspace, `/${id}/report`),
        responseType: "blob",
      }),
    onSuccess: (blob) =>
      download(blob, `product-import-${id}.csv`, "text/csv;charset=utf-8"),
  })
  const busy = action.isPending || upload.isPending
  const edit = run ? canEditBulk(run) : false
  return (
    <section
      className="mx-auto flex w-full max-w-6xl flex-col gap-5 pb-8"
      data-product-bulk-detail
    >
      <BulkNav />
      <BulkError error={query.error} retry={() => void query.refetch()} />
      {query.isPending ? <p role="status">{t("loading")}</p> : null}
      {run ? (
        <>
          <header className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <h1 className="text-2xl font-semibold tracking-tight break-words">
                {run.name || t("untitled")}
              </h1>
              <p className="mt-2 text-sm text-muted-foreground">
                {t("revision", { revision: run.revision })} ·{" "}
                {t(`modes.${run.mode}`)} · {run.match_key}
              </p>
            </div>
            <Button
              variant="outline"
              className="min-h-11"
              onClick={() => void query.refetch()}
            >
              {t("refresh")}
            </Button>
          </header>
          <section className={panel} aria-label={t("status")}>
            <RunProgress run={run} />
            <p className="text-sm text-muted-foreground">
              {t("backgroundHelp")}
            </p>
            {run.instruction ? (
              <p className="max-w-[65ch] text-sm whitespace-pre-wrap">
                {run.instruction}
              </p>
            ) : null}
          </section>
          <MigrationGuidance run={run} />
          <BulkError
            error={action.error || upload.error || report.error}
            retry={() => {
              action.reset()
              upload.reset()
              report.reset()
              void query.refetch()
            }}
          />
          {edit ? (
            <section className={panel}>
              <h2 className="text-lg font-semibold">{t("sources")}</h2>
              <form
                className="flex flex-col gap-3"
                onSubmit={(event) => {
                  event.preventDefault()
                  upload.mutate()
                }}
              >
                <label className={fieldClass}>
                  {t("files")}
                  <Input
                    className="min-h-11"
                    type="file"
                    accept=".xlsx,.csv,.pdf"
                    multiple
                    onChange={(event) =>
                      setFiles(Array.from(event.target.files ?? []))
                    }
                  />
                </label>
                <p className="text-sm text-muted-foreground">{t("fileHelp")}</p>
                {!validFiles(files) ||
                files.length + run.sources.length > 10 ? (
                  <p role="alert">{t("fileInvalid")}</p>
                ) : null}
                <Button
                  className="min-h-11 self-start"
                  type="submit"
                  disabled={
                    busy ||
                    !files.length ||
                    !validFiles(files) ||
                    files.length + run.sources.length > 10
                  }
                >
                  {t("upload")}
                </Button>
              </form>
              <form
                className="flex flex-col gap-3 border-t border-(--hair) pt-4"
                onSubmit={(event) => {
                  event.preventDefault()
                  action.mutate({
                    suffix: "/stage",
                    body: { revision: run.revision, text },
                  })
                }}
              >
                <label className={fieldClass}>
                  {t("pasteText")}
                  <Textarea
                    rows={4}
                    value={text}
                    maxLength={200000}
                    onChange={(event) => setText(event.target.value)}
                    required
                  />
                </label>
                <p className="text-sm text-muted-foreground">
                  {t("pasteHelp")}
                </p>
                <Button
                  type="submit"
                  className="min-h-11 self-start"
                  disabled={busy || !text.trim()}
                >
                  {t("prepareText")}
                </Button>
              </form>
            </section>
          ) : null}
          {edit ? (
            <ScopeRequest
              run={run}
              owner={owner}
              disabled={busy}
              submit={(body) => action.mutate({ suffix: "/stage", body })}
            />
          ) : null}
          {run.sources.length ? (
            <section className={panel}>
              <h2 className="text-lg font-semibold">{t("mapping")}</h2>
              <p className="text-sm text-muted-foreground">
                {t("mappingHelp")}
              </p>
              {run.sources.map((source) => (
                <SourceMapping
                  key={`${source.id}:${run.revision}:${JSON.stringify([Object.keys(source.columns), source.header_rows])}`}
                  source={source}
                  run={run}
                  owner={owner}
                  disabled={busy || !edit}
                  submit={(body) =>
                    action.mutate({ suffix: "/mapping", method: "PATCH", body })
                  }
                />
              ))}
            </section>
          ) : null}
          <BulkRows
            run={run}
            owner={owner}
            busy={busy}
            onEdit={(row, body) =>
              action.mutate({
                suffix: `/rows/${row}`,
                method: "PATCH",
                body: { revision: run.revision, ...body },
              })
            }
          />
          <section className={panel}>
            <h2
              id="bulk-approval-title"
              tabIndex={-1}
              className="text-lg font-semibold"
            >
              {t("approvalTitle")}
            </h2>
            <p className="max-w-[65ch] text-sm text-muted-foreground">
              {t("approvalHelp")}
            </p>
            <div className="grid gap-4 md:grid-cols-2">
              {(["write", "delete"] as const).map((scope) => (
                <Approval
                  key={scope}
                  run={run}
                  scope={scope}
                  busy={busy}
                  create={() =>
                    action.mutate({
                      suffix: "/reviews",
                      body: { revision: run.revision, scope },
                    })
                  }
                  approve={(review) =>
                    action.mutate({
                      suffix: "/approve",
                      body: { revision: run.revision, review_id: review.id },
                    })
                  }
                />
              ))}
            </div>
          </section>
          <div className="flex flex-wrap gap-3">
            {run.permissions.resume ? (
              <Button
                className="min-h-11"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  action.mutate({
                    suffix: "/resume",
                    body: { revision: run.revision },
                  })
                }
              >
                {t("resumePreparation")}
              </Button>
            ) : null}
            <Button
              className="min-h-11"
              variant="outline"
              disabled={report.isPending}
              onClick={() => report.mutate()}
            >
              {t(report.isPending ? "downloading" : "report")}
            </Button>
            {run.permissions.cancel && !TERMINAL_STATES.has(run.state) ? (
              <ConfirmDialog
                trigger={t("cancelRun")}
                title={t("cancelTitle")}
                description={t("cancelHelp")}
                confirm={t("cancelRun")}
                cancel={t("keepRun")}
                onConfirm={() => action.mutate({ suffix: "/cancel" })}
                disabled={busy}
              />
            ) : null}
            {TERMINAL_STATES.has(run.state) || run.state === "needs_input" ? (
              <Button
                className="min-h-11"
                variant="outline"
                disabled={busy}
                onClick={() => action.mutate({ suffix: "/remainder" })}
              >
                {t("remainder")}
              </Button>
            ) : null}
          </div>
        </>
      ) : null}
    </section>
  )
}

function Approval({
  run,
  scope,
  busy,
  create,
  approve,
}: {
  run: BulkRun
  scope: BulkReview["scope"]
  busy: boolean
  create: () => void
  approve: (review: BulkReview) => void
}) {
  const t = useTranslations("productBulk")
  const [now, setNow] = useState(() => Date.now())
  const review = reviewFor(run, scope, now)
  useEffect(() => {
    if (!review) return
    const timer = setTimeout(
      () => setNow(Date.now()),
      Math.max(1, Date.parse(review.expires_at) - Date.now() + 1)
    )
    return () => clearTimeout(timer)
  }, [review])
  const count =
    scope === "delete"
      ? (run.counts.deletes ?? 0)
      : Math.max(0, (run.counts.ready ?? 0) - (run.counts.deletes ?? 0))
  return (
    <div className="flex min-w-0 flex-col gap-3 rounded-xl border border-(--hair) p-4 focus-visible:outline-2 focus-visible:outline-ring">
      <h3 className="font-semibold">
        {t(scope === "delete" ? "deleteReview" : "writeReview")}
      </h3>
      <p className="text-sm tabular-nums">
        {t("reviewCount", { count: review?.row_count ?? count })}
      </p>
      {review ? (
        <>
          <p className="text-sm text-muted-foreground">
            {t("reviewExpires", {
              date: new Date(review.expires_at).toLocaleString(),
            })}
          </p>
          <ConfirmDialog
            trigger={t(scope === "delete" ? "approveDelete" : "approveWrite")}
            title={t(scope === "delete" ? "approveDelete" : "approveWrite")}
            description={t(
              scope === "delete" ? "deleteConfirm" : "writeConfirm",
              { count: review.row_count, revision: run.revision }
            )}
            confirm={t("queueApproved")}
            cancel={t("backToReview")}
            destructive={scope === "delete"}
            disabled={
              busy ||
              !run.permissions.approve ||
              new Date(review.expires_at).getTime() <= now
            }
            onConfirm={() => approve(review)}
          />
        </>
      ) : (
        <Button
          className="min-h-11 self-start"
          variant="outline"
          disabled={
            busy ||
            !run.permissions.approve ||
            !count ||
            !["ready", "needs_input"].includes(run.state)
          }
          onClick={create}
        >
          {t("prepareReview")}
        </Button>
      )}
    </div>
  )
}

export function SourceMapping({
  source,
  run,
  owner,
  disabled,
  submit,
}: {
  source: BulkSource
  run: BulkRun
  owner: Owner
  disabled: boolean
  submit: (body: unknown) => void
}) {
  const t = useTranslations("productBulk")
  const client = useQueryClient()
  const [sheet, setSheet] = useState(Object.keys(source.columns)[0] ?? "")
  const current = run.mapping[`${source.id}:${sheet}`] as
    { columns?: MappingColumns; header_row?: number } | undefined
  const hasHeaderRow = !["application/pdf", "text/plain"].includes(
    source.media_type ?? ""
  )
  const [headerRow, setHeaderRow] = useState(
    String(
      hasHeaderRow
        ? (current?.header_row ?? source.header_rows?.[sheet] ?? 1)
        : 0
    )
  )
  const effectiveHeaderRow = hasHeaderRow ? Number(headerRow) : 0
  const parts = mappingParts(current?.columns ?? {})
  const [columns, setColumns] = useState<Record<string, string>>(parts.columns)
  const [transforms, setTransforms] = useState(parts.transforms)
  const [transformError, setTransformError] = useState(false)
  const [columnPage, setColumnPage] = useState(0)
  const duplicateHeaders = new Set(
    (source.columns[sheet] ?? []).filter(
      (column, index, all) => all.indexOf(column) !== index
    )
  )
  const allColumns = Object.fromEntries(
    (source.columns[sheet] ?? []).map((column) => [
      column,
      columns[column] ?? "",
    ])
  )
  const [name, setName] = useState("")
  const template = useQuery({
    queryKey: [...owner.key, "template"],
    queryFn: ({ signal }) =>
      bulkRequest<BulkTemplate>(
        owner.workspace,
        "/template",
        "GET",
        undefined,
        signal
      ),
    retry: false,
  })
  const saved = useQuery({
    queryKey: [...owner.key, "mappings"],
    queryFn: ({ signal }) =>
      bulkRequest<{ mappings: SavedMapping[] }>(
        owner.workspace,
        "/mappings",
        "GET",
        undefined,
        signal
      ),
    retry: false,
  })
  const save = useMutation({
    mutationFn: () =>
      bulkRequest(owner.workspace, "/mappings", "POST", {
        name,
        mapping: {
          columns: mappingColumns(allColumns, transforms),
          sheet,
          header_row: effectiveHeaderRow,
        },
      }),
    onSuccess: () => {
      setName("")
      void client.invalidateQueries({ queryKey: [...owner.key, "mappings"] })
    },
  })
  return (
    <div
      id={`bulk-source-${source.id}`}
      tabIndex={-1}
      className="flex min-w-0 flex-col gap-3 rounded-xl border border-(--hair) p-4 focus-visible:outline-2 focus-visible:outline-ring"
    >
      <div className="flex flex-wrap justify-between gap-2">
        <h3 className="font-medium break-words">{source.name}</h3>
        <span className="text-sm text-muted-foreground">{source.state}</span>
      </div>
      {source.issues.length ? (
        <ul className="text-sm text-destructive">
          {source.issues.map((issue, index) => (
            <li key={index}>{displayValue(issue)}</li>
          ))}
        </ul>
      ) : null}
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          let prepared: MappingColumns
          try {
            prepared = mappingColumns(allColumns, transforms)
            setTransformError(false)
          } catch {
            setTransformError(true)
            return
          }
          submit({
            revision: run.revision,
            source_id: source.id,
            sheet: sheet || undefined,
            header_row: effectiveHeaderRow,
            columns: prepared,
          })
        }}
      >
        <div className="grid gap-3 sm:grid-cols-3">
          <label className={fieldClass}>
            {t("sheet")}
            <select
              className={selectClass}
              value={sheet}
              disabled={disabled}
              onChange={(event) => {
                setSheet(event.target.value)
                const mapping = run.mapping[
                  `${source.id}:${event.target.value}`
                ] as { columns?: MappingColumns } | undefined
                const parts = mappingParts(mapping?.columns ?? {})
                setColumns(parts.columns)
                setTransforms(parts.transforms)
                setHeaderRow(
                  String(
                    (mapping as { header_row?: number } | undefined)
                      ?.header_row ??
                      source.header_rows?.[event.target.value] ??
                      0
                  )
                )
                setColumnPage(0)
              }}
            >
              {Object.keys(source.columns).map((key) => (
                <option key={key} value={key}>
                  {key || t("document")}
                </option>
              ))}
            </select>
          </label>
          {hasHeaderRow ? (
            <label className={fieldClass}>
              {t("headerRow")}
              <Input
                type="number"
                min={0}
                step={1}
                value={headerRow}
                className="min-h-11"
                disabled={disabled}
                onChange={(event) => setHeaderRow(event.target.value)}
                required
              />
            </label>
          ) : null}
          <label className={fieldClass}>
            {t("savedMappings")}
            <select
              className={selectClass}
              value=""
              disabled={disabled}
              onChange={(event) => {
                const selected = saved.data?.mappings.find(
                  (item) => item.id === event.target.value
                )
                if (selected) {
                  const parts = mappingParts(
                    (selected.mapping.columns ?? {}) as MappingColumns
                  )
                  setColumns(parts.columns)
                  setTransforms(parts.transforms)
                  if (typeof selected.mapping.header_row === "number")
                    setHeaderRow(String(selected.mapping.header_row))
                }
              }}
            >
              <option value="">{t("chooseMapping")}</option>
              {saved.data?.mappings.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        {duplicateHeaders.size ? (
          <p role="alert" className="text-sm text-destructive">
            {t("duplicateHeaders")}
          </p>
        ) : null}
        <div className="grid gap-3 sm:grid-cols-2">
          {source.columns[sheet]
            ?.slice(columnPage * 50, (columnPage + 1) * 50)
            .map((column, index) => (
              <label
                key={`${columnPage * 50 + index}:${column}`}
                className={fieldClass}
              >
                <span className="break-words">{column}</span>
                <select
                  className={selectClass}
                  disabled={disabled || duplicateHeaders.has(column)}
                  value={columns[column] ?? ""}
                  onChange={(event) =>
                    setColumns({ ...columns, [column]: event.target.value })
                  }
                >
                  <option value="">{t("ignoreColumn")}</option>
                  {["_operation", "_key", "_product_id", "_clear"].map(
                    (key) => (
                      <option value={key} key={key}>
                        {t(`reserved.${key}`)}
                      </option>
                    )
                  )}
                  {(template.data ? templateFields(template.data) : [])
                    .filter((field) => !field.key.startsWith("_"))
                    .map((field) => (
                      <option key={field.key} value={field.key}>
                        {field.label || field.key} ({field.type || "text"})
                      </option>
                    ))}
                </select>
              </label>
            ))}
        </div>
        {(source.columns[sheet]?.length ?? 0) > 50 ? (
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={columnPage === 0}
              onClick={() => setColumnPage(columnPage - 1)}
            >
              {t("previous")}
            </Button>
            <Button
              type="button"
              variant="outline"
              className="min-h-11"
              disabled={
                (columnPage + 1) * 50 >= (source.columns[sheet]?.length ?? 0)
              }
              onClick={() => setColumnPage(columnPage + 1)}
            >
              {t("next")}
            </Button>
          </div>
        ) : null}
        <details>
          <summary className="min-h-11 cursor-pointer py-3 text-sm">
            {t("transforms.title")}
          </summary>
          <label className={fieldClass}>
            {t("transforms.label")}
            <Textarea
              className="font-mono"
              rows={6}
              maxLength={16000}
              value={transforms}
              disabled={disabled}
              onChange={(event) => setTransforms(event.target.value)}
            />
          </label>
          <p className="mt-2 text-sm text-muted-foreground">
            {t("transforms.help")}
          </p>
          <pre className="mt-2 overflow-x-auto rounded-lg bg-muted p-3 text-xs">
            {
              '{"Price":{"kind":"decimal","decimal_separator":",","group_separator":"."},"Date":{"kind":"date","format":"%d/%m/%Y"}}'
            }
          </pre>
        </details>
        {transformError ? (
          <p role="alert" className="text-sm text-destructive">
            {t("transforms.invalid")}
          </p>
        ) : null}
        <Button
          className="min-h-11 self-start"
          type="submit"
          disabled={
            disabled ||
            !Object.values(columns).some(Boolean) ||
            !Number.isSafeInteger(effectiveHeaderRow) ||
            effectiveHeaderRow < 0
          }
        >
          {t("applyMapping")}
        </Button>
      </form>
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <label className={fieldClass}>
          {t("mappingName")}
          <Input
            value={name}
            maxLength={255}
            onChange={(event) => setName(event.target.value)}
            className="min-h-11"
            required
            disabled={disabled}
          />
        </label>
        <Button
          className="min-h-11"
          variant="outline"
          type="submit"
          disabled={
            disabled ||
            save.isPending ||
            !name.trim() ||
            !Object.values(columns).some(Boolean)
          }
        >
          {t("saveMapping")}
        </Button>
      </form>
      <BulkError error={template.error || saved.error || save.error} />
      {save.isSuccess ? <p role="status">{t("saved")}</p> : null}
    </div>
  )
}
