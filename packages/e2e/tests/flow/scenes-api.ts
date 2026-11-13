// F2: Flow home + scenes over a stand-in API. SIMULATED data (the honey story of the approved
// design); the real-stack specs in tests-stack/flow-{scenes,noticed}.spec.ts prove the real API.
import type { MockReply } from "../../src/api-mock"
import type { FakeApi } from "../office/fake-api"
import scenesEn from "../../../../apps/flow/messages/en/scenes.json" with { type: "json" }

export const sc = scenesEn.scenes

/** F5: home's Studio prompt reads the storefront design. Default: none yet (the API's empty state). */
export const NO_DESIGN = {
  data: {
    version: 0,
    draft_revision_id: null,
    published_revision_id: null,
    draft: null,
  },
}
export const NO_REVISIONS = {
  data: [],
  meta: {
    current_page: 1,
    last_page: 1,
    per_page: 50,
    total: 0,
    from: null,
    to: null,
  },
}

const c = (id: string, type: string, metric?: string, period?: string) => ({
  id,
  type,
  ...(metric ? { metric } : {}),
  ...(period ? { period } : {}),
})
/** B1's built-in defaults (apps/api/src/modules/flow_scenes/dto.rs). */
export const DEFAULTS: Record<
  string,
  { id: string; type: string; metric?: string; period?: string }[]
> = {
  pulse: [
    c("sales-today", "metric_card", "sales.gross", "today"),
    c("orders-pending", "metric_card", "orders.pending", "today"),
    c("sales-trend", "line_chart", "sales.gross", "last_7_days"),
    c("attention", "attention_panel"),
  ],
  stock: [
    c("below-minimum", "metric_card", "stock.below_minimum", "today"),
    c("stock-list", "stock_list"),
  ],
  noticed: [c("attention", "attention_panel")],
  orders: [
    c("orders-pending", "metric_card", "orders.pending", "today"),
    c("orders-draft", "metric_card", "orders.draft", "today"),
    c("orders", "order_list"),
  ],
  sales: [
    c("sales-week", "metric_card", "sales.gross", "this_week"),
    c("net-week", "metric_card", "sales.net", "this_week"),
    c("sales-daily", "bar_chart", "sales.gross", "last_7_days"),
    c("vs-last-week", "comparison", "sales.gross", "this_week"),
  ],
  routines: [c("reports", "report_list")],
  studio: [],
}
const ORDER_AREA = (x: { type: string; metric?: string }) =>
  x.type === "order_list" ||
  !!x.metric?.startsWith("orders.") ||
  !!x.metric?.startsWith("sales.")

export const HONEY = "01950000-0000-7000-8000-0000000000a1"
export const TEA = "01950000-0000-7000-8000-0000000000a2"
export const SPICES = "01950000-0000-7000-8000-0000000000a3"
const stockItems = [
  {
    product_id: HONEY,
    product_name: "Raw Honey 500g",
    available: "14.00",
    minimum: "20.00",
    shortfall: "6.00",
  },
  {
    product_id: TEA,
    product_name: "Organic Tea",
    available: "7.00",
    minimum: "12.00",
    shortfall: "5.00",
  },
  {
    product_id: SPICES,
    product_name: "Natural Spices",
    available: "4.00",
    minimum: "10.00",
    shortfall: "6.00",
  },
]

type Saved = {
  position?: number
  hidden?: boolean
  config?: { components: unknown[] }
}

/**
 * Registers the scene world on a FakeApi. `role: "ops"` lacks order.list + invoice.list (sales
 * and order components are restricted, attention order/invoice sections are null).
 * Returns handles a test uses to change the world (resolve stock, inspect saved rows).
 */
