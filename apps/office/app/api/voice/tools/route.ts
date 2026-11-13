import { randomUUID } from "node:crypto"
import {
  discoverTools,
  getOperation,
  isToolAllowed,
  prepareAction,
  tools,
  ToolUnavailableError,
} from "@/lib/voice/catalog"
import { VoiceError, voiceFailure, voiceResponse } from "@/lib/voice/server"
import { assistantRequest } from "@/lib/voice/bridge"
import { authorizeVoiceSession } from "@/lib/voice/identity"
import {
  actionStatus,
  proposeAction,
  restoreReview,
  type StoredAction,
} from "@/lib/voice/actions"
import { executeAction } from "@/lib/voice/workflow"
import { takeFilePermit, voiceInput } from "@/lib/voice/input"
import { requireAssistantAdmission } from "@/lib/voice/admission"
import {
  batchFailure,
  proposeBatch,
  batchStatus,
  type StoredBatch,
  BATCH_PREPARE_OPERATION,
  BATCH_STATUS_OPERATION,
  MODEL_BATCH_MAX,
} from "@/lib/voice/batches"
import { noteVoiceFailure, withVoiceTracing } from "@/lib/voice/tracing"
import { narrate } from "@/lib/voice/narration"
import { present, viewChoice } from "@/lib/voice/present"
import { describeCapabilities } from "@/lib/voice/capabilities.server"
import { asksForPage, openPage, pageTool, PAGE_TOOL } from "@/lib/voice/pages"
import { noteToolCall, trackGap, type GapScope } from "@/lib/voice/gaps"
import {
  importPrices,
  importPricesTool,
  IMPORT_PRICES_OPERATION,
  notIncludedFromModel,
  withNotIncluded,
} from "@/lib/voice/file-import"
import {
  conversationHistory,
  historyTool,
  HISTORY_TOOL,
} from "@/lib/voice/history"
import {
  productBulk,
  productBulkTool,
  PRODUCT_BULK_OPERATION,
} from "@/lib/voice/product-bulk"
export const runtime = "nodejs"

const statusTool = {
  id: "assistant_task_status",
  summary:
    "Check this session's recorded task outcomes; never retry unknown writes. After presenting a review, wait for its confirmed outcome with wait:true.",
  method: "GET",
  write: false,
  body: {
    type: "object",
    properties: {
      actionId: { type: "string", format: "uuid" },
      wait: { type: "boolean" },
    },
  },
}
const batchPrepareTool = {
  id: BATCH_PREPARE_OPERATION,
  summary:
    "Propose one fixed ordered batch of two through 25 fully resolved writes. One independent Office confirmation covers only these exact requests. Any failure, unknown, queued or partial result stops the remainder; no dynamic result references. For more than 25 changes, prepare the first 25 and set remaining to how many are left; offer the next part only after this batch's status is completed, as a new batch with its own review.",
  method: "POST",
  write: true,
  body: {
    type: "object",
    required: ["calls"],
    properties: {
      calls: {
        type: "array",
        minItems: 2,
        maxItems: MODEL_BATCH_MAX,
        items: {
          type: "object",
          required: ["operationId", "arguments"],
          properties: {
            operationId: { type: "string" },
            arguments: { type: "object" },
          },
        },
      },
      remaining: {
        type: "integer",
        minimum: 1,
        maximum: 10000,
        description:
          "How many changes of the same request are left after this batch; each later part gets its own review.",
      },
      unclear: {
        type: "array",
        maxItems: 20,
        description:
          "Lines in a shared photo or file you could not read clearly; shown to the user, never written.",
        items: {
          type: "object",
          required: ["seen"],
          properties: {
            line: { type: "integer" },
            seen: { type: "string", maxLength: 120 },
            note: { type: "string", maxLength: 120 },
          },
        },
      },
    },
  },
}
// Requests about many records ("mark all ... delivered") also see the fixed-batch tool.
const manyRecords = /batch|ordered|bulk|several|\b(all|every|each|multiple)\b/i

