"use client"

import { useEffect, useRef, useState } from "react"
import { usePathname } from "next/navigation"
import { useTranslations } from "next-intl"
import { ExternalLink, Mic, MicOff, Paperclip, Send } from "lucide-react"

import { Button, buttonVariants } from "@genesy/ui/components/button"
import { Input } from "@genesy/ui/components/input"
import { Notice, type NoticeVariant } from "@genesy/ui/components/notice"
import {
  AttachmentList,
  AttachmentPreview,
} from "@genesy/voice/components/attachments"
import { useVoice } from "@genesy/voice/components/provider"
import { VoiceUnfinished } from "@genesy/voice/components/unfinished"
import { ConversationResults } from "@genesy/voice/components/conversation-results"
import { isBlankMessage } from "@genesy/voice/lib/blank"
import { ATTACHMENT_ACCEPT } from "@genesy/voice/lib/helpers"
import type { SessionPhase } from "@genesy/voice/lib/presentation"
import { Pills } from "@/components/discovery/pills" // F3
import { EndCall } from "@/components/frame/conversation"
import { TaskCapsule } from "@/components/task/capsule" // F4
import { MigrationReviewHandoff } from "@/components/product-bulk/guidance"
import { ResumePrompt } from "@/components/task/resume-prompt" // F4
import { flowStore, useFlow } from "@/lib/flow-store"
import type { Readiness } from "@/lib/readiness"
import { useReadiness } from "@/lib/use-readiness"
import { parseWake } from "@/lib/wake"

const look: Record<
  Readiness,
  { variant: NoticeVariant; title: string; text?: string }
> = {
  available: { variant: "success", title: "assistantAvailable" },
  off: { variant: "info", title: "assistantOff", text: "assistantOffText" },
  not_ready: {
    variant: "warning",
    title: "assistantNotReady",
    text: "assistantNotReadyText",
  },
  unknown: { variant: "error", title: "assistantUnknown" },
}

/**
 * The same place on every screen: suggestions, what needs saying, then type / attach / talk / send.
 * Typing never opens the microphone; the unsent text survives navigation.
 */
