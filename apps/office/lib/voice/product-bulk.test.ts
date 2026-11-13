import { beforeEach, describe, expect, it, vi } from "vitest"
const mock = vi.hoisted(() => ({ assistant: vi.fn(), fetch: vi.fn() }))
vi.mock("./bridge", async (original) => ({
  ...(await original<typeof import("./bridge")>()),
  assistantRequest: mock.assistant,
}))
import { productBulk } from "./product-bulk"
import type { VoiceContext } from "./server"
const id = "01950000-0000-7000-8000-000000000001"
const context = {
  token: "secret",
  workspaceId: "workspace",
  apiUrl: "https://api.test/v1",
  sessionId: "session",
  permissions: [],
} as VoiceContext
beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal("fetch", mock.fetch)
})
describe("durable product bulk voice helper", () => {
  it("inspects supported definitions and exact truncated cells without write authority", async () => {
    mock.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ data: { fields: [], custom_fields: [] } }))
    )
    await productBulk({ command: "template" }, context)
    expect(mock.fetch.mock.calls[0]?.[0]).toBe(
      "https://api.test/v1/workspace/workspace/product-bulk-runs/template"
    )
    mock.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({ data: { cells: [{ value: "source text" }], next: 8 } })
      )
    )
    const result = await productBulk(
      { command: "raw", run_id: id, row_id: id, after: 0 },
      context
    )
    expect(mock.fetch.mock.calls[1]?.[0]).toContain(
      `/${id}/rows/${id}/raw?page_size=8&after=0`
    )
    expect(result.data).toMatchObject({
      raw: { cells: [{ value: "source text" }] },
    })
    await expect(
      productBulk(
        { command: "raw", run_id: id, row_id: "../products", after: 0 },
        context
      )
    ).rejects.toThrow("exact staged row")
    expect(mock.fetch).toHaveBeenCalledTimes(2)
    expect(
      mock.fetch.mock.calls.every(([, options]) => options.method === "GET")
    ).toBe(true)
  })
  it("guides unresolved data before approval and reads bounded staged evidence", async () => {
    mock.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          data: {
            id,
            revision: 3,
            state: "ready",
            counts: { ready: 4, errors: 1 },
          },
        })
      )
    )
    const result = await productBulk({ command: "status", run_id: id }, context)
    expect(result.data).toMatchObject({
      guidance: { step: "clarify", needs_user: true },
    })
    mock.fetch.mockResolvedValueOnce(
      new Response(JSON.stringify({ data: { rows: [], next: null } }))
    )
    await productBulk(
      { command: "rows", run_id: id, row_status: "error", after: 20 },
      context
    )
    expect(mock.fetch.mock.calls[1]?.[0]).toBe(
      `https://api.test/v1/workspace/workspace/product-bulk-runs/${id}/rows?page_size=20&after=20&status=error`
    )
  })
  it("maps and corrects only exact revision-bound staged data", async () => {
    mock.fetch.mockImplementation(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({ data: { id, revision: 4, state: "preparing" } })
        )
      )
    )
    await expect(
      productBulk(
        {
          command: "mapping",
          run_id: id,
          source_id: id,
          header_row: 1,
          columns: { Price: "price" },
        },
        context
      )
    ).rejects.toThrow("current preparation revision")
    await productBulk(
      {
        command: "mapping",
        run_id: id,
        revision: 3,
        source_id: id,
        header_row: 1,
        columns: {
          Price: {
            target: "price",
            transform: { kind: "decimal", decimal_separator: "," },
          },
        },
      },
      context
    )
    expect(mock.fetch.mock.calls[0]?.[1].method).toBe("PATCH")
    expect(JSON.parse(mock.fetch.mock.calls[0]?.[1].body)).toMatchObject({
      revision: 3,
      source_id: id,
      columns: { Price: { target: "price" } },
    })
    await productBulk(
      {
        command: "correct",
        run_id: id,
        revision: 4,
        row_id: id,
        values: { name: "Tea", price: "12.50" },
      },
      context
    )
    expect(mock.fetch.mock.calls[1]?.[0]).toContain(`/${id}/rows/${id}`)
    expect(JSON.parse(mock.fetch.mock.calls[1]?.[1].body)).toEqual({
      revision: 4,
      values: { name: "Tea", price: "12.50" },
    })
  })
  it("can prepare an exact review but leaves approval on the Flow confirmation surface", async () => {
    mock.fetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({ data: { id, scope: "delete", row_count: 2 } })
      )
    )
    const result = await productBulk(
      { command: "review", run_id: id, revision: 5, scope: "delete" },
      context
    )
    expect(result.status).toBe("needs_input")
    expect(result.handoff?.route).toBe(`/imports/products/${id}`)
    expect(mock.fetch.mock.calls[0]?.[0]).toContain(`/${id}/reviews`)
    expect(mock.fetch).toHaveBeenCalledTimes(1)
    await expect(
      productBulk({ command: "approve", run_id: id, revision: 5 }, context)
    ).rejects.toThrow("Choose a product import command")
    expect(mock.fetch).toHaveBeenCalledTimes(1)
  })
  it("creates a draft without claiming product mutations or using native write tools", async () => {
    mock.fetch.mockResolvedValue(
      new Response(
        JSON.stringify({ data: { id, revision: 1, state: "staged" } })
      )
    )
    const result = await productBulk({ command: "create" }, context)
    expect(result.status).toBe("needs_input")
    expect(result.data).toMatchObject({ flow_path: `/imports/products/${id}` })
    expect(mock.fetch.mock.calls[0]?.[0]).toBe(
      "https://api.test/v1/workspace/workspace/product-bulk-runs"
    )
    expect(mock.fetch).toHaveBeenCalledTimes(1)
  })
  it("rejects approval commands and missing scoped selection", async () => {
    await expect(
      productBulk({ command: "approve", run_id: id }, context)
    ).rejects.toThrow("Choose a product import command")
    await expect(
      productBulk(
        { command: "stage", run_id: id, revision: 1, operation: "delete" },
        context
      )
    ).rejects.toThrow("explicit product selection")
    expect(mock.fetch).not.toHaveBeenCalled()
  })
  it("rejects foreign/missing attachments before creating a draft", async () => {
    await expect(
      productBulk({ command: "create", attachments: [id] }, context)
    ).rejects.toThrow("Share a supported file")
    expect(mock.fetch).not.toHaveBeenCalled()
  })
  it("passes exact revision and server-side transformations through unchanged", async () => {
    mock.fetch.mockResolvedValue(
      new Response(
        JSON.stringify({ data: { id, revision: 2, state: "preparing" } })
      )
    )
    const changes = { price: { kind: "percent", value: "7.5" } }
    expect(
      (
        await productBulk(
          {
            command: "stage",
            run_id: id,
            revision: 1,
            selection: { ids: [id] },
            operation: "update",
            changes,
          },
          context
        )
      ).status
    ).toBe("queued")
    expect(JSON.parse(mock.fetch.mock.calls[0]?.[1].body)).toEqual({
      revision: 1,
      selection: { ids: [id] },
      operation: "update",
      changes,
    })
  })
  it("queues instruction preparation once after creating a durable draft", async () => {
    mock.fetch
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({ data: { id, revision: 1, state: "staged" } })
        )
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({ data: { id, revision: 2, state: "preparing" } })
        )
      )
    const result = await productBulk(
      { command: "create", instruction: "Update active products by 7.5%" },
      context
    )
    expect(result.status).toBe("queued")
    expect(mock.fetch).toHaveBeenCalledTimes(2)
    expect(mock.fetch.mock.calls[1]?.[0]).toBe(
      `https://api.test/v1/workspace/workspace/product-bulk-runs/${id}/stage`
    )
    expect(JSON.parse(mock.fetch.mock.calls[1]?.[1].body)).toEqual({
      revision: 1,
      text: "Update active products by 7.5%",
    })
  })
  it("resumes only an explicitly revision-bound preparation", async () => {
    await expect(
      productBulk({ command: "resume", run_id: id }, context)
    ).rejects.toThrow("current preparation revision")
    mock.fetch.mockResolvedValue(
      new Response(
        JSON.stringify({ data: { id, revision: 2, state: "preparing" } })
      )
    )
    expect(
      (
        await productBulk(
          { command: "resume", run_id: id, revision: 2 },
          context
        )
      ).status
    ).toBe("queued")
    expect(JSON.parse(mock.fetch.mock.calls[0]?.[1].body)).toEqual({
      revision: 2,
    })
  })
})
