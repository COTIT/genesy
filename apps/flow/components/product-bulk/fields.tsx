"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { apiRequest } from "@genesy/api-client/mutator"
import { Button } from "@genesy/ui/components/button"
import { Input } from "@genesy/ui/components/input"
import { ConfirmDialog } from "@genesy/ui/components/confirm-dialog"
import { fieldRequest, type Json, type ProductField } from "@/lib/product-bulk"
import { useBulkOwner } from "@/lib/use-product-bulk"
import { BulkError, BulkNav, fieldClass, panel, selectClass } from "./shared"
import { ValueInput } from "./value-input"

const TYPES: ProductField["type"][] = [
  "text",
  "number",
  "boolean",
  "date",
  "select",
  "multiselect",
]
export function ProductFields() {
  const owner = useBulkOwner()
  return <Fields key={`${owner.workspace}:${owner.user}`} owner={owner} />
}
function Fields({ owner }: { owner: ReturnType<typeof useBulkOwner> }) {
  const t = useTranslations("productBulk")
  const client = useQueryClient()
  const [creating, setCreating] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const query = useQuery({
    queryKey: [...owner.key, "fields"],
    queryFn: ({ signal }) =>
      fieldRequest<ProductField[]>(
        owner.workspace,
        "",
        "GET",
        undefined,
        signal
      ),
    enabled: owner.ready,
    retry: false,
  })
  const fields = query.data ?? []
  const field = fields.find((field) => field.id === selected)
  const admin = owner.rights.may("workspace_admin")
  return (
    <section
      className="mx-auto flex w-full max-w-5xl flex-col gap-5 pb-8"
      data-product-fields
    >
      <BulkNav />
      <header className="flex flex-wrap justify-between gap-3">
        <h1 className="text-2xl font-semibold">{t("fields.title")}</h1>
        {admin ? (
          <Button
            className="min-h-11"
            onClick={() => {
              setSelected(null)
              setCreating(true)
            }}
          >
            {t("fields.new")}
          </Button>
        ) : null}
      </header>
      <p className="max-w-[65ch] text-sm text-muted-foreground">
        {t("fields.help")}
      </p>
      <BulkError error={query.error} retry={() => void query.refetch()} />
      {query.isPending ? <p role="status">{t("loading")}</p> : null}
      <ul className="grid gap-2 sm:grid-cols-2">
        {fields.map((field) => (
          <li key={field.id}>
            <button
              type="button"
              className="flex min-h-11 w-full flex-col gap-1 rounded-xl border border-(--hair) bg-card p-4 text-left focus-visible:outline-2 focus-visible:outline-ring aria-pressed:border-(--gold)"
              aria-pressed={selected === field.id}
              onClick={() => {
                setSelected(field.id)
                setCreating(false)
              }}
            >
              <span className="font-medium break-words">{field.label}</span>
              <span className="text-sm text-muted-foreground">
                {t(`fields.types.${field.type}`)} ·{" "}
                {t(`fields.statuses.${field.status}`)}
                {field.required ? ` · ${t("fields.required")}` : ""}
              </span>
            </button>
          </li>
        ))}
      </ul>
      {admin && (creating || field) ? (
        <FieldEditor
          key={field ? `${field.id}:${field.revision}` : "new"}
          field={field}
          workspace={owner.workspace}
          onSaved={() => {
            setCreating(false)
            void client.invalidateQueries({ queryKey: owner.key })
          }}
        />
      ) : null}
      <ProductValues owner={owner} fields={fields} />
    </section>
  )
}
function FieldEditor({
  field,
  workspace,
  onSaved,
}: {
  field?: ProductField
  workspace: string
  onSaved: () => void
}) {
  const t = useTranslations("productBulk")
  const [key, setKey] = useState(field?.key ?? "")
  const [label, setLabel] = useState(field?.label ?? "")
  const [type, setType] = useState<ProductField["type"]>(field?.type ?? "text")
  const [status, setStatus] = useState<ProductField["status"]>(
    field?.status ?? "draft"
  )
  const [required, setRequired] = useState(field?.required ?? false)
  const [nullable, setNullable] = useState(field?.nullable ?? true)
  const [maxLength, setMaxLength] = useState(String(field?.max_length ?? 16384))
  const [min, setMin] = useState(field?.min ?? "")
  const [max, setMax] = useState(field?.max ?? "")
  const [options, setOptions] = useState(field?.options ?? [])
  const save = useMutation({
    mutationFn: () =>
      fieldRequest(
        workspace,
        field ? `/${field.id}` : "",
        field ? "PATCH" : "POST",
        {
          ...(field ? {} : { key }),
          label,
          type,
          status,
          required,
          nullable,
          max_length: Number(maxLength),
          min: min || null,
          max: max || null,
          options,
        }
      ),
    onSuccess: onSaved,
  })
  const archive = useMutation({
    mutationFn: () => fieldRequest(workspace, `/${field?.id}`, "DELETE"),
    onSuccess: onSaved,
  })
  const busy = save.isPending || archive.isPending
  return (
    <section className={panel}>
      <h2 className="text-lg font-semibold">
        {t(field ? "fields.edit" : "fields.new")}
      </h2>
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <label className={fieldClass}>
            {t("fields.key")}
            <Input
              className="min-h-11"
              value={key}
              onChange={(event) => setKey(event.target.value)}
              required
              disabled={Boolean(field)}
              maxLength={100}
            />
          </label>
          <label className={fieldClass}>
            {t("fields.label")}
            <Input
              className="min-h-11"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              required
              maxLength={255}
            />
          </label>
          <label className={fieldClass}>
            {t("fields.type")}
            <select
              className={selectClass}
              value={type}
              disabled={field?.used}
              onChange={(event) =>
                setType(event.target.value as ProductField["type"])
              }
            >
              {TYPES.map((type) => (
                <option value={type} key={type}>
                  {t(`fields.types.${type}`)}
                </option>
              ))}
            </select>
          </label>
          <label className={fieldClass}>
            {t("status")}
            <select
              className={selectClass}
              value={status}
              disabled={field?.status === "archived"}
              onChange={(event) =>
                setStatus(event.target.value as ProductField["status"])
              }
            >
              {["draft", "active"].map((value) => (
                <option key={value} value={value}>
                  {t(`fields.statuses.${value}`)}
                </option>
              ))}
              {field?.status === "archived" ? (
                <option value="archived">
                  {t("fields.statuses.archived")}
                </option>
              ) : null}
            </select>
          </label>
        </div>
        <div className="flex flex-wrap gap-4">
          <label className="flex min-h-11 items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="size-5"
              checked={required}
              onChange={(event) => {
                setRequired(event.target.checked)
                if (event.target.checked) setNullable(false)
              }}
            />
            {t("fields.required")}
          </label>
          <label className="flex min-h-11 items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="size-5"
              checked={nullable}
              disabled={required}
              onChange={(event) => setNullable(event.target.checked)}
            />
            {t("fields.nullable")}
          </label>
        </div>
        <p className="text-sm text-muted-foreground">
          {t("fields.requiredHelp")}
        </p>
        {type === "text" ? (
          <label className={fieldClass}>
            {t("fields.maxLength")}
            <Input
              className="min-h-11"
              type="number"
              min={1}
              max={16384}
              step={1}
              value={maxLength}
              onChange={(event) => setMaxLength(event.target.value)}
              required
            />
          </label>
        ) : null}
        {type === "number" ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <label className={fieldClass}>
              {t("fields.min")}
              <Input
                className="min-h-11"
                inputMode="decimal"
                value={min}
                onChange={(event) => setMin(event.target.value)}
              />
            </label>
            <label className={fieldClass}>
              {t("fields.max")}
              <Input
                className="min-h-11"
                inputMode="decimal"
                value={max}
                onChange={(event) => setMax(event.target.value)}
              />
            </label>
          </div>
        ) : null}
        {type === "select" || type === "multiselect" ? (
          <fieldset className="flex flex-col gap-3">
            <legend className="mb-2 text-sm font-medium">
              {t("fields.options")}
            </legend>
            {options.map((option, index) => (
              <div key={option.id} className="flex flex-wrap gap-2">
                <Input
                  aria-label={t("fields.optionLabel", { index: index + 1 })}
                  className="min-h-11 min-w-0 flex-1"
                  value={option.label}
                  onChange={(event) =>
                    setOptions(
                      options.map((item) =>
                        item.id === option.id
                          ? { ...item, label: event.target.value }
                          : item
                      )
                    )
                  }
                  required
                />
                <Button
                  className="min-h-11"
                  variant="outline"
                  type="button"
                  onClick={() =>
                    setOptions(options.filter((item) => item.id !== option.id))
                  }
                >
                  {t("remove")}
                </Button>
              </div>
            ))}
            <Button
              type="button"
              className="min-h-11 self-start"
              variant="outline"
              onClick={() =>
                setOptions([...options, { id: crypto.randomUUID(), label: "" }])
              }
            >
              {t("fields.addOption")}
            </Button>
          </fieldset>
        ) : null}
        <BulkError error={save.error || archive.error} />
        <div className="flex flex-wrap gap-3">
          <Button
            type="submit"
            className="min-h-11"
            disabled={busy || field?.status === "archived"}
          >
            {t("save")}
          </Button>
          {field && field.status !== "archived" ? (
            <ConfirmDialog
              trigger={t("fields.archive")}
              title={t("fields.archive")}
              description={t("fields.archiveHelp")}
              confirm={t("fields.archive")}
              cancel={t("backToReview")}
              onConfirm={() => archive.mutate()}
              disabled={busy}
            />
          ) : null}
        </div>
      </form>
    </section>
  )
}