export function sceneWorld(
  api: FakeApi,
  {
    ws = "w1",
    role = "owner" as "owner" | "ops",
    timeZone = "Africa/Dar_es_Salaam",
    /** Products below minimum: the 3 named ones by default, or this many (bento fit edge data). */
    stockRows = undefined as number | undefined,
    /** No sales in any period (empty-state edge data). */
    noSales = false,
  } = {}
) {
  const saved = new Map<string, Saved>()
  const marks = new Map<string, { state: string }>()
  let stock =
    stockRows === undefined
      ? [...stockItems]
      : Array.from({ length: stockRows }, (_, i) => ({
          product_id: `01950000-0000-7000-8000-${String(i + 1).padStart(12, "0")}`,
          product_name: `Product ${i + 1}`,
          available: `${(i % 9) + 1}.00`,
          minimum: "10.00",
          shortfall: `${10 - ((i % 9) + 1)}.00`,
        }))
  const ops = role === "ops"
  const sceneOf = (key: string, position: number) => {
    const row = saved.get(key)
    const components = (row?.config?.components ?? DEFAULTS[key] ?? []) as {
      id: string
      type: string
      metric?: string
    }[]
    return {
      scene: key,
      position: row?.position ?? position,
      hidden: row?.hidden ?? false,
      saved: !!row,
      schema_version: 1,
      config: { components },
      restricted: ops ? components.filter(ORDER_AREA).map((x) => x.id) : [],
    }
  }
  const list = () =>
    [
      ...Object.keys(DEFAULTS).map((k, i) => sceneOf(k, i)),
      ...[...saved.keys()]
        .filter((k) => !(k in DEFAULTS))
        .map((k) => sceneOf(k, 7)),
    ].sort((a, b) => a.position - b.position || a.scene.localeCompare(b.scene))
  const visibleStock = () =>
    stock.filter((s) => marks.get(s.product_id)?.state !== "snoozed")
  const base = `/workspace/${ws}`
  const sums = (gross: number) => ({
    gross_total: gross,
    net_total: Math.round(gross / 1.18),
    vat_total: 0,
    refunded_total: 0,
    orders: 24,
  })
  api
    .on("GET", `${base}/flow/scenes`, () => ({ data: list() }))
    .on("PUT", `${base}/flow/scenes/:scene`, ({ params, body }) => {
      const [key] = params
      saved.set(key!, { ...saved.get(key!), ...(body as Saved) })
      return {
        data: sceneOf(
          key!,
          Object.keys(DEFAULTS).indexOf(key!) >= 0
            ? Object.keys(DEFAULTS).indexOf(key!)
            : 7
        ),
      }
    })
    .on("DELETE", `${base}/flow/scenes/:scene`, ({ params }) => {
      saved.delete(params[0]!)
      return { status: 204 }
    })
    .on("POST", `${base}/flow/scenes/reset`, () => {
      saved.clear()
      return { status: 204 }
    })
    .on("GET", `${base}/attention`, () => ({
      data: {
        generated_at: "2026-11-11T06:00:00Z",
        lots: { quarantined: 0, items: [] },
        till_shifts: null,
        webhooks: null,
        orders: ops
          ? null
          : { open: 2, draft: 1, awaiting_payment: 0, items: [] },
        invoices: ops
          ? null
          : { draft: 0, overdue: 1, unpaid: 3, open: [], items: [] },
        stock: {
          negative: 0,
          out_of_stock: 0,
          items: [],
          below_minimum: { count: stock.length, items: stock },
        },
      },
    }))
    .on("GET", `${base}/attention/priorities`, () => {
      const items = visibleStock()
      const groups = [
        items.length
          ? {
              kind: "stock.below_minimum",
              count: items.length,
              open_count: items.length,
              acknowledged_count: items.filter((i) => marks.has(i.product_id))
                .length,
              snoozed_count: stock.length - items.length,
              snoozed: false,
              headline_ref: items[0]!.product_id,
              impact: { open_orders: ops ? null : 8 },
              actions: ["review", "acknowledge", "snooze"],
              items: items.map((i) => ({
                ref_id: i.product_id,
                label: i.product_name,
                available: i.available,
                minimum: i.minimum,
                shortfall: i.shortfall,
                open_orders: ops ? null : 3,
                acknowledged: marks.get(i.product_id)?.state === "acknowledged",
                money: null,
                since: null,
              })),
            }
          : null,
        ops
          ? null
          : {
              kind: "orders.open",
              count: 2,
              open_count: 2,
              acknowledged_count: 0,
              snoozed_count: 0,
              snoozed: false,
              headline_ref: null,
              impact: { open_orders: 2 },
              actions: ["review"],
              items: [],
            },
      ].filter(Boolean)
      return {
        data: {
          generated_at: "2026-11-11T06:00:00Z",
          groups,
          restricted: ops ? ["orders.open", "invoices.overdue"] : [],
        },
      }
    })
    .on("PUT", `${base}/attention/marks`, ({ body }) => {
      const b = body as { ref_id: string; state: string }
      marks.set(b.ref_id, { state: b.state })
      return { data: { id: `mark-${b.ref_id}`, ...b } }
    })
    .on("GET", `${base}/sales-summary`, ({ query }) =>
      ops
        ? { status: 403, body: { message: "Forbidden" } }
        : // Like the API: RFC 3339 instants only, a bare day is a 422.
          ![query.get("from"), query.get("to")].every((v) => v?.includes("T"))
          ? {
              status: 422,
              body: { message: "The from must be an RFC 3339 time." },
            }
          : {
              data: {
                from: query.get("from"),
                to: query.get("to"),
                source: null,
                currencies: noSales
                  ? []
                  : [{ currency: "EUR", decimals: 2, ...sums(1284000) }],
              },
            }
    )
    .on("GET", `${base}/sales/compare`, () =>
      ops
        ? { status: 403, body: { message: "Forbidden" } }
        : {
            data: {
              time_zone: timeZone,
              period: {
                from: "2026-11-09T00:00:00Z",
                to: "2026-11-12T00:00:00Z",
              },
              compare_to_period: {
                from: "2026-11-02T00:00:00Z",
                to: "2026-11-05T00:00:00Z",
              },
              currencies: noSales
                ? []
                : [
                    {
                      currency: "EUR",
                      decimals: 2,
                      current: sums(1284000),
                      base: sums(1070000),
                      gross_difference: 214000,
                      net_difference: 181356,
                      orders_difference: 0,
                      gross_change_bp: 2000,
                      net_change_bp: 2000,
                    },
                  ],
            },
          }
    )
    .on("GET", `${base}/sales/daily`, ({ query }) => {
      if (ops) return { status: 403, body: { message: "Forbidden" } }
      const from = query.get("from")!
      const to = query.get("to")!
      const days: {
        date: string
        gross: number
        net: number
        orders: number
      }[] = []
      for (
        let d = Date.parse(`${from}T00:00:00Z`);
        d < Date.parse(`${to}T00:00:00Z`);
        d += 86_400_000
      )
        days.push({
          date: new Date(d).toISOString().slice(0, 10),
          gross: 150000 + days.length * 10000,
          net: 127000,
          orders: 3,
        })
      return {
        data: {
          time_zone: timeZone,
          from,
          to,
          currencies: [{ currency: "EUR", decimals: 2, days }],
        },
      }
    })
    .on("GET", `${base}/orders`, () =>
      ops
        ? { status: 403, body: { message: "Forbidden" } }
        : {
            data: [
              {
                id: "o1",
                number: "ORD-0042",
                status: "open",
                created_at: "2026-11-10T09:00:00Z",
                total: 125050,
                currency: "EUR",
                decimals: 2,
                customer: null,
              },
              {
                id: "o2",
                number: "ORD-0043",
                status: "open",
                created_at: "2026-11-11T07:30:00Z",
                total: 48000,
                currency: "EUR",
                decimals: 2,
                customer: null,
              },
            ],
            meta: { current_page: 1, last_page: 1, per_page: 10, total: 2 },
          }
    )
    .on("GET", `${base}/product-bulk-runs`, () => ({
      data: { runs: [], next: null },
    }))
    .on("GET", `${base}/scheduled-reports`, () => ({ data: [] }))
    .on("GET", `${base}/storefront/pages/home`, () => NO_DESIGN)
    .on("GET", `${base}/storefront/pages/home/revisions`, () => NO_REVISIONS)
  return {
    saved,
    marks,
    /** Stock raised above the minimum: the rule alert clears (the API recomputes it). */
    resolve: (productId: string) => {
      stock = stock.filter((s) => s.product_id !== productId)
    },
  }
}

