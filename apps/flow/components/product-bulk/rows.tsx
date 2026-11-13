"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { useQuery } from "@tanstack/react-query"
import { Button } from "@genesy/ui/components/button"
import { Input } from "@genesy/ui/components/input"
import {
  bulkPollInterval,
  bulkRequest,
  canEditBulk,
  displayValue,
  fieldRequest,
  rowLocation,
  type BulkRow,
  type BulkRun,
  type Json,
  type ProductField,
  type RowsPage,
  type BulkTemplate,
  type TemplateField,
} from "@/lib/product-bulk"
import { useBulkOwner, usePageVisible } from "@/lib/use-product-bulk"
import { BulkError, fieldClass, panel, selectClass } from "./shared"
import { ValueInput } from "./value-input"
import { RawCells } from "./raw-cells"

export function BulkRows({
  run,
  owner,
  busy,
  onEdit,
}: {
  run: BulkRun
  owner: ReturnType<typeof useBulkOwner>
  busy: boolean
  onEdit: (id: string, values: Record<string, unknown>) => void
}) {
  const t = useTranslations("productBulk")
  const visible = usePageVisible()
  const [q, setQ] = useState("")
  const [search, setSearch] = useState("")
  const [status, setStatus] = useState("")
  const [cursors, setCursors] = useState<(number | string | null)[]>([null])
  const after = cursors[cursors.length - 1]
  const query = useQuery({
    queryKey: [
      ...owner.key,
      "rows",
      run.id,
      run.revision,
      run.state,
      after,
      status,
      q,
    ],
    queryFn: ({ signal }) => {
      const params = new URLSearchParams({ page_size: "100" })
      if (after != null) params.set("after", String(after))
      if (status) params.set("status", status)
      if (q) params.set("q", q)
      return bulkRequest<RowsPage>(
        owner.workspace,
        `/${run.id}/rows?${params}`,
        "GET",
        undefined,
        signal
      )
    },
    enabled: visible,
    refetchInterval: () => bulkPollInterval([run.state], visible),
    retry: false,
  })
  const fields = useQuery({
    queryKey: [...owner.key, "fields"],
    queryFn: ({ signal }) =>
      fieldRequest<ProductField[]>(
        owner.workspace,
        "",
        "GET",
        undefined,
        signal
      ),
    retry: false,
  })
  const editable = canEditBulk(run) && !busy
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
  return (
    <section className={panel} aria-labelledby="bulk-rows-title">
      <h2 id="bulk-rows-title" tabIndex={-1} className="text-lg font-semibold">
        {t("rowsTitle")}
      </h2>
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault()
          setQ(search.trim())
          setCursors([null])
        }}
      >
        <label className={`${fieldClass} flex-1`}>
          {t("searchRows")}
          <Input
            className="min-h-11"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
        <label className={fieldClass}>
          {t("status")}
          <select
            className={selectClass}
            value={status}
            onChange={(event) => {
              setStatus(event.target.value)
              setCursors([null])
            }}
          >
            <option value="">{t("allRows")}</option>
            {["ready", "error", "applied", "excluded", "stale"].map((value) => (
              <option key={value} value={value}>
                {t(`rowStates.${value}`)}
              </option>
            ))}
          </select>
        </label>
        <Button type="submit" className="min-h-11">
          {t("search")}
        </Button>
      </form>
      <BulkError error={query.error} retry={() => void query.refetch()} />
      {query.isPending ? <p role="status">{t("loading")}</p> : null}
      {query.data ? (
        <p className="text-sm tabular-nums">
          {t("rowsShown", {
            count: query.data.rows.length,
            total: query.data.total ?? run.counts.total ?? 0,
          })}
        </p>
      ) : null}
      <ol className="flex flex-col gap-3">
        {query.data?.rows.map((row) => (
          <li key={row.id}>
            <BulkRowCard
              key={`${row.id}:${run.revision}`}
              row={row}
              editable={editable}
              fields={fields.data ?? []}
              nativeFields={template.data?.fields ?? []}
              raw={<RawCells row={row} runId={run.id} owner={owner} />}
              onEdit={(body) => onEdit(row.id, body)}
            />
          </li>
        ))}
      </ol>
      {query.data?.rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("noRows")}</p>
      ) : null}
      <nav
        className="flex flex-wrap items-center gap-3"
        aria-label={t("rowPages")}
      >
        <Button
          className="min-h-11"
          variant="outline"
          disabled={cursors.length === 1 || query.isFetching}
          onClick={() => setCursors(cursors.slice(0, -1))}
        >
          {t("previous")}
        </Button>
        <span className="text-sm tabular-nums">
          {t("page", { page: cursors.length })}
        </span>
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
    </section>
  )
}

