import { apiRequest } from "@genesy/api-client/mutator"

export type Json =
  null | boolean | number | string | Json[] | { [key: string]: Json }
export type ProductField = {
  id: string
  key: string
  label: string
  type: "text" | "number" | "boolean" | "date" | "select" | "multiselect"
  status: "draft" | "active" | "archived"
  required: boolean
  nullable: boolean
  max_length: number
  min: string | null
  max: string | null
  options: { id: string; label: string }[]
  used: boolean
  revision: number
}
export type BulkSource = {
  id: string
  media_type?: string
  name: string
  state: string
  size_bytes: number
  columns: Record<string, string[]>
  header_rows?: Record<string, number>
  issues: Json[]
  progress?: {
    pages_done?: number
    total_pages?: number
    rows_read?: number
    sheet_index?: number
  }
}
export type BulkReview = {
  id: string
  revision: number
  scope: "write" | "delete"
  state: string
  expires_at: string
  row_count: number
  fingerprint: string
}
export type BulkRun = {
  id: string
  name: string
  instruction?: string
  revision: number
  state: string
  counts: Record<string, number>
  progress: Record<string, Json>
  sources: BulkSource[]
  mapping: Record<string, Json>
  reviews: BulkReview[]
  spend: Record<string, Json>
  created_at?: string
  expires_at?: string
  reason?: string
  mode: string
  match_key: string
  permissions: {
    edit: boolean
    approve: boolean
    cancel: boolean
    resume?: boolean
  }
  issues?: Json[]
}
export type BulkRunSummary = Pick<
  BulkRun,
  "id" | "name" | "state" | "revision" | "created_at" | "reason" | "issues"
> &
  Partial<Pick<BulkRun, "counts" | "progress" | "spend" | "sources">>
export type RunsPage = { runs: BulkRunSummary[]; next?: number | string | null }
export type BulkRow = {
  id: string
  ordinal: number
  source?: string | null
  source_id?: string | null
  page?: number | null
  sheet?: string | null
  line?: number | null
  operation: "create" | "update" | "delete" | "upsert"
  target: Json
  resolved_target?: Json
  values: Record<string, Json>
  before?: Record<string, Json> | null
  after?: Record<string, Json> | null
  included: boolean
  issues: Json[]
  status: string
  receipt?: Json
  raw?: {
    source_id?: string | null
    sheet?: string | null
    page?: number | null
    line?: number | null
    cells: string[]
    columns: string[]
    formula_columns: number[]
  }
  raw_truncated?: boolean
}
export type RawCellsPage = {
  cells: { column: string; index: number; value: string; formula: boolean }[]
  next?: number | null
}
export type RowsPage = {
  rows: BulkRow[]
  after?: number | string | null
  next?: number | string | null
  has_more?: boolean
  total?: number
}
export type SavedMapping = {
  id: string
  name: string
  mapping: Record<string, Json>
}
export type TemplateField = {
  key: string
  label?: string
  type?: string
  required?: boolean
  example?: Json
  options?: { id: string; label: string }[]
}
export type BulkTemplate = {
  fields: TemplateField[]
  custom_fields?: ProductField[]
  schema?: Json
}
export function templateFields(template: BulkTemplate): TemplateField[] {
  return [
    ...["_operation", "_key", "_product_id", "_clear"]
      .filter((key) => !template.fields.some((field) => field.key === key))
      .map((key) => ({ key, type: "text" })),
    ...template.fields,
    ...(template.custom_fields ?? [])
      .filter((field) => field.status === "active")
      .map((field) => ({
        key: `custom_fields.${field.id}`,
        label: field.label,
        type: field.type,
        required: field.required,
        options: field.options,
      })),
  ]
}
export type MappingColumns = Record<
  string,
  string | { target: string; transform?: Json }
>
export function mappingParts(columns: MappingColumns) {
  return {
    columns: Object.fromEntries(
      Object.entries(columns).map(([key, value]) => [
        key,
        typeof value === "string" ? value : value.target,
      ])
    ),
    transforms: JSON.stringify(
      Object.fromEntries(
        Object.entries(columns).flatMap(([key, value]) =>
          typeof value !== "string" && value.transform
            ? [[key, value.transform]]
            : []
        )
      ),
      null,
      2
    ),
  }
}
export function mappingColumns(
  columns: Record<string, string>,
  text: string
): MappingColumns {
  const transforms: unknown = JSON.parse(text || "{}")
  if (
    !transforms ||
    typeof transforms !== "object" ||
    Array.isArray(transforms)
  )
    throw new Error("Invalid transforms")
  const entries = Object.entries(transforms)
  if (
    entries.some(
      ([column, value]) =>
        !columns[column] ||
        !value ||
        typeof value !== "object" ||
        Array.isArray(value) ||
        typeof value.kind !== "string"
    )
  )
    throw new Error("Invalid transforms")
  return Object.fromEntries(
    Object.entries(columns).map(([column, target]) => [
      column,
      column in transforms
        ? { target, transform: (transforms as Record<string, Json>)[column] }
        : target,
    ])
  )
}

