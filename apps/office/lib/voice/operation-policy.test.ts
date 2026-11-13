import { readFileSync } from "node:fs"
import { execFileSync } from "node:child_process"
import { describe, expect, it } from "vitest"
import {
  operationManifest,
  prepareAction,
  tools,
  isToolAllowed,
  discoverTools,
  prepareSecureAction,
  validateToolResponse,
  getOperation,
} from "./catalog"

const uuid = "0193f6b0-1c2d-7e3f-8a9b-0c1d2e3f4a5b"

describe("reviewed operation coverage", () => {
  it("exposes reviewed storefront design tools and keeps whole-document, preview and public reads internal", () => {
    const policies = JSON.parse(
      readFileSync(new URL("./operation-policy.json", import.meta.url), "utf8")
    )
    const reviewed: Record<string, { risk: string; effects: string[] }> = {
      get: { risk: "read", effects: [] },
      revisions: { risk: "read", effects: [] },
      capabilities: { risk: "read", effects: [] },
      edit: { risk: "write", effects: ["record"] },
      restore: { risk: "write", effects: ["record"] },
      publish: { risk: "high", effects: ["record", "publish"] },
    }
    for (const [suffix, expected] of Object.entries(reviewed)) {
      const id = `storefront_design_${suffix}`
      const tool = tools.find((tool) => tool.id === id)
      expect(policies[id]?.disposition, id).toBe("execute")
      expect(tool, id).toBeDefined()
      expect(tool!.risk, id).toBe(expected.risk)
      expect(tool!.effects, id).toEqual(expected.effects)
      expect(tool!.authorization.allOf, id).toEqual(["workspace_admin"])
      expect(tool!.confirmationFields, id).toEqual([])
      if (expected.effects.length) {
        expect(tool!.write, id).toBe(true)
        expect(tool!.idempotent, id).toBe(true)
        expect(tool!.retry.mode, id).toBe("idempotency_key")
        expect(tool!.reconciliation, id).toEqual(["storefront_design_get"])
      } else expect(tool!.write, id).toBe(false)
      expect(isToolAllowed(tool!, ["workspace_admin"]), id).toBe(true)
      expect(
        isToolAllowed(tool!, ["setting.update", "product.update"]),
        id
      ).toBe(false)
    }
    for (const suffix of ["save", "preview", "published", "edit_preview"]) {
      const id = `storefront_design_${suffix}`
      expect(policies[id]?.disposition, id).toBe("internal")
      expect(
        operationManifest.find((operation) => operation.id === id)?.disposition,
        id
      ).toBe("internal")
      expect(
        tools.some((tool) => tool.id === id),
        id
      ).toBe(false)
    }
  })

  it("prepares targeted design edits and refuses whole documents", () => {
    const edit = (body: unknown) =>
      prepareAction("storefront_design_edit", { body }, uuid)
    const action = edit({
      expected_version: 0,
      changes: [{ op: "set_theme", theme: "neutral" }],
    })
    expect(action.path).toBe(`/workspace/${uuid}/storefront/pages/home/edits`)
    expect(action.write).toBe(true)
    for (const bad of [
      {
        expected_version: 0,
        document: {},
        changes: [{ op: "set_theme", theme: "neutral" }],
      },
      {
        expected_version: 0,
        changes: [{ op: "replace_document", document: {} }],
      },
      { expected_version: 0, changes: [] },
    ])
      expect(() => edit(bad), JSON.stringify(bad)).toThrow()
    expect(() =>
      prepareAction("storefront_design_save", { body: {} }, uuid)
    ).toThrow()
  })

  it("reviews attention marks like design edits; priorities and daily sales are reads", () => {
    const tool = (id: string) => tools.find((t) => t.id === id)
    for (const id of ["attention_marks_set", "attention_marks_delete"]) {
      expect(tool(id)?.write, id).toBe(true)
      const op = (o: string) => operationManifest.find((m) => m.id === o)
      expect(op(id)?.disposition, id).toBe("execute")
      expect(op(id)?.risk, id).toBe(op("storefront_design_edit")?.risk)
    }
    // `write` actions only ever leave the tool route as a review card (`proposeAction`).
    const set = prepareAction(
      "attention_marks_set",
      {
        body: {
          kind: "stock.below_minimum",
          ref_id: uuid,
          state: "snoozed",
          snooze: "until_tomorrow",
        },
      },
      uuid
    )
    expect(set.write).toBe(true)
    expect(
      prepareAction("attention_marks_delete", { path: { mark: uuid } }, uuid)
        .write
    ).toBe(true)
    for (const id of ["attention_priorities", "orders_sales_daily"])
      expect(tool(id)?.write, id).toBe(false)
  })

  it("voices the member's own Flow scenes: list reads, every change is a review card", () => {
    const tool = (id: string) => tools.find((t) => t.id === id)
    expect(tool("flow_scenes_list")?.write).toBe(false)
    for (const id of [
      "flow_scenes_update",
      "flow_scenes_delete",
      "flow_scenes_reset",
    ]) {
      expect(tool(id)?.write, id).toBe(true)
      expect(getOperation(id)?.disposition, id).toBe("execute")
      expect(getOperation(id)?.risk, id).toBe("write")
      // Display settings of the caller only: any member may voice them, no extra right.
      expect(isToolAllowed(tool(id)!, []), id).toBe(true)
    }
    expect(
      prepareAction(
        "flow_scenes_update",
        { path: { scene: "pulse" }, body: { hidden: true } },
        uuid
      ).write
    ).toBe(true)
    expect(prepareAction("flow_scenes_reset", {}, uuid).write).toBe(true)
  })

  it("pins material-review, durable-delivery and cleanup authorities", () => {
    const policies = JSON.parse(
      readFileSync(new URL("./operation-policy.json", import.meta.url), "utf8")
    )
    const required: Record<string, string[]> = {
      products_create: [
        "modules/products/approval.rs",
        "modules/products/approval_repo.rs",
        "modules/products/service.rs",
        "modules/products/repo.rs",
      ],
      product_variants_generate: [
        "modules/products/approval.rs",
        "modules/products/approval_repo.rs",
        "modules/product_variants/service.rs",
        "modules/product_variants/dto.rs",
      ],
      invoices_create: [
        "modules/invoices/approval.rs",
        "modules/invoices/approval_repo.rs",
        "modules/invoices/service.rs",
        "modules/products/approval_repo.rs",
      ],
      invoices_update: [
        "modules/invoices/approval.rs",
        "modules/invoices/approval_repo.rs",
        "modules/invoices/service.rs",
        "modules/assistant/preconditions.rs",
      ],
      orders_invoice: [
        "modules/orders/service.rs",
        "modules/invoices/service.rs",
        "modules/invoices/approval_repo.rs",
        "modules/invoices/dto.rs",
        "modules/assistant/preconditions.rs",
        "modules/assistant/service.rs",
      ],
      orders_refund: [
        "modules/orders/payments.rs",
        "modules/orders/dto.rs",
        "modules/assistant/repo.rs",
        "modules/assistant/preconditions.rs",
        "http/assistant.rs",
      ],
      teams_set_permissions: [
        "modules/teams/dto.rs",
        "modules/teams/service.rs",
        "modules/teams/repo.rs",
        "modules/assistant/repo.rs",
      ],
      assistant_batches_create: [
        "modules/assistant/batch_handlers.rs",
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "modules/assistant/batch_dto.rs",
        "modules/products/approval.rs",
        "modules/invoices/approval.rs",
      ],
      assistant_batches_get: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "modules/assistant/service.rs",
      ],
      assistant_batches_list: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "modules/assistant/repo.rs",
      ],
      assistant_batches_approve: [
        "modules/assistant/batch_handlers.rs",
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "http/assistant.rs",
      ],
      assistant_batches_start: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "http/assistant.rs",
      ],
      assistant_batches_advance: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
        "http/assistant.rs",
      ],
      assistant_batches_pause: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
      ],
      assistant_batches_cancel: [
        "modules/assistant/batches.rs",
        "modules/assistant/batch_repo.rs",
      ],
      invoices_send: [
        "modules/invoices/approval.rs",
        "modules/invoices/send.rs",
        "modules/invoices/delivery.rs",
        "modules/invoices/delivery_repo.rs",
        "modules/invoices/repo.rs",
        "modules/messages/repo.rs",
        "jobs/kinds.rs",
      ],
      invoices_credit: [
        "modules/invoices/approval.rs",
        "modules/invoices/credit.rs",
        "modules/invoices/repo.rs",
      ],
      invoice_payments_create: [
        "modules/invoices/approval.rs",
        "modules/invoices/repo.rs",
      ],
      stock_set_levels: [
        "modules/assistant/preconditions.rs",
        "modules/assistant/preconditions_repo.rs",
      ],
      messages_send: [
        "modules/assistant/mail_recipients.rs",
        "modules/assistant/preconditions.rs",
        "modules/messages/dto.rs",
        "modules/messages/repo.rs",
      ],
      assistant_actions_create: [
        "modules/assistant/dto.rs",
        "modules/assistant/preconditions.rs",
        "modules/invoices/approval.rs",
      ],
      assistant_cleanup_ack: [
        "modules/assistant/cleanup.rs",
        "modules/assistant/retention.rs",
        "modules/assistant/batch_repo.rs",
        "jobs/kinds.rs",
        "cleanup.rs",
        "lib.rs",
        "main.rs",
        "config.rs",
      ],
    }
    for (const [id, paths] of Object.entries(required)) {
      const refs = policies[id].authorization.authority.map(
        (ref: string) => ref.split("#")[0]
      )
      for (const path of paths)
        expect(refs, `${id}: ${path}`).toContain(`apps/api/src/${path}`)
    }
    const pinned = JSON.parse(
      readFileSync(
        new URL("./operation-policy.authorities.json", import.meta.url),
        "utf8"
      )
    )
    const expected = new Set(
      Object.values(policies).flatMap((policy) =>
        (
          policy as { authorization: { authority: string[] } }
        ).authorization.authority.map((ref) => ref.split("#")[0])
      )
    )
    for (const path of [
      "http/auth.rs",
      "http/workspace.rs",
      "http/farm.rs",
      "modules/permissions/model.rs",
      "modules/permissions/catalog.rs",
      "idempotency.rs",
    ])
      expected.add(`apps/api/src/${path}`)
    expect(Object.keys(pinned).sort()).toEqual([...expected].sort())
  })

  it("retains typed mail receipts and bounded durable invoice task contracts", () => {
    for (const id of ["invoices_send", "invoices_resend"]) {
      expect(getOperation(id)?.reconciliation).toContain("invoices_send_tasks")
      expect(
        getOperation(id)?.successResponses["200"]?.content?.["application/json"]
          ?.schema
      ).toHaveProperty("$ref", "#/components/schemas/InvoiceDeliveryReply")
      expect(() =>
        validateToolResponse(id, 200, "application/json", {
          message: "ok",
          delivery: {
            invoice_delivery_id: uuid,
            invoice_id: uuid,
            status: "accepted",
            send_log_id: uuid,
            pdf_file_id: null,
          },
        })
      ).not.toThrow()
    }
    expect(
      getOperation("messages_send")?.successResponses["200"]?.content?.[
        "application/json"
      ]?.schema
    ).toHaveProperty("$ref", "#/components/schemas/SendMessageReply")
    expect(() =>
      validateToolResponse("invoices_send_tasks", 200, "application/json", {
        data: [
          { invoice_delivery_id: uuid, invoice_id: uuid, status: "queued" },
        ],
      })
    ).not.toThrow()
    expect(
      getOperation("products_list")?.parameters.find(
        (parameter) => parameter.name === "category_id"
      )?.schema
    ).toHaveProperty("format", "uuid")
  })

  it.each([
    ["invoices_update", "currency_id", uuid],
    ["invoices_update", "payment_method", "bank_transfer"],
    ["invoices_update", "is_credit", true],
    ["invoices_create", "due_date", "2026-11-07"],
    ["invoices_create", "customer_ref", { po_number: "PO-1" }],
  ])(
    "rejects native ignored field %s.%s before preparing an effect",
    (id, field, value) => {
      const operationId = String(id)
      const argumentsValue = {
        ...(operationId === "invoices_update"
          ? { path: { invoice: uuid } }
          : {}),
        body: { relation_id: uuid, [String(field)]: value },
      }
      expect(() => prepareAction(operationId, argumentsValue, uuid)).toThrow(
        /Unknown body field/
      )
      const discovered = discoverTools(operationId, ["workspace_owner"])[0]!
      expect(discovered.body).not.toHaveProperty(`properties.${field}`)
    }
  )

  it("keeps effective invoice header changes and create defaults available", () => {
    expect(() =>
      prepareAction(
        "invoices_update",
        {
          path: { invoice: uuid },
          body: {
            relation_id: uuid,
            note: "Updated note",
            due_date: "2026-11-07",
            customer_ref: { po_number: "PO-1" },
          },
        },
        uuid
      )
    ).not.toThrow()
    expect(() =>
      prepareAction(
        "invoices_create",
        {
          body: {
            relation_id: uuid,
            currency_id: uuid,
            payment_method: "bank_transfer",
            is_credit: true,
          },
        },
        uuid
      )
    ).not.toThrow()
  })

  it.each([
    ["custom_statuses_update", "custom_status", "domain", "activity"],
    ["custom_types_update", "custom_type", "domain", "activity"],
    ["discount_groups_update", "group", "internal", true],
    ["email_templates_update", "template", "template_domain", "invoice"],
  ])(
    "rejects immutable native fields on %s",
    (id, pathParameter, field, value) => {
      const operationId = String(id)
      const body =
        operationId === "email_templates_update"
          ? { body: "<p>Updated</p>", [String(field)]: value }
          : { name: "Updated", [String(field)]: value }
      expect(() =>
        prepareAction(
          operationId,
          { path: { [String(pathParameter)]: uuid }, body },
          uuid
        )
      ).toThrow(/Unknown body field/)
    }
  )

  it("rejects ignored nested credit flags before an immutable review", () => {
    expect(() =>
      prepareAction(
        "invoices_credit",
        {
          path: { invoice: uuid },
          body: {
            items: [
              {
                id: uuid,
                credit_amount: 1,
                return_amount: 1.25,
                disable_discount: true,
              },
            ],
          },
        },
        uuid
      )
    ).toThrow(/Unknown body.*disable_discount/)
    const body = discoverTools("invoices_credit", ["workspace_owner"])[0]!.body
    expect(body).not.toHaveProperty(
      "properties.items.items.properties.disable_discount"
    )
  })

  it("keeps supported credit quantity and VAT-inclusive money arguments", () => {
    expect(() =>
      prepareAction(
        "invoices_credit",
        {
          path: { invoice: uuid },
          body: {
            items: [{ id: uuid, credit_amount: 1, return_amount: 1.25 }],
          },
        },
        uuid
      )
    ).not.toThrow()
  })

  it("classifies every documented operation exactly once", () => {
    const spec = JSON.parse(
      readFileSync(
        new URL("../../../api/openapi.json", import.meta.url),
        "utf8"
      )
    )
    const ids = Object.values(spec.paths).flatMap((item) =>
      Object.values(item as Record<string, { operationId?: string }>).flatMap(
        (op) => (op.operationId ? [op.operationId] : [])
      )
    )
    expect(operationManifest.map((op) => op.id).sort()).toEqual(ids.sort())
    expect(operationManifest).toHaveLength(561)
    expect(tools).toHaveLength(415)
    expect(
      operationManifest.filter((op) => op.disposition === "secure_ui")
    ).toHaveLength(15)
    expect(
      operationManifest.filter((op) => op.disposition === "internal")
    ).toHaveLength(76)
    expect(new Set(operationManifest.map((op) => op.id)).size).toBe(ids.length)
    expect(
      operationManifest.every((op) => op.authorization.authority.length > 0)
    ).toBe(true)
  })

  it("files delivery corrections by voice but leaves the four-eyes decision to the Office", () => {
    const request = getOperation("farm_supply_correction_request")!
    expect(request.disposition).toBe("execute")
    expect(request.risk).toBe("high")
    expect(request.retry).toEqual({
      mode: "client_reference",
      field: "client_ref",
    })
    for (const id of [
      "farm_supply_correction_approve",
      "farm_supply_correction_reject",
    ]) {
      expect(getOperation(id)?.disposition).toBe("internal")
      expect(discoverTools(id, ["farm:ops", "farm:admin"])).toEqual([])
    }
  })

  it("keeps device and secret flows out of executable tools", () => {
    for (const id of [
      "till_login",
      "till_sales_create",
      "app_keys_create",
      "till_registers_rotate_key",
      "till_users_create",
      "settings_update",
    ]) {
      expect(tools.some((tool) => tool.id === id)).toBe(false)
      expect(
        operationManifest.find((tool) => tool.id === id)?.disposition
      ).not.toBe("execute")
    }
  })

  it("preserves empty success and multipart business contracts", () => {
    expect(
      tools.find((op) => op.id === "product_variants_delete")?.successResponses
    ).toHaveProperty("204")
    expect(
      tools.find((op) => op.id === "farm_documents_create")?.requestMedia
    ).toHaveProperty("multipart/form-data")
    expect(tools.find((op) => op.id === "supply_links_revoke")).toBeDefined()
  })

  it("uses effects and permission conjunctions from reviewed policy", () => {
    expect(tools.find((op) => op.id === "invoices_preview")?.write).toBe(true)
    expect(
      tools.find((op) => op.id === "pdf_templates_preview")?.effects
    ).toContain("file")
    expect(
      tools.find((op) => op.id === "messages_profile_test")?.effects
    ).toContain("email")
    expect(tools.find((op) => op.id === "messages_profile_test")?.write).toBe(
      true
    )
    const send = tools.find((op) => op.id === "invoices_send")!
    expect(send.authorization.allOf).toEqual([
      "invoice.send",
      "workspace.send-message",
    ])
    expect(isToolAllowed(send, ["invoice.send"])).toBe(false)
    expect(
      isToolAllowed(send, ["invoice.send", "workspace.send-message"])
    ).toBe(true)
    expect(isToolAllowed(send, ["workspace_owner"])).toBe(true)
    expect(
      tools.find((op) => op.id === "supply_links_list")?.authorization.allOf
    ).toEqual(["workspace_admin"])
  })

  it("requires trusted farm claims without a workspace-owner role bypass", () => {
    const supply = getOperation("farm_supply_create")!
    expect(isToolAllowed(supply, ["workspace_owner"])).toBe(false)
    expect(isToolAllowed(supply, ["workspace_admin"])).toBe(false)
    expect(isToolAllowed(supply, ["farm:farmer"])).toBe(false)
    expect(isToolAllowed(supply, ["farm:field_officer"])).toBe(false)
    expect(isToolAllowed(supply, ["farm:ops"])).toBe(true)
    expect(isToolAllowed(supply, ["farm:admin"])).toBe(true)
    const dispatch = getOperation("supply_chain_dispatch")!
    expect(isToolAllowed(dispatch, [])).toBe(false)
    expect(isToolAllowed(dispatch, ["product.update"])).toBe(true)
    expect(isToolAllowed(dispatch, ["farm:ops"])).toBe(true)
    expect(isToolAllowed(dispatch, ["farm:field_officer"])).toBe(false)
    const membership = getOperation("farm_memberships_create")!
    expect(isToolAllowed(membership, ["workspace_owner"])).toBe(true)
    expect(isToolAllowed(membership, ["workspace_admin"])).toBe(true)
    expect(isToolAllowed(membership, ["farm:admin"])).toBe(true)
    expect(isToolAllowed(membership, ["farm:ops"])).toBe(true)
    expect(isToolAllowed(membership, ["farm:field_officer"])).toBe(false)
  })

  it("hands physical verification to native UI and accepts only manual attestation", () => {
    const bodies = {
      farm_payouts_create: {
        client_ref: uuid,
        person_id: uuid,
        amount: 1250,
        currency_id: uuid,
        id_type: "nida",
        name_confirmed: true,
      },
      farm_vouchers_redeem: {
        code: "ABCD-EFGH",
        id_type: "nida",
        name_confirmed: true,
      },
    }
    for (const [id, body] of Object.entries(bodies)) {
      expect(() => prepareAction(id, { body }, uuid)).toThrow()
      const operation = getOperation(id)!
      expect(operation.disposition).toBe("secure_ui")
      expect(operation.effects).toContain("money")
      expect(operation.risk).toBe("high")
      const [discovered] = discoverTools(id, ["farm:field_officer"])
      expect(discovered?.body).toBeNull()
      expect(discovered?.requestMedia).toEqual({})
      expect(discovered?.handoff?.route).toBe(`/voice/secure?operation=${id}`)
      expect(() => prepareSecureAction(id, { body }, uuid)).not.toThrow()
      expect(() =>
        prepareSecureAction(
          id,
          { body: { ...body, name_confirmed: false } },
          uuid
        )
      ).toThrow()
      expect(() =>
        prepareSecureAction(
          id,
          { body: { ...body, name_confirmed: undefined } },
          uuid
        )
      ).toThrow()
    }
  })

  it("classifies assistant controls as internal and never allows self-approval tools", () => {
    const internal = operationManifest.filter((operation) =>
      operation.id.startsWith("assistant_")
    )
    expect(internal).toHaveLength(37)
    expect(
      internal
        .filter((operation) => operation.id.startsWith("assistant_batches_"))
        .map((operation) => operation.id)
        .sort()
    ).toEqual([
      "assistant_batches_advance",
      "assistant_batches_approve",
      "assistant_batches_cancel",
      "assistant_batches_create",
      "assistant_batches_get",
      "assistant_batches_list",
      "assistant_batches_pause",
      "assistant_batches_start",
    ])
    for (const operation of internal) {
      expect(operation.disposition).toBe("internal")
      expect(discoverTools(operation.id, ["workspace_owner"])).toEqual([])
      expect(() => prepareAction(operation.id, {}, uuid)).toThrow()
      expect(() => prepareSecureAction(operation.id, {}, uuid)).toThrow()
    }
  })

  it("validates UUIDs, query types, bodies and exact numeric values", () => {
    expect(() =>
      prepareAction("orders_get", { path: { order: "abc" } }, uuid)
    ).toThrow()
    expect(() =>
      prepareAction("orders_list", { query: { page: "first" } }, uuid)
    ).toThrow()
    expect(() =>
      prepareAction("orders_create", { body: { lines: "wrong" } }, uuid)
    ).toThrow()
    expect(() =>
      prepareAction(
        "orders_create",
        { body: { reference: Number.MAX_SAFE_INTEGER + 1 } },
        uuid
      )
    ).toThrow()
    expect(() =>
      prepareAction("orders_create", { body: { unexpected: "wrong" } }, uuid)
    ).toThrow()
    expect(
      prepareAction("orders_get", { path: { order: uuid } }, uuid).path
    ).toBe(`/workspace/${uuid}/orders/${uuid}`)
  })

  it("retains reconciliations and does not classify quotes as mutations", () => {
    expect(
      tools.find((op) => op.id === "invoices_send")?.reconciliation
    ).toContain("invoices_get")
    expect(tools.find((op) => op.id === "pricing_calculate_items")?.write).toBe(
      false
    )
    expect(() =>
      prepareAction("pricing_calculate_items", { body: {} }, uuid)
    ).not.toThrow()
    expect(
      tools.find((op) => op.id === "farm_pue_requests_estimate")?.write
    ).toBe(false)
  })

  it("redacts secure discovery and separates native preparation", () => {
    const [tool] = discoverTools("app_keys_create", ["workspace_owner"])
    expect(tool?.handoff?.route).toBe("/voice/secure?operation=app_keys_create")
    expect(tool?.requestMedia).toEqual({})
    expect(tool?.successResponses).toEqual({})
    expect(tool?.body).toBeNull()
    expect(() =>
      prepareSecureAction("orders_create", { body: {} }, uuid)
    ).toThrow()
    expect(() =>
      prepareAction("till_registers_create", { body: {} }, uuid)
    ).toThrow()
    expect(() =>
      prepareSecureAction(
        "till_registers_rotate_key",
        { path: { register: uuid } },
        uuid
      )
    ).not.toThrow()
  })

  it("searches compact descriptors after one full top match, then reveals self-contained exact input schemas", () => {
    const search = discoverTools("orders create", ["workspace_owner"])
    expect(search.length).toBeGreaterThan(1)
    expect(search[0]).toEqual(
      discoverTools(search[0]!.id, ["workspace_owner"])[0]
    )
    expect(
      search
        .slice(1)
        .every(
          (tool) =>
            tool.body === null &&
            tool.parameters.length === 0 &&
            Object.keys(tool.requestMedia).length === 0
        )
    ).toBe(true)
    const [order] = discoverTools("orders_create", ["workspace_owner"])
    expect(order?.body).toHaveProperty("properties.lines.items.required", [
      "product_id",
      "quantity",
    ])
    expect(JSON.stringify(order)).not.toContain('"$ref"')
    expect(order?.requestMedia["application/json"]?.schema).toEqual({})
    expect(
      order?.successResponses["201"]?.content?.["application/json"]?.schema
    ).toBeUndefined()
    const [upload] = discoverTools("farm_documents_create", [
      "farm:field_officer",
    ])
    expect(upload?.attachmentFields).toEqual(["file"])
  })

  it("prepares multipart with session handles, never body file bytes", () => {
    const action = prepareAction(
      "farm_documents_create",
      { path: { plot: uuid }, attachments: { file: ["app_file_1"] } },
      uuid
    )
    expect(action.contentType).toBe("multipart/form-data")
    expect(action.attachments).toEqual({ file: ["app_file_1"] })
    expect(() =>
      prepareAction(
        "farm_documents_create",
        { path: { plot: uuid }, body: { file: "base64" } },
        uuid
      )
    ).toThrow()
    expect(() =>
      prepareAction(
        "farm_documents_create",
        { path: { plot: uuid }, attachments: { file: ["https://other/file"] } },
        uuid
      )
    ).toThrow()
    expect(() =>
      prepareAction(
        "orders_create",
        { body: {}, attachments: { file: ["app_file_1"] } },
        uuid
      )
    ).toThrow()
  })

  it("checks documented empty success and response schema", () => {
    expect(() =>
      validateToolResponse("product_variants_delete", 204, "", null)
    ).not.toThrow()
    expect(() =>
      validateToolResponse("product_variants_delete", 204, "application/json", {
        ok: true,
      })
    ).toThrow()
    expect(() =>
      validateToolResponse("orders_get", 299, "application/json", {})
    ).toThrow()
    expect(() =>
      validateToolResponse("orders_get", 200, "application/json", {})
    ).toThrow()
  })

  it("retains current product category inputs and stock mutation results", () => {
    expect(getOperation("products_create")?.body).toHaveProperty(
      "properties.category_id.format",
      "uuid"
    )
    expect(getOperation("storefront_categories_list")?.disposition).toBe(
      "external"
    )
    expect(() =>
      validateToolResponse("stock_mutate", 200, "application/json", {
        message: "ok",
        before: "2.00",
        after: "3.00",
        warehouse: { id: uuid, name: "Shop" },
      })
    ).not.toThrow()
    expect(() =>
      validateToolResponse("stock_mutate", 200, "application/json", {
        message: "ok",
      })
    ).not.toThrow()
  })

  it(
    "compiles every success schema with shared OpenAPI references",
    { timeout: 30_000 },
    () => {
      for (const operation of operationManifest) {
        for (const [status, response] of Object.entries(
          operation.successResponses
        )) {
          for (const [media, content] of Object.entries(
            response.content ?? {}
          )) {
            if (!content.schema) continue
            try {
              validateToolResponse(operation.id, Number(status), media, null)
            } catch (error) {
              expect(
                (error as Error).message,
                `${operation.id}:${status}:${media}`
              ).toMatch(/^Invalid /)
            }
          }
        }
      }
    }
  )

  it(
    "fails generation for unknown, removed and changed-policy operations",
    { timeout: 30_000 },
    () => {
      const run = (mutation: string) =>
        execFileSync(
          process.execPath,
          [
            "--input-type=module",
            "-e",
            `
      import {readFileSync} from 'node:fs';
      import {buildCatalog} from './scripts/generate-voice-catalog.mjs';
      const spec=JSON.parse(readFileSync('../api/openapi.json','utf8'));
      const policy=JSON.parse(readFileSync('./lib/voice/operation-policy.json','utf8'));
      ${mutation}; buildCatalog(spec,policy);
    `,
          ],
          { cwd: process.cwd(), stdio: "pipe" }
        )
      expect(() => run("delete policy.orders_create")).toThrow()
      expect(() =>
        run("policy.removed_operation=policy.orders_create")
      ).toThrow()
      expect(() => run("policy.orders_create.path='/different'")).toThrow()
      expect(() =>
        run("policy.orders_create.authorization.schemes=['app_key']")
      ).toThrow()
      expect(() =>
        run("policy.orders_create.authorization.requirements=[]")
      ).toThrow()
      expect(() => run("policy.orders_create.disposition='typo'")).toThrow()
      expect(() =>
        run("policy.products_create.ignoredBodyFields=['name']")
      ).toThrow()
      expect(() =>
        run("policy.products_create.ignoredBodyFields=['missing_field']")
      ).toThrow()
      expect(() =>
        run(
          "policy.invoices_credit.ignoredBodySchemaProperties=['/properties/items/items/properties/id']"
        )
      ).toThrow()
      expect(() =>
        run("policy.products_create.availability='unknown'")
      ).toThrow()
      expect(() =>
        run("policy.products_create.availability='unavailable'")
      ).toThrow()
      expect(() => run("policy.orders_create.effects=[]")).toThrow()
      expect(() => run("policy.orders_create.effects=['credential']")).toThrow()
      expect(() =>
        run(`
      const control=policy.assistant_actions_approve;
      control.disposition='secure_ui'; control.effects=['credential'];
      control.handoff={route:'/voice/secure?operation=assistant_actions_approve',operationId:'assistant_actions_approve',kind:'credentials'};
    `)
      ).toThrow()
      expect(() =>
        run(`
      const source='/workspace/{workspace}/orders';
      const target='/workspace/{workspace}/assistant/actions/{action}/approve';
      spec.paths[target]={post:spec.paths[source].post};
      delete spec.paths[source].post;
      policy.orders_create.path=target;
    `)
      ).toThrow()
    }
  )
})