export function BulkRowCard({
  row,
  editable,
  fields,
  nativeFields,
  raw,
  onEdit,
}: {
  row: BulkRow
  editable: boolean
  fields: ProductField[]
  nativeFields: TemplateField[]
  raw: React.ReactNode
  onEdit: (body: Record<string, unknown>) => void
}) {
  const t = useTranslations("productBulk")
  const [values, setValues] = useState(row.values)
  const [operation, setOperation] = useState(row.operation)
  const target =
    row.target && typeof row.target === "object" && !Array.isArray(row.target)
      ? row.target
      : {}
  const [key, setKey] = useState(String(target.product_id ?? target.key ?? ""))
  const [addField, setAddField] = useState("")
  const [addNative, setAddNative] = useState("")
  const before =
    row.before?.product &&
    typeof row.before.product === "object" &&
    !Array.isArray(row.before.product)
      ? (row.before.product as Record<string, Json>)
      : (row.before ?? {})
  const custom =
    values.custom_fields &&
    typeof values.custom_fields === "object" &&
    !Array.isArray(values.custom_fields)
      ? values.custom_fields
      : {}
  const after = row.after === undefined ? row.values : (row.after ?? {})
  const compared = Object.keys({ ...before, ...after })
  const title = after.name ?? before.name ?? values.name
  const name = typeof title === "string" ? title : `#${row.ordinal}`
  return (
    <article
      id={`bulk-row-${row.id}`}
      tabIndex={-1}
      className="flex min-w-0 flex-col gap-3 rounded-xl border border-(--hair) p-4 focus-visible:outline-2 focus-visible:outline-ring"
      data-bulk-row={row.id}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="min-w-0 font-semibold break-words">{name}</h3>
        <span className="text-sm">
          {t.has(`rowStates.${row.status}`)
            ? t(`rowStates.${row.status}`)
            : row.status}{" "}
          · {t(`operations.${row.operation}`)}
        </span>
      </div>
      <p className="text-xs break-all text-muted-foreground">
        {rowLocation(row)}
      </p>
      {raw}
      {row.issues.length ? (
        <ul className="flex flex-col gap-1 text-sm text-destructive">
          {row.issues.map((issue, index) => (
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
      <div
        className="overflow-x-auto focus-visible:outline-2 focus-visible:outline-ring"
        role="region"
        tabIndex={0}
        aria-label={t("comparison", { name })}
      >
        <table className="w-full text-left text-sm">
          <caption className="sr-only">{t("comparison", { name })}</caption>
          <thead>
            <tr className="border-b border-(--hair)">
              <th scope="col" className="p-2">
                {t("field")}
              </th>
              <th scope="col" className="p-2">
                {t("before")}
              </th>
              <th scope="col" className="p-2">
                {t("after")}
              </th>
            </tr>
          </thead>
          <tbody>
            {compared.map((field) => (
              <tr
                key={field}
                className="border-b border-(--hair) last:border-0"
              >
                <th
                  scope="row"
                  className="max-w-40 p-2 font-medium break-words"
                >
                  {field}
                </th>
                <td className="max-w-72 p-2 break-words whitespace-pre-wrap">
                  {displayValue(before[field])}
                </td>
                <td className="max-w-72 p-2 break-words whitespace-pre-wrap">
                  {row.operation === "delete"
                    ? "—"
                    : displayValue(after[field])}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {row.receipt ? (
        <details>
          <summary className="min-h-11 cursor-pointer py-3 text-sm">
            {t("receipt")}
          </summary>
          <pre className="max-h-72 overflow-auto rounded-lg bg-muted p-3 text-xs break-all whitespace-pre-wrap">
            {JSON.stringify(row.receipt, null, 2)}
          </pre>
        </details>
      ) : null}
      {editable ? (
        <details>
          <summary className="min-h-11 cursor-pointer py-3 text-sm font-medium">
            {t("correctRow")}
          </summary>
          <form
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault()
              onEdit({
                operation,
                values,
                target: key
                  ? {
                      ...target,
                      [target.product_id ? "product_id" : "key"]: key,
                    }
                  : target,
              })
            }}
          >
            <label className="flex min-h-11 items-center gap-3 text-sm">
              <input
                type="checkbox"
                checked={row.included}
                className="size-5"
                onChange={(event) => onEdit({ included: event.target.checked })}
              />
              {t("includeRow")}
            </label>
            <div className="grid gap-3 sm:grid-cols-2">
              <label className={fieldClass}>
                {t("operation")}
                <select
                  className={selectClass}
                  value={operation}
                  onChange={(event) =>
                    setOperation(event.target.value as BulkRow["operation"])
                  }
                >
                  {["create", "update", "delete"].map((value) => (
                    <option key={value} value={value}>
                      {t(`operations.${value}`)}
                    </option>
                  ))}
                </select>
              </label>
              <label className={fieldClass}>
                {t("target")}
                <Input
                  className="min-h-11"
                  value={key}
                  onChange={(event) => setKey(event.target.value)}
                />
              </label>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              {Object.entries(values)
                .filter(([key]) => key !== "custom_fields")
                .map(([key, value]) => (
                  <label key={key} className={fieldClass}>
                    {key}
                    <ValueInput
                      field={{
                        type:
                          typeof value === "boolean" ||
                          nativeFields.find((field) => field.key === key)
                            ?.type === "boolean"
                            ? "boolean"
                            : "text",
                        max_length: 16384,
                        options: [],
                      }}
                      value={value}
                      label={key}
                      onChange={(value) =>
                        setValues({ ...values, [key]: value })
                      }
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      className="min-h-11 self-start"
                      onClick={() => setValues({ ...values, [key]: null })}
                    >
                      {t("clearValue")}
                    </Button>
                  </label>
                ))}
              {fields
                .filter((field) => field.id in custom)
                .map((field) => (
                  <label key={field.id} className={fieldClass}>
                    {field.label}
                    <ValueInput
                      field={field}
                      value={custom[field.id]}
                      label={field.label}
                      onChange={(value) =>
                        setValues({
                          ...values,
                          custom_fields: { ...custom, [field.id]: value },
                        })
                      }
                    />
                  </label>
                ))}
            </div>
            <div className="flex flex-wrap items-end gap-2">
              <label className={fieldClass}>
                {t("addNativeField")}
                <select
                  className={selectClass}
                  value={addNative}
                  onChange={(event) => setAddNative(event.target.value)}
                >
                  <option value="">—</option>
                  {nativeFields
                    .filter((field) => !(field.key in values))
                    .map((field) => (
                      <option key={field.key} value={field.key}>
                        {field.label || field.key}
                      </option>
                    ))}
                </select>
              </label>
              <Button
                type="button"
                className="min-h-11"
                variant="outline"
                disabled={!addNative}
                onClick={() => {
                  setValues({ ...values, [addNative]: "" })
                  setAddNative("")
                }}
              >
                {t("add")}
              </Button>
            </div>
            <div className="flex flex-wrap items-end gap-2">
              <label className={fieldClass}>
                {t("addCustomField")}
                <select
                  className={selectClass}
                  value={addField}
                  onChange={(event) => setAddField(event.target.value)}
                >
                  <option value="">—</option>
                  {fields
                    .filter(
                      (field) =>
                        field.status === "active" && !(field.id in custom)
                    )
                    .map((field) => (
                      <option key={field.id} value={field.id}>
                        {field.label}
                      </option>
                    ))}
                </select>
              </label>
              <Button
                type="button"
                className="min-h-11"
                variant="outline"
                disabled={!addField}
                onClick={() => {
                  setValues({
                    ...values,
                    custom_fields: { ...custom, [addField]: null },
                  })
                  setAddField("")
                }}
              >
                {t("add")}
              </Button>
            </div>
            <p className="text-sm text-muted-foreground">
              {t("editInvalidates")}
            </p>
            <Button className="min-h-11 self-start" type="submit">
              {t("saveCorrection")}
            </Button>
          </form>
        </details>
      ) : null}
    </article>
  )
}
