"use client"

import { Input } from "@genesy/ui/components/input"
import { Textarea } from "@genesy/ui/components/textarea"
import { displayValue, type Json, type ProductField } from "@/lib/product-bulk"
import { selectClass } from "./shared"

export function ValueInput({
  field,
  value,
  onChange,
  disabled,
  label,
}: {
  field: Pick<ProductField, "type" | "options" | "max_length">
  value: Json | undefined
  onChange: (value: Json) => void
  disabled?: boolean
  label: string
}) {
  if (field.type === "boolean")
    return (
      <select
        aria-label={label}
        className={selectClass}
        value={value == null ? "" : String(value)}
        disabled={disabled}
        onChange={(event) =>
          onChange(
            event.target.value === "" ? null : event.target.value === "true"
          )
        }
      >
        <option value="">—</option>
        <option value="true">true</option>
        <option value="false">false</option>
      </select>
    )
  if (field.type === "select" || field.type === "multiselect")
    return (
      <select
        aria-label={label}
        multiple={field.type === "multiselect"}
        className={selectClass}
        value={
          field.type === "multiselect"
            ? Array.isArray(value)
              ? value.map(String)
              : []
            : typeof value === "string"
              ? value
              : ""
        }
        disabled={disabled}
        onChange={(event) =>
          onChange(
            field.type === "multiselect"
              ? Array.from(event.target.selectedOptions).map(
                  (option) => option.value
                )
              : event.target.value || null
          )
        }
      >
        {field.type === "select" ? <option value="">—</option> : null}
        {field.options.map((option) => (
          <option key={option.id} value={option.id}>
            {option.label}
          </option>
        ))}
      </select>
    )
  if (field.type === "text")
    return (
      <Textarea
        aria-label={label}
        value={
          typeof value === "string"
            ? value
            : value == null
              ? ""
              : displayValue(value)
        }
        maxLength={field.max_length}
        rows={2}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      />
    )
  return (
    <Input
      aria-label={label}
      className="min-h-11"
      type={field.type === "date" ? "date" : "text"}
      inputMode={field.type === "number" ? "decimal" : undefined}
      value={value == null ? "" : String(value)}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value)}
    />
  )
}