/** The same world for specs on the fixed `mockApi` table (read-only, owner role). */
export const sceneTable = (ws = "w1"): Record<string, MockReply> => ({
  [`GET /workspace/${ws}/flow/scenes`]: {
    body: {
      data: Object.entries(DEFAULTS).map(([scene, components], position) => ({
        scene,
        position,
        hidden: false,
        saved: false,
        schema_version: 1,
        config: { components },
        restricted: [],
      })),
    },
  },
  [`GET /workspace/${ws}/attention`]: {
    body: {
      data: {
        generated_at: "2026-11-11T06:00:00Z",
        lots: { quarantined: 0, items: [] },
        till_shifts: null,
        webhooks: null,
        orders: { open: 2, draft: 1, awaiting_payment: 0, items: [] },
        invoices: null,
        stock: {
          negative: 0,
          out_of_stock: 0,
          items: [],
          below_minimum: { count: 0, items: [] },
        },
      },
    },
  },
  [`GET /workspace/${ws}/attention/priorities`]: {
    body: {
      data: {
        generated_at: "2026-11-11T06:00:00Z",
        groups: [],
        restricted: [],
      },
    },
  },
  [`GET /workspace/${ws}/sales-summary`]: {
    body: {
      data: {
        from: "2026-11-11",
        to: "2026-11-12",
        source: null,
        currencies: [],
      },
    },
  },
  [`GET /workspace/${ws}/sales/daily`]: {
    body: {
      data: {
        time_zone: "UTC",
        from: "2026-11-10",
        to: "2026-11-13",
        currencies: [],
      },
    },
  },
  [`GET /workspace/${ws}/product-bulk-runs`]: {
    body: { data: { runs: [], next: null } },
  },
  [`GET /workspace/${ws}/scheduled-reports`]: { body: { data: [] } },
  [`GET /workspace/${ws}/storefront/pages/home`]: { body: NO_DESIGN },
  [`GET /workspace/${ws}/storefront/pages/home/revisions`]: {
    body: NO_REVISIONS,
  },
})

/** A scene's More menu (keep Flow simple): every action besides Refresh lives there. */
export const moreMenu = (page: import("@playwright/test").Page) =>
  page.getByRole("button", { name: sc.moreActions })
/** An item of the open More menu, whatever its kind (action, choice, on/off). */
export const menuItem = (page: import("@playwright/test").Page, name: string) =>
  page
    .locator(
      '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]'
    )
    .filter({ hasText: name })