export const bulkKey = (workspace: string, user: string) =>
  ["product-bulk", workspace, user] as const
export const bulkPath = (workspace: string, suffix = "") =>
  `/workspace/${encodeURIComponent(workspace)}/product-bulk-runs${suffix}`

/** Domain calls share authentication, error handling and cancellation with generated clients. */
export async function bulkRequest<T>(
  workspace: string,
  suffix = "",
  method = "GET",
  data?: unknown,
  signal?: AbortSignal
): Promise<T> {
  return (
    await apiRequest<{ data: T }>({
      url: bulkPath(workspace, suffix),
      method,
      data,
      signal,
    })
  ).data
}
export async function fieldRequest<T>(
  workspace: string,
  suffix = "",
  method = "GET",
  data?: unknown,
  signal?: AbortSignal
): Promise<T> {
  return (
    await apiRequest<{ data: T }>({
      url: `/workspace/${encodeURIComponent(workspace)}/product-fields${suffix}`,
      method,
      data,
      signal,
    })
  ).data
}
export const TERMINAL_STATES = new Set([
  "completed",
  "completed_with_errors",
  "cancelled",
  "failed",
  "expired",
])
export function runState(
  run: Pick<BulkRunSummary, "state" | "counts">
): string {
  return run.state === "completed" &&
    ((run.counts?.errors ?? 0) > 0 || (run.counts?.stale ?? 0) > 0)
    ? "completed_with_errors"
    : run.state
}
const ACTIVE_STATES = new Set([
  "extracting",
  "extract",
  "preparing",
  "validating",
  "running",
])
export function bulkPollInterval(
  states: string[],
  visible: boolean
): number | false {
  if (!visible) return false
  if (states.some((state) => ACTIVE_STATES.has(state))) return 2000
  if (states.some((state) => state === "queued" || state === "queue"))
    return 10000
  return false
}
export function canEditBulk(run: BulkRun): boolean {
  return (
    run.permissions.edit &&
    [
      "staged",
      "draft",
      "needs_mapping",
      "needs_input",
      "ready",
      "review",
    ].includes(run.state) &&
    !run.reviews.some((review) =>
      ["approved", "queued", "running", "completed"].includes(
        review.state ?? ""
      )
    )
  )
}
export function reviewFor(
  run: BulkRun,
  scope: BulkReview["scope"],
  now = Date.now()
): BulkReview | undefined {
  return run.reviews.find(
    (review) =>
      review.scope === scope &&
      review.revision === run.revision &&
      Date.parse(review.expires_at) > now &&
      (!review.state || review.state === "pending" || review.state === "ready")
  )
}
export function validFiles(files: File[]): boolean {
  return (
    files.length <= 10 &&
    files.every(
      (file) =>
        file.size > 0 &&
        file.size <= 10 * 1024 * 1024 &&
        /\.(csv|xlsx|pdf)$/i.test(file.name)
    )
  )
}
export function displayValue(value: unknown): string {
  if (value === undefined) return "—"
  if (value === null) return "∅"
  return typeof value === "object" ? JSON.stringify(value) : String(value)
}
export function errorDetails(error: unknown): string | null {
  const data = (
    error as {
      response?: {
        data?: { message?: string; errors?: Record<string, string[]> }
      }
    } | null
  )?.response?.data
  if (!data) return null
  return [
    data.message,
    ...Object.entries(data.errors ?? {}).map(
      ([field, messages]) => `${field}: ${messages.join(" ")}`
    ),
  ]
    .filter(Boolean)
    .join(" · ")
}
export function rowLocation(row: BulkRow): string {
  return [
    row.source ?? row.source_id,
    row.sheet,
    row.page != null ? `p. ${row.page}` : null,
    row.line != null ? `#${row.line}` : null,
  ]
    .filter(Boolean)
    .join(" · ")
}
