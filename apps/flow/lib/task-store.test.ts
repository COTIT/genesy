import { beforeEach, expect, it } from "vitest"

import type { VoiceReview } from "@genesy/voice/lib/client-contract"
import { approvalOutcome, loadTask, ownerKey, reviewLeft, saveTask, taskFor } from "./task-store"

const review = (ticket: string) => ({ ticket }) as VoiceReview
const earlier = { status: "completed", data: [] }

it("a review still open, or the same ticket, changes nothing", () => {
  expect(reviewLeft(null, review("a"), null, null)).toBeNull()
  expect(reviewLeft(review("a"), review("a"), earlier, earlier)).toBeNull()
})

it("a cancelled review is withdrawn: never sent", () => {
  expect(
    reviewLeft(review("a"), null, { status: "cancelled" }, earlier)
  ).toEqual({ withdrawn: review("a"), decided: null, result: null })
})

it("an answered review is decided; its outcome becomes the receipt", () => {
  for (const status of ["completed", "failed", "unknown", "partial"])
    expect(reviewLeft(review("a"), null, { status }, earlier)).toEqual({
      decided: review("a"),
      withdrawn: null,
      // Its own answer is kept: later answers (another question) never become this receipt.
      result: { status },
    })
})

it("a review left with no answer of its own is neither decided nor withdrawn", () => {
  // The connection dropped: the outcome on screen is an earlier read's, not this review's.
  expect(reviewLeft(review("a"), null, earlier, earlier)).toBeNull()
  expect(reviewLeft(review("a"), null, null, null)).toBeNull()
})

it("task state belongs to one workspace and login; anything else sees an empty task", () => {
  const state = {
    owner: ownerKey("w1", "token-a"),
    decided: review("a"),
    result: { status: "completed" },
    withdrawn: review("b"),
    resumeDismissed: true,
  }
  expect(taskFor(state, ownerKey("w1", "token-a"))).toBe(state)
  for (const other of [ownerKey("w2", "token-a"), ownerKey("w1", "token-b"), ownerKey(null, null)])
    expect(taskFor(state, other)).toMatchObject({
      decided: null,
      result: null,
      withdrawn: null,
      resumeDismissed: false,
    })
  // No login or no workspace is nobody's: never matches a stored owner, even another empty one.
  expect(ownerKey(null, "token-a")).toBeNull()
  expect(ownerKey("w1", null)).toBeNull()
  expect(taskFor({ ...state, owner: null }, null).decided).toBeNull()
})

// A tiny sessionStorage for node: the memory is per tab.
const store = new Map<string, string>()
beforeEach(() => {
  store.clear()
  globalThis.sessionStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  } as Storage
})
const secret = {
  ...review("t1"),
  confirmationCode: "135790",
  confirmationPhrase: "Confirm action 135790",
}
const done = { status: "completed", data: [] } as never

it("the kept card comes back for its owner only, without code, phrase or token", () => {
  const owner = ownerKey("w1", "token-abcdefghijklmnopqrstuvwxyz")!
  saveTask({
    owner,
    decided: secret,
    result: done,
    withdrawn: null,
    resumeDismissed: false,
  })
  const raw = store.get("genesy.flow.task")!
  expect(raw).not.toContain("135790")
  expect(raw).not.toContain("token-abcdefghijklmnopqrstuvwxyz")
  expect(loadTask(owner).decided?.ticket).toBe("t1")
  expect(loadTask(owner).result).toEqual(done)
  // Another login in the same workspace, or another workspace: nothing, and the entry is dropped.
  expect(loadTask(ownerKey("w1", "other-login")!)).toEqual({})
  expect(store.size).toBe(0)
  saveTask({
    owner,
    decided: secret,
    result: done,
    withdrawn: null,
    resumeDismissed: false,
  })
  expect(loadTask(ownerKey("w2", "token-abcdefghijklmnopqrstuvwxyz")!)).toEqual(
    {}
  )
  expect(store.size).toBe(0)
})

it("an unreadable or nobody's memory is dropped; an empty task keeps nothing", () => {
  store.set("genesy.flow.task", "{not json")
  expect(loadTask("w1:x")).toEqual({})
  expect(store.size).toBe(0)
  saveTask({
    owner: "w1:x",
    decided: null,
    result: null,
    withdrawn: null,
    resumeDismissed: false,
  })
  expect(store.size).toBe(0)
})

// P6-3: an approval on its way when the tab reloaded is resolved from Office's status, never re-sent.
const approved = { ticket: "t", operationId: "stock_set_levels", summary: "Set stock levels" } as VoiceReview
const status = (s: string, operation = "stock_set_levels") => ({ status: s, data: { actionId: "a", operation } })

it("an approval in flight resolves to its receipt once Office says it completed", () => {
  expect(approvalOutcome(approved, [status("completed")])).toEqual({ status: "completed" })
})

it("a persisted unknown receipt upgrades only when its matching recovery completed", () => {
  const unknown = { status: "unknown", message: "Check the record" }
  expect(approvalOutcome(approved, [status("completed")], unknown)).toEqual({ status: "completed" })
  for (const state of ["awaiting_review", "running", "unknown", "failed", "partial", "cancelled"])
    expect(approvalOutcome(approved, [status(state)], unknown)).toBeNull()
  expect(approvalOutcome(approved, [], unknown)).toBeNull()
  expect(approvalOutcome(approved, [status("completed", "products_update")], unknown)).toBeNull()
})

it("recovery never replaces a verified terminal receipt", () => {
  for (const state of ["completed", "partial", "failed", "cancelled"])
    expect(approvalOutcome(approved, [status("completed")], { status: state, data: { receipt: "kept" } })).toBeNull()
})

it("running, unknown or failed after a reload: not confirmed yet, never 'not done'", () => {
  for (const s of ["running", "unknown", "failed", "partial"])
    expect(approvalOutcome(approved, [status(s)])).toEqual({ status: "unknown" })
})

it("waits while Office has not answered or lists only other operations", () => {
  expect(approvalOutcome(approved, [])).toBeNull()
  expect(approvalOutcome(approved, [status("completed", "products_update")])).toBeNull()
  expect(approvalOutcome(approved, [status("awaiting_review")])).toBeNull()
})

it("a batch is matched by its own id", () => {
  const batch = { ticket: "b", kind: "batch", batchId: "b-1" } as VoiceReview
  const task = { status: "completed", data: { batchId: "b-1", kind: "batch" as const } }
  expect(approvalOutcome(batch, [task])).toEqual({ status: "completed" })
  expect(approvalOutcome(batch, [{ ...task, data: { ...task.data, batchId: "b-2" } }])).toBeNull()
})