export function Dock({ phase }: { phase: SessionPhase["phase"] }) {
  const t = useTranslations()
  const voice = useVoice()
  const pathname = usePathname()
  const draft = useFlow((s) => s.draft)
  const resultOpen = useFlow((s) => s.resultOpen)
  const sheetShown = useFlow((s) => s.sheetShown)
  const immersive = useFlow((s) => s.immersive)
  const picker = useRef<HTMLInputElement>(null)
  const previewReturn = useRef<HTMLElement | null>(null)
  const composer = useRef<HTMLInputElement>(null)
  const held = Boolean(voice.review)
  // A control turns disabled (upgrading, or held by a review) and the browser drops its focus to
  // the page: a keyboard user goes on at the review's button (K1), else in the message box (P9-4;
  // a tap never pops the phone keyboard). Held dock buttons stay focusable (aria-disabled, K4),
  // yet the review is still where a keyboard user goes next. Focus anywhere else is left alone.
  useEffect(() => {
    const active = document.activeElement as HTMLButtonElement | null
    if (!(voice.upgrading || held) || !active?.matches(":focus-visible")) return
    const heldHere = held && active.getAttribute("aria-disabled") === "true"
    if (!active.disabled && !active.closest("[inert]") && !heldHere) return
    const review = held
      ? document.querySelector<HTMLElement>(
          // P5: the card in the conversation; the capsule when the conversation is hidden.
          '[data-inline-task] #flow-review-title, [data-task-state="needs_approval"] button'
        )
      : null
    ;(review ?? composer.current)?.focus()
  }, [voice.upgrading, held])
  // End left the screen with the call: a keyboard user goes on at the mic (no phone keyboard
  // pops up), or the message box; on home (no message box) at the orb. Never at the page.
  const startVoice = useRef<HTMLButtonElement>(null)
  const wasConnected = useRef(voice.connected)
  useEffect(() => {
    const dropped = document.activeElement === document.body
    if (wasConnected.current && !voice.connected && dropped)
      requestAnimationFrame(() =>
        [
          startVoice.current?.disabled ? null : startVoice.current,
          composer.current,
          document.querySelector<HTMLElement>("[data-home-orb]"),
        ]
          .find((el) => el && el.offsetParent !== null)
          ?.focus()
      )
    wasConnected.current = voice.connected
  }, [voice.connected])
  const [previewKey, setPreviewKey] = useState<string | null>(null)
  const live = phase === "live"
  const homeDash =
    pathname === "/" &&
    !resultOpen &&
    !(
      (voice.connected || voice.starting || voice.upgrading) &&
      (immersive || voice.starting)
    )
  const { query: readiness, state } = useReadiness()

  // Clears the box only if it still holds what was sent (typing during the send stays).
  const deliver = async (text: string, typed: string) => {
    if ((await voice.send(text)) && flowStore.getState().draft === typed)
      flowStore.getState().setDraft("")
  }
  // A start the dock asked for: once it settles, send what the box holds then, or nothing.
  const sendOnConnect = useRef(false)
  useEffect(() => {
    if (voice.starting || !sendOnConnect.current) return
    sendOnConnect.current = false
    if (!voice.connected) return
    const typed = flowStore.getState().draft
    const { rest } = parseWake(typed.trim())
    if (rest && !isBlankMessage(rest)) void deliver(rest, typed)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [voice.starting, voice.connected])

  // One start at a time, also across the readiness check below.
  const opening = useRef(false)

  // F3: `text` lets a suggestion send its instruction exactly as typing it + Enter would.
  const submit = async (text = draft) => {
    const parsed = parseWake(text.trim())
    const { wake } = parsed
    // Only invisible characters (zero-width marks survive trim): nothing to send (P4-1).
    const rest = isBlankMessage(parsed.rest) ? "" : parsed.rest
    const files = voice.attachments.some((file) => file.state !== "submitted")
    if (!rest && !wake && !files) return
    if (!voice.connected) {
      if (opening.current || voice.starting || voice.upgrading) {
        sendOnConnect.current = true
        return
      }
      opening.current = true
      try {
        // Readiness may still be on its way: ask for the latest answer, never a stale one.
        const ready =
          state ?? (await readiness.refetch({ cancelRefetch: false })).data
        if (ready !== "available") return
        if (!rest) flowStore.getState().setDraft("")
        // Sent from the box once connected; a start that fails sends nothing later.
        sendOnConnect.current = true
        await voice.start(true, "command")
      } finally {
        opening.current = false
      }
      return
    }
    if (!rest && !files) return flowStore.getState().setDraft("")
    void deliver(rest, text)
  }

  const handoff = voice.outcome?.handoff?.route
  const handoffHref = handoff ? voice.resolveHref(handoff) : ""
  return (
    <section
      aria-label={t("shell.dock")}
      // Short screens (phone landscape, zoomed laptop): one row, suggestions beside the field.
      // Never taller than ~45% of the visible screen (P4-2): what sits above the field scrolls in
      // the dock's own pane; the field and Send always stay in view.
      className="flex max-h-[calc(var(--flow-vh,100dvh)*0.45)] min-w-0 shrink-0 flex-col gap-2 rounded-2xl glass-strong p-3 [&_li_p]:min-w-0 [&_li_p]:truncate [@media(max-height:500px)]:flex-row [@media(max-height:500px)]:flex-wrap [@media(max-height:500px)]:items-center [@media(max-height:500px)]:p-2 [@media(max-height:500px)]:[&>*]:basis-full [@media(max-height:500px)]:[&>[data-flow-pane]]:flex-[2_1_0] [@media(max-height:500px)]:[&>[data-flow-pane]]:basis-0 [@media(max-height:500px)]:[&>form]:flex-[3_1_0] [@media(max-height:500px)]:[&>form]:basis-0"
    >
      <div
        data-flow-pane="dock"
        className="relative flex max-h-[calc(var(--flow-vh,100dvh)*0.45-5rem)] min-h-0 min-w-0 shrink flex-col gap-2 overflow-y-auto overscroll-y-contain [@media(max-height:500px)]:max-h-[calc(var(--flow-vh,100dvh)*0.45-1rem)]"
      >
        <ConversationResults
          rows={voice.messages}
          completedRequestId={voice.completedRequestId}
          reportOpen={voice.reportOpen}
          onReportOpenChange={voice.setReportOpen}
        />
        {/* F3: dynamic suggestions; an instruction starts a text conversation, never the mic. */}
        <Pills
          disabled={Boolean(voice.review)}
          ask={(text) => {
            // On home the request and its answer show in the full conversation.
            if (homeDash) flowStore.setState({ immersive: true })
            flowStore.getState().setDraft(text)
            void submit(text)
          }}
        />
        {state && state !== "available" && !live ? (
          <Notice
            variant={look[state].variant}
            title={t(`flow.${look[state].title}`)}
            action={
              state === "unknown" || state === "not_ready" ? (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={readiness.isFetching}
                  onClick={() => void readiness.refetch()}
                >
                  {t("flow.retry")}
                </Button>
              ) : null
            }
          >
            {look[state].text ? t(`flow.${look[state].text}`) : null}
          </Notice>
        ) : null}
        {/* P9-4: always mounted, so a screen reader hears when a review arrives. A live region,
            not a second role="status": the dock's status line stays the one status. */}
        <p aria-live="polite" className="sr-only">
          {voice.review
            ? t("shell.reviewReady", { summary: voice.review.summary })
            : ""}
        </p>
        {phase === "ended" ? (
          <p role="status" className="text-sm">
            {t("shell.ended")}{" "}
            {voice.unfinished || voice.recoveredTasks.length
              ? `${t("shell.preserved")} `
              : null}
            <span className="text-muted-foreground">
              {t("shell.startAgain")}
            </span>
          </p>
        ) : null}
        <VoiceUnfinished />
        {/* F4: the active task (review, run, receipt) on home; scenes show it in the scene bar. */}
        {pathname === "/" && !resultOpen ? <TaskCapsule /> : null}
        <ResumePrompt
          idle={!(voice.connected || voice.starting || voice.upgrading)}
        />{" "}
        {/* F4; P5-1: also after a reload, never while starting */}
        {handoffHref.startsWith("/imports/products/") ? (
          <MigrationReviewHandoff href={handoffHref} />
        ) : handoff ? (
          <Notice variant="info" title={t("flow.chat.secureTitle")}>
            {handoffHref ? (
              <>
                <a
                  href={handoffHref}
                  target="_blank"
                  rel="noopener noreferrer"
                  className={buttonVariants({ className: "mt-2 min-h-11" })}
                >
                  {t("flow.chat.openInOffice")}
                  <ExternalLink aria-hidden />
                </a>
                <p className="mt-2 text-xs">{t("flow.chat.officeSignIn")}</p>
              </>
            ) : (
              t("flow.chat.officeNotLinked")
            )}
          </Notice>
        ) : null}
        {voice.error ? (
          <p role="alert" className="text-sm text-destructive">
            {voice.error}
          </p>
        ) : null}
        {voice.connected && !homeDash ? (
          <AttachmentList
            onPreview={(file, opener) => {
              previewReturn.current = opener
              setPreviewKey(file.key)
            }}
          />
        ) : null}
      </div>
      {/* While an action waits for approval nothing here sends or acts, so nothing cancels it by
          accident; the field itself stays focused and keeps what the person is typing (SW9). */}
      <form
        data-held={held || undefined}
        // Home's dashboard has no message box: the orb (or "Type instead") opens the conversation.
        className={`${homeDash ? "hidden" : "flex"} min-w-0 items-center gap-2`}
        onSubmit={(e) => {
          e.preventDefault()
          if (!held) void submit()
        }}
        onPaste={(e) => {
          if (held || !voice.connected || !e.clipboardData.files.length) return
          e.preventDefault()
          voice.choose(Array.from(e.clipboardData.files))
        }}
      >
        {voice.connected ? (
          <>
            <input
              ref={picker}
              type="file"
              multiple
              accept={ATTACHMENT_ACCEPT}
              aria-label={t("voice.chooseFiles")}
              hidden
              onChange={(e) => {
                voice.choose(Array.from(e.target.files ?? []))
                e.target.value = ""
              }}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t("voice.addFiles")}
              aria-disabled={held || undefined}
              className={`min-h-11 min-w-11 ${held ? "opacity-50" : ""}`}
              onClick={() => {
                if (!held) picker.current?.click()
              }}
            >
              <Paperclip aria-hidden />
            </Button>
          </>
        ) : null}
        <Input
          ref={composer}
          data-flow-composer
          aria-label={t("voice.message")}
          placeholder={t("shell.placeholder")}
          autoComplete="off"
          value={draft}
          onChange={(e) => {
            flowStore.getState().setDraft(e.target.value)
            if (voice.connected) voice.activity()
          }}
          className="min-h-11 min-w-0 flex-1"
        />
        {/* Short screens: the sheet's line is too slim for End, so it sits here (one on screen). */}
        {sheetShown ? (
          <EndCall
            tight
            className="hidden [@media(max-height:500px)]:inline-flex"
          />
        ) : null}
        {!live ? (
          <Button
            ref={startVoice}
            type="button"
            variant="outline"
            size="icon"
            className="min-h-11 min-w-11 aria-disabled:opacity-50"
            aria-label={t("voice.start")}
            aria-disabled={held || undefined}
            disabled={state !== "available"}
            onClick={() => {
              if (!held) void voice.start(false)
            }}
          >
            <Mic aria-hidden />
          </Button>
        ) : voice.connected && voice.textOnly ? (
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="min-h-11 min-w-11"
            aria-label={t("shell.switchVoice")}
            disabled={voice.busy || Boolean(voice.review)}
            onClick={() => void voice.upgrade()}
          >
            <Mic aria-hidden />
          </Button>
        ) : voice.connected ? (
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="min-h-11 min-w-11"
            aria-label={t("shell.mic")}
            aria-pressed={!voice.muted}
            onClick={voice.toggleMute}
          >
            {voice.muted ? <MicOff aria-hidden /> : <Mic aria-hidden />}
          </Button>
        ) : null}
        <Button
          type="submit"
          size="icon"
          className="min-h-11 min-w-11 aria-disabled:opacity-50"
          aria-label={t("voice.send")}
          // Unknown readiness still sends (Enter included); the start checks it first.
          // While a review waits it stays focusable (aria-disabled): disabling it would drop
          // the person's focus to <body> the moment the review arrives.
          aria-disabled={held || undefined}
          disabled={!live && !!state && state !== "available"}
        >
          <Send aria-hidden />
        </Button>
      </form>
      <AttachmentPreview
        previewKey={previewKey}
        onClose={() => setPreviewKey(null)}
        finalFocus={previewReturn}
      />
    </section>
  )
}