type Product = {
  id: string
  name: string
  custom_fields?: Record<string, Json>
}
function ProductValues({
  owner,
  fields,
}: {
  owner: ReturnType<typeof useBulkOwner>
  fields: ProductField[]
}) {
  const t = useTranslations("productBulk")
  const [search, setSearch] = useState("")
  const [q, setQ] = useState("")
  const [page, setPage] = useState(1)
  const [fieldId, setFieldId] = useState("")
  const [operator, setOperator] = useState("eq")
  const [value, setValue] = useState<Json>("")
  const [filter, setFilter] = useState<unknown[]>([])
  const [selected, setSelected] = useState<Product | null>(null)
  const field = fields.find((item) => item.id === fieldId)
  const query = useQuery({
    queryKey: [...owner.key, "products", q, page, filter],
    queryFn: ({ signal }) =>
      apiRequest<{
        data: Product[]
        meta: { total: number; last_page: number }
      }>({
        url: `/workspace/${encodeURIComponent(owner.workspace)}/products`,
        params: {
          q,
          page,
          page_size: 30,
          custom_filters: JSON.stringify(filter),
        },
        signal,
      }),
    enabled: owner.ready,
    retry: false,
  })
  const operators = !field
    ? []
    : [
        "eq",
        "is_set",
        ...(["number", "date"].includes(field.type)
          ? ["lt", "lte", "gt", "gte"]
          : []),
        ...(["text", "multiselect"].includes(field.type) ? ["contains"] : []),
        ...(field.type === "multiselect" ? ["any"] : []),
      ].filter((item) => !(field.type === "multiselect" && item === "eq"))
  const filterField = field
    ? {
        ...field,
        type:
          operator === "is_set"
            ? ("boolean" as const)
            : field.type === "multiselect" && operator === "contains"
              ? ("select" as const)
              : field.type,
      }
    : null
  return (
    <section className={panel}>
      <h2 className="text-lg font-semibold">{t("fields.productValues")}</h2>
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault()
          setQ(search.trim())
          setPage(1)
          setFilter(fieldId ? [{ field_id: fieldId, operator, value }] : [])
        }}
      >
        <label className={fieldClass}>
          {t("fields.searchProducts")}
          <Input
            className="min-h-11"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
        <div className="grid gap-3 sm:grid-cols-3">
          <label className={fieldClass}>
            {t("field")}
            <select
              className={selectClass}
              value={fieldId}
              onChange={(event) => {
                setFieldId(event.target.value)
                setOperator("is_set")
                setValue(true)
              }}
            >
              <option value="">{t("fields.noFilter")}</option>
              {fields
                .filter((field) => field.status === "active")
                .map((field) => (
                  <option key={field.id} value={field.id}>
                    {field.label}
                  </option>
                ))}
            </select>
          </label>
          {field ? (
            <>
              <label className={fieldClass}>
                {t("fields.operator")}
                <select
                  className={selectClass}
                  value={operator}
                  onChange={(event) => {
                    setOperator(event.target.value)
                    setValue(
                      event.target.value === "is_set"
                        ? true
                        : field.type === "multiselect" &&
                            event.target.value === "any"
                          ? []
                          : ""
                    )
                  }}
                >
                  {operators.map((operator) => (
                    <option key={operator} value={operator}>
                      {t(`fields.operators.${operator}`)}
                    </option>
                  ))}
                </select>
              </label>
              <label className={fieldClass}>
                {t("fields.value")}
                {filterField ? (
                  <ValueInput
                    field={filterField}
                    value={value}
                    label={t("fields.value")}
                    onChange={setValue}
                  />
                ) : null}
              </label>
            </>
          ) : null}
        </div>
        <Button type="submit" className="min-h-11 self-start">
          {t("search")}
        </Button>
      </form>
      <BulkError error={query.error} retry={() => void query.refetch()} />
      {query.data ? (
        <p className="text-sm tabular-nums">
          {t("fields.productCount", { count: query.data.meta.total })}
        </p>
      ) : null}
      <ul className="grid gap-2">
        {query.data?.data.map((product) => (
          <li key={product.id}>
            <Button
              className="h-auto min-h-11 w-full justify-start text-left whitespace-normal"
              variant="outline"
              onClick={() => setSelected(product)}
            >
              {product.name}
            </Button>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-3">
        <Button
          className="min-h-11"
          variant="outline"
          disabled={page === 1 || query.isFetching}
          onClick={() => setPage(page - 1)}
        >
          {t("previous")}
        </Button>
        <Button
          className="min-h-11"
          variant="outline"
          disabled={
            !query.data || page >= query.data.meta.last_page || query.isFetching
          }
          onClick={() => setPage(page + 1)}
        >
          {t("next")}
        </Button>
      </div>
      {selected ? (
        <ProductValueEditor
          key={selected.id}
          product={selected}
          fields={fields}
          owner={owner}
        />
      ) : null}
    </section>
  )
}
function ProductValueEditor({
  product,
  fields,
  owner,
}: {
  product: Product
  fields: ProductField[]
  owner: ReturnType<typeof useBulkOwner>
}) {
  const t = useTranslations("productBulk")
  const client = useQueryClient()
  const [changes, setChanges] = useState<Record<string, Json>>({})
  const query = useQuery({
    queryKey: [...owner.key, "product", product.id],
    queryFn: ({ signal }) =>
      apiRequest<{ data: Product }>({
        url: `/workspace/${encodeURIComponent(owner.workspace)}/products/${product.id}`,
        signal,
      }),
    retry: false,
  })
  const save = useMutation({
    mutationFn: () =>
      apiRequest({
        url: `/workspace/${encodeURIComponent(owner.workspace)}/products/${product.id}`,
        method: "POST",
        data: { custom_fields: changes },
      }),
    onSuccess: () => {
      setChanges({})
      void client.invalidateQueries({ queryKey: owner.key })
    },
  })
  return (
    <form
      className="flex flex-col gap-4 border-t border-(--hair) pt-4"
      onSubmit={(event) => {
        event.preventDefault()
        save.mutate()
      }}
    >
      <h3 className="font-semibold">{product.name}</h3>
      <div className="grid gap-3 sm:grid-cols-2">
        {fields
          .filter((field) => field.status !== "draft")
          .map((field) => (
            <label className={fieldClass} key={field.id}>
              {field.label}
              {field.required ? " *" : ""}
              <ValueInput
                field={field}
                value={
                  field.id in changes
                    ? changes[field.id]
                    : query.data?.data.custom_fields?.[field.id]
                }
                label={field.label}
                disabled={
                  field.status === "archived" ||
                  !owner.rights.may("product.update") ||
                  query.isPending ||
                  save.isPending
                }
                onChange={(value) =>
                  setChanges({ ...changes, [field.id]: value })
                }
              />
              {field.nullable &&
              !field.required &&
              field.status === "active" ? (
                <Button
                  className="min-h-11 self-start"
                  type="button"
                  variant="ghost"
                  disabled={
                    !owner.rights.may("product.update") || save.isPending
                  }
                  onClick={() => setChanges({ ...changes, [field.id]: null })}
                >
                  {t("clearValue")}
                </Button>
              ) : null}
            </label>
          ))}
      </div>
      <BulkError error={query.error || save.error} />
      {save.isSuccess ? <p role="status">{t("saved")}</p> : null}
      <Button
        className="min-h-11 self-start"
        type="submit"
        disabled={
          save.isPending ||
          !Object.keys(changes).length ||
          !owner.rights.may("product.update")
        }
      >
        {t("save")}
      </Button>
    </form>
  )
}
