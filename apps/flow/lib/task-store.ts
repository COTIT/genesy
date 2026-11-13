import { useEffect, useRef } from "react"
import { useStore } from "zustand"
import { createStore } from "zustand/vanilla"

import { useSession } from "@genesy/api-client/auth"
import type { VoiceOutcome, VoiceReview } from "@genesy/voice/lib/client-contract"
import type { RecoveryTask } from "@genesy/voice/lib/recovery"

type TaskState = {
  /** Whose task this is (`ownerKey`): nothing here is shown to another workspace or login. */
  owner: string | null
  /** The review the person last decided; with the provider's outcome it is the receipt. */
  decided: VoiceReview | null
  /** The decided review's own answer, kept: later answers never become its receipt. */
  result: VoiceOutcome | null
  /** A proposal cancelled before it was sent (a correction or Cancel): shown as never sent. */
  withdrawn: VoiceReview | null
  /** The resume prompt was dismissed for this ended conversation. */
  resumeDismissed: boolean
}

/** Business-task memory of the shell; the voice provider stays the only source of live state. */
const EMPTY: TaskState = {
  owner: null,
  decided: null,
  result: null,
  withdrawn: null,
  resumeDismissed: false,
}
export const taskStore = createStore<TaskState>()(() => ({
  owner: null,
  decided: null,
  result: null,
  withdrawn: null,
  resumeDismissed: false,
}))
/** One workspace and one login; null (signed out, no workspace) belongs to nobody. */
export const ownerKey = (workspaceId: string | null, token: string | null) =>
  workspaceId && token ? `${workspaceId}:${token}` : null
/** The task as `owner` may see it: someone else's (or nobody's) task is empty. */
export const taskFor = (state: TaskState, owner: string | null) =>
  owner && state.owner === owner ? state : EMPTY
const useOwner = () =>
  useSession((s) => ownerKey(s.workspaceId, s.token))
export const useTask = <T,>(pick: (state: TaskState) => T) => {
  const owner = useOwner()
  return useStore(taskStore, (state) => pick(taskFor(state, owner)))
}

const MEMORY = "genesy.flow.task"
/** The owner without its token: the workspace id plus a short hash (FNV-1a) of the login. */
const fingerprint = (owner: string) => {
  const [workspace, ...login] = owner.split(":")
  let hash = 0x811c9dc5
  for (const c of login.join(":"))
    hash = Math.imul(hash ^ c.charCodeAt(0), 0x01000193)
  return `${workspace}:${(hash >>> 0).toString(36)}`
}
/** A review as the card needs it: never its confirmation code or phrase. */
const bare = (review: VoiceReview | null) =>
  review ? { ...review, confirmationCode: "", confirmationPhrase: "" } : null

/** Keeps the task card for this tab (sessionStorage), so a reload finds it again. */
export function saveTask(state: TaskState) {
  try {
    if (!state.owner || !(state.decided || state.withdrawn))
      return sessionStorage.removeItem(MEMORY)
    sessionStorage.setItem(
      MEMORY,
      JSON.stringify({
        owner: fingerprint(state.owner),
        decided: bare(state.decided),
        result: state.result,
        withdrawn: bare(state.withdrawn),
      })
    )
  } catch {
    // Storage blocked or full: the card just isn't kept across a reload.
  }
}

/** This owner's kept card; anything else (another login or workspace, unreadable) is dropped. */
export function loadTask(owner: string): Partial<TaskState> {
  try {
    const raw = sessionStorage.getItem(MEMORY)
    if (!raw) return {}
    const kept = JSON.parse(raw) as Partial<TaskState> | null
    const review = (r: unknown) =>
      r === null || typeof (r as VoiceReview)?.ticket === "string"
    if (
      kept?.owner === fingerprint(owner) &&
      review(kept.decided ?? null) &&
      review(kept.withdrawn ?? null)
    )
      return {
        decided: kept.decided ?? null,
        result: kept.decided ? (kept.result ?? null) : null,
        withdrawn: kept.withdrawn ?? null,
      }
    sessionStorage.removeItem(MEMORY)
  } catch {
    try {
      sessionStorage.removeItem(MEMORY)
    } catch {
      // Storage blocked: nothing to drop.
    }
  }
  return {}
}

