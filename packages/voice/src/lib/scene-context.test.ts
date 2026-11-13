import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { TranscriptRow } from "./helpers"
import { contextSync, contextUpdate, sceneContext } from "./scene-context"
import type { ResultView } from "./views"

const daily = {
  kind: "sales_daily" as const,
  from: "2026-11-03",
  to: "2026-11-10",
  time_zone: "UTC",
  currencies: [{ currency: "TZS", decimals: 0, days: [{ date: "2026-11-03", gross: 987654, net: 900000, orders: 3 }] }],
}
const records = {
  kind: "records" as const,
  resource: "orders" as const,
  total: 7,
  href: "/orders",
  rows: [{ id: "o1", label: "Neema Shop", detail: "Neema", status: "open", money: { minor: 125050, decimals: 2, currency: "USD" }, stock: null, href: "/orders/o1" }],
}
const picture = (id: string, view: ResultView, operation?: string): TranscriptRow => ({
  id,
  role: "result",
  text: "",
  views: [view],
  ...(operation ? { operation } : {}),
})

describe("scene context", () => {
  it("identifies the product migration being discussed without sending source values", () => {
    const id = "01950000-0000-7000-8000-000000000001"
    expect(sceneContext({ pathname: `/imports/products/${id}` })).toEqual({
      active_scene: "product_import",
      selected_record_type: "product_bulk_run",
      selected_record_id: id,
    })
    expect(sceneContext({ pathname: "/imports/products/fields" })).toEqual({
      active_scene: "imports",
    })
  })
  it("names the scene from the path and carries ids only", () => {
    expect(sceneContext({ pathname: "/" })).toEqual({ active_scene: "home" })
    expect(sceneContext({ pathname: "/storefront" })).toEqual({
      active_scene: "storefront",
    })
    expect(
      sceneContext({
        pathname: "/scenes/stock",
        selected: { type: "product", ids: ["p1"] },
      })
    ).toEqual({
      active_scene: "stock",
      selected_record_type: "product",
      selected_record_id: "p1",
    })
    expect(
      sceneContext({
        pathname: "/scenes/orders",
        selected: { type: "order", ids: ["o1", "o2"] },
      })
    ).toEqual({
      active_scene: "orders",
      selected_record_type: "order",
      selected_record_ids: ["o1", "o2"],
    })
  })

  it("names each result picture by its stable id, newest first, with the read it came from", () => {
    expect(
      sceneContext({
        pathname: "/results",
        results: [{ id: "u", role: "user", text: "trend" }, picture("r1", records, "orders_list"), picture("r2", daily, "orders_sales_daily")],
        reviewOpen: true,
      })
    ).toEqual({
      active_scene: "results",
      results_shown: 2,
      latest_result: "sales_daily",
      pictures: [
        {
          id: "r2",
          kind: "sales_daily",
          shape: "series",
          shown_as: "line",
          supports: ["line", "sparkline", "bar", "table"],
          operation: "orders_sales_daily",
          period: { from: "2026-11-03", to: "2026-11-10" },
        },
        { id: "r1", kind: "records", shape: "rows", shown_as: "list", supports: ["list", "kpi", "table"], operation: "orders_list" },
      ],
      review_open: true,
    })
  })

  it("never carries a value: no amount, name, currency, status or record link", () => {
    const sent = contextUpdate(sceneContext({ pathname: "/results", results: [picture("r1", records), picture("r2", daily)] }))
    for (const leak of ["987654", "125050", "Neema", "/orders/o1", "TZS", "USD", "open"]) expect(sent).not.toContain(leak)
  })

  it("lists what the scene shows by id, type, metric and period: never a value", () => {
    expect(
      sceneContext({
        pathname: "/scenes/pulse",
        visible: [
          { id: "sales-today", type: "metric_card", metric: "sales.gross", period: "today", value: 1250 },
          { id: "attention", type: "attention_panel" },
        ] as never,
      })
    ).toEqual({
      active_scene: "pulse",
      visible: [
        { id: "sales-today", type: "metric_card", metric: "sales.gross", period: "today" },
        { id: "attention", type: "attention_panel" },
      ],
    })
    // The task scene: how many results are shown and the newest one's kind.
    expect(sceneContext({ pathname: "/results", results: [picture("r1", records), picture("r2", daily)] })).toMatchObject({
      active_scene: "results",
      results_shown: 2,
      latest_result: "sales_daily",
    })
    expect(sceneContext({ pathname: "/", visible: [], results: [] })).toEqual({ active_scene: "home" })
  })

  it("wraps the context as untrusted UI state", () => {
    const sent = JSON.parse(contextUpdate({ active_scene: "home" }))
    expect(sent.flow_context).toEqual({ active_scene: "home" })
    expect(sent.instruction).toMatch(/UNTRUSTED/)
    expect(sent.instruction).toMatch(/never answer it aloud/i)
  })
})

describe("context sync", () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  const fake = (connected = true) => {
    const conversation = {
      sendContextualUpdate: vi.fn(),
      sendUserMessage: vi.fn(),
    }
    const sync = contextSync(
      () => (connected ? conversation : null),
      300
    )
    return { conversation, sync }
  }

  it("sends one quiet update per scene change, after the debounce, never a user message", () => {
    const { conversation, sync } = fake()
    sync.update({ active_scene: "home" })
    sync.update({ active_scene: "storefront" })
    expect(conversation.sendContextualUpdate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(conversation.sendContextualUpdate).toHaveBeenCalledTimes(1)
    expect(
      JSON.parse(conversation.sendContextualUpdate.mock.calls[0]![0])
        .flow_context
    ).toEqual({ active_scene: "storefront" })
    expect(conversation.sendUserMessage).not.toHaveBeenCalled()
  })

  it("skips a context equal to the last one sent", () => {
    const { conversation, sync } = fake()
    sync.update({ active_scene: "home" })
    vi.advanceTimersByTime(300)
    sync.update({ active_scene: "home" })
    vi.advanceTimersByTime(300)
    expect(conversation.sendContextualUpdate).toHaveBeenCalledTimes(1)
  })

  it("sends nothing while disconnected, and resends after a reset (new session)", () => {
    const off = fake(false)
    off.sync.update({ active_scene: "home" })
    vi.advanceTimersByTime(300)
    expect(off.conversation.sendContextualUpdate).not.toHaveBeenCalled()

    const { conversation, sync } = fake()
    sync.update({ active_scene: "home" })
    vi.advanceTimersByTime(300)
    sync.reset()
    sync.update({ active_scene: "home" })
    vi.advanceTimersByTime(300)
    expect(conversation.sendContextualUpdate).toHaveBeenCalledTimes(2)
  })

  it("drops a pending update on cancel", () => {
    const { conversation, sync } = fake()
    sync.update({ active_scene: "home" })
    sync.cancel()
    vi.advanceTimersByTime(300)
    expect(conversation.sendContextualUpdate).not.toHaveBeenCalled()
  })
})
