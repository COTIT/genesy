"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { useQuery } from "@tanstack/react-query"
import { Input } from "@genesy/ui/components/input"
import { Textarea } from "@genesy/ui/components/textarea"
import { Button } from "@genesy/ui/components/button"
import {
  bulkRequest,
  type BulkTemplate,
  type BulkRun,
  type Json,
} from "@/lib/product-bulk"
import { useBulkOwner } from "@/lib/use-product-bulk"
import { BulkError, fieldClass, panel, selectClass } from "./shared"
import { ValueInput } from "./value-input"

/** Selection is explicit; matching IDs and decimal calculations stay on the server. */
export function ScopeRequest({
  run,
  owner,
  disabled,
  submit,
}: {
  run: BulkRun
  owner: ReturnType<typeof useBulkOwner>
  disabled: boolean
  submit: (body: unknown) => void
}) {
  const t = useTranslations("productBulk")
  const [ids, setIds] = useState("")
  const [q, setQ] = useState("")
  const [status, setStatus] = useState("")
  const [operation, setOperation] = useState("update")
  const [field, setField] = useState("price")
  const [kind, setKind] = useState("set")
  const [value, setValue] = useState<Json>("")
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
  const explicit = Boolean(ids.trim() || q.trim() || status)
  const type = template.data?.fields.find((item) => item.key === field)?.type
  const valueType =
    type === "boolean"
      ? "boolean"
      : type === "date"
        ? "date"
        : ["number", "decimal", "integer"].includes(type ?? "")
          ? "number"
          : "text"
  return (
    <details className={panel}>
      <summary className="min-h-11 cursor-pointer py-3 font-semibold">
        {t("scope.title")}
      </summary>
      <p className="max-w-[65ch] text-sm text-muted-foreground">
        {t("scope.help")}
      </p>
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault()
          if (!explicit) return
          submit({
            revision: run.revision,
            selection: {
              ...(ids.trim()
                ? { ids: ids.split(/[\s,]+/).filter(Boolean) }
                : {}),
              ...(q.trim() ? { q: q.trim() } : {}),
              ...(status ? { status } : {}),
            },
            operation,
            ...(operation === "update"
              ? { changes: { [field]: { kind, value } } }
              : {}),
          })
        }}
      >
        <label className={fieldClass}>
          {t("scope.ids")}
          <Textarea
            rows={2}
            maxLength={2000000}
            value={ids}
            disabled={disabled}
            onChange={(event) => setIds(event.target.value)}
          />
        </label>
        <div className="grid gap-3 sm:grid-cols-2">
          <label className={fieldClass}>
            {t("scope.query")}
            <Input
              className="min-h-11"
              value={q}
              disabled={disabled}
              onChange={(event) => setQ(event.target.value)}
            />
          </label>
          <label className={fieldClass}>
            {t("status")}
            <select
              className={selectClass}
              value={status}
              disabled={disabled}
              onChange={(event) => setStatus(event.target.value)}
            >
              <option value="">{t("scope.noStatus")}</option>
              {["active", "disabled", "nla"].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
        </div>
        <label className={fieldClass}>
          {t("operation")}
          <select
            className={selectClass}
            value={operation}
            disabled={disabled}
            onChange={(event) => setOperation(event.target.value)}
          >
            <option value="update">{t("operations.update")}</option>
            <option value="delete">{t("operations.delete")}</option>
          </select>
        </label>
        {operation === "update" ? (
          <div className="grid gap-3 sm:grid-cols-3">
            <label className={fieldClass}>
              {t("field")}
              <select
                className={selectClass}
                value={field}
                disabled={disabled}
                onChange={(event) => setField(event.target.value)}
              >
                {template.data?.fields.map((field) => (
                  <option key={field.key} value={field.key}>
                    {field.label || field.key}
                  </option>
                ))}
              </select>
            </label>
            <label className={fieldClass}>
              {t("scope.change")}
              <select
                className={selectClass}
                value={kind}
                disabled={disabled}
                onChange={(event) => setKind(event.target.value)}
              >
                <option value="set">{t("scope.set")}</option>
                <option value="percent">{t("scope.percent")}</option>
              </select>
            </label>
            <label className={fieldClass}>
              {t("fields.value")}
              {kind === "percent" ? (
                <Input
                  className="min-h-11"
                  inputMode="decimal"
                  value={value == null ? "" : String(value)}
                  disabled={disabled}
                  onChange={(event) => setValue(event.target.value)}
                  required
                />
              ) : (
                <ValueInput
                  field={{ type: valueType, max_length: 16384, options: [] }}
                  value={value}
                  disabled={disabled}
                  onChange={setValue}
                  label={t("fields.value")}
                />
              )}
            </label>
          </div>
        ) : null}
        <Button
          type="submit"
          className="min-h-11 self-start"
          disabled={
            disabled ||
            !explicit ||
            (operation === "update" && (!field || value === ""))
          }
        >
          {t("scope.prepare")}
        </Button>
      </form>
      <BulkError error={template.error} />
    </details>
  )
}
