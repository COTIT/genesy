/** Guidance follows persisted run facts. It never grants product-write authority. */
export type MigrationRun = {
  state: string
  counts?: Record<string, number>
  sources?: { state: string }[]
  permissions?: { resume?: boolean }
}
export function migrationGuide(run: MigrationRun) {
  const counts = run.counts ?? {}
  const errors = (counts.errors ?? 0) + (counts.stale ?? 0)
  if (["completed", "cancelled", "failed", "expired"].includes(run.state))
    return {
      step: errors || run.state === "failed" ? "recovery" : "receipt",
      needs_user: Boolean(errors || run.state === "failed"),
    } as const
  if (run.state === "paused")
    return { step: "paused", needs_user: true } as const
  if (
    [
      "extracting",
      "extract",
      "preparing",
      "validating",
      "queued",
      "running",
    ].includes(run.state)
  )
    return { step: "working", needs_user: false } as const
  if (errors || run.state === "needs_input" || run.state === "needs_mapping")
    return { step: "clarify", needs_user: true } as const
  if (run.state === "ready" && (counts.ready ?? 0) > 0)
    return { step: "approval", needs_user: true } as const
  return { step: "source", needs_user: true } as const
}