/**
 * P6-3: the outcome of an approval that was on its way when this tab reloaded, from Office's
 * status. The review carries no action id, so a batch matches by its id and an action by its
 * operation (the newest listed). Only "completed" is confirmed; anything else must be checked,
 * never repeated. Null while Office has not said.
 */
export function approvalOutcome(
  decided: VoiceReview,
  tasks: RecoveryTask[],
  current: VoiceOutcome | null = null
): VoiceOutcome | null {
  if (current && current.status !== "unknown") return null
  const task = tasks.find((t) =>
    decided.batchId
      ? t.data?.batchId === decided.batchId
      : !t.data?.batchId && t.data?.operation === decided.operationId
  )
  if (!task || task.status === "awaiting_review") return null
  if (current && task.status !== "completed") return null
  return { status: task.status === "completed" ? "completed" : "unknown" }
}

/** What a review leaving means: withdrawn (cancelled, never sent), decided (answered), or nothing. */
export function reviewLeft(
  before: VoiceReview | null,
  review: VoiceReview | null,
  outcome: VoiceOutcome | null,
  earlier: VoiceOutcome | null
): Pick<TaskState, "decided" | "withdrawn" | "result"> | null {
  if (!before || before.ticket === review?.ticket) return null
  // Left with no answer of its own (e.g. the connection dropped): neither sent nor decided here.
  if (!outcome || outcome === earlier) return null
  if (outcome.status === "cancelled")
    return { withdrawn: before, decided: null, result: null }
  return review ? null : { decided: before, withdrawn: null, result: outcome }
}

/**
 * Watches the provider once (from the shell): when a review leaves, it was either cancelled
 * (withdrawn, never sent) or decided (its outcome is the receipt). A new conversation starts clean.
 */
export function useTaskWatch(voice: {
  review: VoiceReview | null
  outcome: VoiceOutcome | null
  starting: boolean
  busy: boolean
  progress: { status: string } | null
  recoveredTasks: RecoveryTask[]
}) {
  const last = useRef<VoiceReview | null>(null)
  // The outcome showing while the review was open belongs to earlier work, never to this review.
  const earlier = useRef<VoiceOutcome | null>(null)
  const { review, outcome, starting } = voice
  const owner = useOwner()
  // Another workspace or login (switch, logout, another tab's login) starts from nothing.
  useEffect(() => taskStore.subscribe(saveTask), [])
  // A reload of this tab finds its own card again (never another owner's).
  useEffect(() => {
    if (taskStore.getState().owner !== owner)
      taskStore.setState(
        { ...EMPTY, ...(owner ? loadTask(owner) : {}), owner },
        true
      )
    last.current = null
  }, [owner])
  useEffect(() => {
    const before = last.current
    last.current = review
    if (review && before?.ticket !== review.ticket) earlier.current = outcome
    const next = reviewLeft(before, review, outcome, earlier.current)
    if (next && taskStore.getState().owner === owner) taskStore.setState(next)
  }, [review, outcome, owner])
  // P6-3: an approval on its way is kept as decided with no answer yet, so a reload finds it.
  const approving = Boolean(review && voice.busy && voice.progress?.status === "running")
  useEffect(() => {
    if (approving && review && taskStore.getState().owner === owner)
      taskStore.setState({ decided: review, result: null, withdrawn: null })
  }, [approving, review, owner])
  // After the reload: Office's status answers it (the confirm is never sent again).
  const { recoveredTasks } = voice
  useEffect(() => {
    const { decided, result } = taskStore.getState()
    if (review || !decided || taskStore.getState().owner !== owner) return
    const answer = approvalOutcome(decided, recoveredTasks, result)
    if (answer) taskStore.setState({ result: answer })
  }, [review, recoveredTasks, owner])
  useEffect(() => {
    if (starting) taskStore.setState({ ...EMPTY, owner }, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [starting])
  // Leaving the signed-in app forgets the task.
  useEffect(() => () => taskStore.setState(EMPTY, true), [])
}