const batchStatusTool = {
  id: BATCH_STATUS_OPERATION,
  summary:
    "Read an owned fixed batch's recorded steps; never retry or resume unknown/paused writes. Use wait:true while independent Office confirmation is pending.",
  method: "GET",
  write: false,
  body: {
    type: "object",
    properties: {
      batchId: { type: "string", format: "uuid" },
      wait: { type: "boolean" },
    },
  },
}
/** Model bridge can propose operations and read status; it cannot approve or commit. */
export function POST(request: Request) {
  const scope: GapScope = {}
  return withVoiceTracing("genesy_assistant_tools", () =>
    handle(request, scope).then((response) => trackGap(scope, response))
  )
}
async function handle(request: Request, scope: GapScope) {
  let releaseFile: (() => void) | undefined
  let language = "en"
  let isBatch = false
  try {
    releaseFile = takeFilePermit(request)
    const { input, files } = await voiceInput(request)
    const statusRequest =
      input.command === "status" ||
      (input.command === "execute" &&
        input.operationId === PRODUCT_BULK_OPERATION &&
        ["status", "rows", "raw", "template", "cancel"].includes(
          String(
            (input.arguments as Record<string, unknown> | undefined)?.command
          )
        )) ||
      (input.command === "execute" &&
        (input.operationId === BATCH_STATUS_OPERATION ||
          input.operationId === statusTool.id))
    if (!statusRequest) await requireAssistantAdmission()
    const { context, session } = await authorizeVoiceSession(
      request,
      input.workspaceId,
      input.sessionId,
      statusRequest ? "read" : "live"
    )
    if (!statusRequest) await requireAssistantAdmission(context.workspaceId)
    if (!statusRequest && !session.conversation_id)
      throw new VoiceError("Conversation identity has not been bound.", 409)
    language = context.language ?? "en"
    if (!statusRequest) {
      scope.context = context
      scope.operationId =
        input.command === "execute"
          ? String(input.operationId ?? "")
          : undefined
      scope.query = String(input.query ?? scope.operationId ?? "").replace(
        /_/g,
        " "
      )
      noteToolCall(context, scope.operationId)
    }
    isBatch =
      input.operationId === BATCH_PREPARE_OPERATION ||
      input.operationId === BATCH_STATUS_OPERATION
    if (input.command === "discover") {
      if (files.size) throw new VoiceError("Discovery does not accept files.")
      const query = String(input.query ?? "")
      const wantsHistory =
        /history|conversation|earlier|previous|remember|preference|gesprek|geschiedenis|mazungumzo|awali/i.test(
          query
        )
      // The agent reports a request no tool can do as "unsupported: <short request>".
      const unsupported = /^unsupported:\s*(.+)/is.exec(query)?.[1]
      if (unsupported) {
        noteVoiceFailure("unsupported", undefined, unsupported)
        return voiceResponse({
          tools: [],
          reason: "unsupported",
          message:
            "Recorded for review. Tell the user Genesy can't do this; never promise it later.",
        })
      }
      // "What can you do?": curated, permission-filtered capability list (capabilities.ts).
      if (query === "capabilities")
        return voiceResponse({
          tools: [],
          capabilities: describeCapabilities(context.permissions, language),
        })
      const found = discoverTools(query, context.permissions)
      const canImport = isToolAllowed(
        getOperation("products_update")!,
        context.permissions
      )
      const wantsImport =
        canImport &&
        /import|price ?list|spreadsheet|excel|xlsx|csv|sheet/i.test(query)
      const bulkList = getOperation("products_list")
      const bulkView = getOperation("products_get")
      const wantsProductBulk =
        bulkList &&
        bulkView &&
        isToolAllowed(bulkList, context.permissions) &&
        isToolAllowed(bulkView, context.permissions) &&
        /assistant_product_bulk|product|catalog|katalog|bidhaa|migrat|migre|bulk|import|spreadsheet|excel|xlsx|csv|pdf/i.test(
          query
        )
      if (
        !found.length &&
        !wantsHistory &&
        !wantsImport &&
        !wantsProductBulk &&
        ![
          BATCH_PREPARE_OPERATION,
          BATCH_STATUS_OPERATION,
          statusTool.id,
        ].includes(query) &&
        !asksForPage(query) &&
        !manyRecords.test(query) &&
        !/status|task|pending|last/i.test(query)
      ) {
        // Logged for review: a real gap in Genesy, or a request outside its scope.
        noteVoiceFailure("unsupported", undefined, query)
        return voiceResponse({
          tools: [],
          reason: "unsupported",
          message: "No Genesy tool matches this request. Nothing was changed.",
        })
      }
      return voiceResponse({
        tools:
          query === HISTORY_TOOL && wantsHistory
            ? [historyTool]
            : query === PRODUCT_BULK_OPERATION
              ? wantsProductBulk
                ? [productBulkTool]
                : []
              : query === BATCH_PREPARE_OPERATION
                ? [batchPrepareTool]
                : query === BATCH_STATUS_OPERATION
                  ? [batchStatusTool]
                  : query === statusTool.id
                    ? [statusTool]
                    : query === IMPORT_PRICES_OPERATION
                      ? wantsImport
                        ? [importPricesTool]
                        : []
                      : [
                          ...(wantsHistory ? [historyTool] : []),
                          ...(wantsProductBulk ? [productBulkTool] : []),
                          ...(wantsImport ? [importPricesTool] : []),
                          ...found,
                          ...(manyRecords.test(query) && !wantsProductBulk
                            ? [batchPrepareTool, batchStatusTool]
                            : []),
                          ...(/status|task|pending|last/i.test(query)
                            ? [statusTool]
                            : []),
                          ...(asksForPage(query) ? [pageTool] : []),
                        ],
        domains: [
          ...new Set(
            tools
              .filter((tool) => isToolAllowed(tool, context.permissions))
              .map((tool) => tool.domain)
          ),
        ],
      })
    }
    const argumentsValue =
      input.arguments &&
      typeof input.arguments === "object" &&
      !Array.isArray(input.arguments)
        ? (input.arguments as Record<string, unknown>)
        : {}
    if (
      input.command === "execute" &&
      input.operationId === PRODUCT_BULK_OPERATION
    ) {
      const list = getOperation("products_list")
      const view = getOperation("products_get")
      if (
        !list ||
        !view ||
        !isToolAllowed(list, context.permissions) ||
        !isToolAllowed(view, context.permissions)
      )
        throw new VoiceError(
          "Product list and view permissions are required.",
          403
        )
      return voiceResponse(await productBulk(argumentsValue, context, files))
    }
    if (input.command === "execute" && input.operationId === HISTORY_TOOL) {
      if (files.size) throw new VoiceError("History reads do not accept files.")
      return voiceResponse({
        status: "completed",
        data: await conversationHistory(context, argumentsValue),
      })
    }
    if (
      input.command === "execute" &&
      input.operationId === BATCH_PREPARE_OPERATION
    ) {
      const unclear = notIncludedFromModel(argumentsValue.unclear)
      return voiceResponse(
        withNotIncluded(
          await proposeBatch(
            argumentsValue.calls,
            context,
            files,
            MODEL_BATCH_MAX,
            argumentsValue.remaining
          ),
          unclear
        )
      )
    }
    if (
      input.command === "execute" &&
      input.operationId === IMPORT_PRICES_OPERATION
    ) {
      if (!isToolAllowed(getOperation("products_update")!, context.permissions))
        throw new VoiceError("You do not have permission for this action.", 403)
      return voiceResponse(await importPrices(argumentsValue, context, files))
    }
    if (
      input.command === "execute" &&
      input.operationId === BATCH_STATUS_OPERATION
    ) {
      if (files.size)
        throw new VoiceError("Batch status does not accept files.")
      const batchId = argumentsValue.batchId
      if (batchId) {
        if (typeof batchId !== "string" || !/^[0-9a-f-]{36}$/i.test(batchId))
          throw new VoiceError("Invalid fixed batch reference.")
        const stored = await assistantRequest<StoredBatch>(
          context,
          `/batches/${batchId}`
        )
        if (stored.session_id !== session.id)
          throw new VoiceError(
            "This fixed batch belongs to another session.",
            403
          )
        return voiceResponse(batchStatus(stored, context.language))
      }
      const rows = await assistantRequest<StoredBatch[]>(
        context,
        `/sessions/${session.id}/batches`
      )
      return voiceResponse({
        status: "completed",
        data: rows.map((row) => batchStatus(row, context.language)),
      })
    }
    if (
      input.command === "status" ||
      input.command === "restore" ||
      (input.command === "execute" && input.operationId === statusTool.id)
    ) {
      if (files.size) throw new VoiceError("Task status does not accept files.")
      const actionId = input.actionId ?? argumentsValue.actionId
      if (actionId) {
        if (typeof actionId !== "string" || !/^[0-9a-f-]{36}$/i.test(actionId))
          throw new VoiceError("Invalid action reference.")
        const stored = await assistantRequest<StoredAction>(
          context,
          "/actions/" + actionId
        )
        if (stored.session_id !== session.id)
          throw new VoiceError("This action belongs to another session.", 403)
        if (input.command === "restore") {
          const operation = getOperation(stored.operation_id)
          if (!operation || !isToolAllowed(operation, context.permissions))
            throw new VoiceError("This action is no longer permitted.", 403)
          return voiceResponse(await restoreReview(stored, context))
        }
        return voiceResponse(actionStatus(stored, session.state === "active"))
      }
      if (input.command === "restore")
        throw new VoiceError("Choose a task to review.")
      const history = await assistantRequest<StoredAction[]>(
        context,
        "/sessions/" + session.id + "/actions"
      )
      const batches = await assistantRequest<StoredBatch[]>(
        context,
        "/sessions/" + session.id + "/batches"
      )
      return voiceResponse({
        status: "completed",
        data: [
          ...history
            .filter((stored) => !stored.batch_id)
            .map((stored) => actionStatus(stored, session.state === "active")),
          ...batches.map((row) => batchStatus(row, context.language)),
        ],
      })
    }
    if (input.command !== "execute")
      throw new VoiceError("Unknown assistant command.")
    if (input.operationId === PAGE_TOOL) {
      if (files.size) throw new VoiceError("Office pages do not accept files.")
      return voiceResponse(
        openPage(argumentsValue, context.permissions, language)
      )
    }
    const operationId = String(input.operationId ?? "")
    const operation = getOperation(operationId)
    if (
      !operation ||
      !["execute", "secure_ui"].includes(operation.disposition)
    ) {
      // Guessed IDs get permitted candidates so the model can retry without a discovery turn.
      const candidates = discoverTools(operationId, context.permissions)
      if (!candidates.length) noteVoiceFailure("unsupported", 400, operationId)
      return voiceResponse(
        {
          message: "This operation is not exposed to the assistant.",
          candidates,
          ...(candidates.length ? {} : { reason: "unsupported" }),
        },
        400
      )
    }
    if (!isToolAllowed(operation, context.permissions))
      throw new VoiceError("You do not have permission for this action.", 403)
    const choice = viewChoice(input, operation)
    if (operation.disposition === "secure_ui") {
      const route = operation.handoff?.route
      if (!route || !route.startsWith("/") || route.startsWith("//"))
        throw new VoiceError("The secure Office form is not available.", 503)
      return voiceResponse({
        status: "handoff",
        message:
          "Open the secure Office form. The conversation will stop before confidential input.",
        handoff: { route, operationId: operation.id },
      })
    }
    // Lines the model could not read: shown on the card, never part of the request.
    const { unclear: unclearValue, ...actionArguments } = argumentsValue
    const unclear = operation.write ? notIncludedFromModel(unclearValue) : []
    let action
    try {
      action = prepareAction(
        operation.id,
        operation.write ? actionArguments : (input.arguments ?? {}),
        context.workspaceId
      )
    } catch (error) {
      if (error instanceof ToolUnavailableError) throw error
      const message =
        error instanceof Error ? error.message : "Invalid arguments."
      // Lookups may run undiscovered: return their exact contract inline. Effects stay discover-first.
      if (!operation.write)
        return voiceResponse(
          {
            message,
            operation: discoverTools(operation.id, context.permissions)[0],
          },
          400
        )
      throw new VoiceError(message)
    }
    if (action.write)
      return voiceResponse(
        narrate(
          withNotIncluded(await proposeAction(action, context, files), unclear),
          "review",
          language
        )
      )
    if (files.size)
      throw new VoiceError("This lookup does not accept binary attachments.")
    return voiceResponse(
      present(
        narrate(
          await executeAction(action, context, randomUUID()),
          "lookup",
          language
        ),
        choice
      )
    )
  } catch (error) {
    return isBatch ? batchFailure(error, language) : voiceFailure(error)
  } finally {
    releaseFile?.()
  }
}
