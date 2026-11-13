"use client"

import Link from "next/link"
import { usePathname } from "next/navigation"
import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { LogOut } from "lucide-react"

import { useLogout, useSession } from "@genesy/api-client/auth"
import {
  useUsersMe,
  useUsersWorkspaces,
} from "@genesy/api-client/endpoints/users"
import { sessionStore } from "@genesy/api-client/session"
import { LocaleSwitcher } from "@genesy/i18n/locale-switcher"
import { Button, buttonVariants } from "@genesy/ui/components/button"
import { ConfirmDialog } from "@genesy/ui/components/confirm-dialog"
import { LogoIcon } from "@genesy/ui/components/logo"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@genesy/ui/components/select"
import { ThemeToggle } from "@genesy/ui/components/theme-toggle"
import { useVoice } from "@genesy/voice/components/provider"
import type { SessionPhase } from "@genesy/voice/lib/presentation"
import { settleWithin } from "@/lib/settle"
import { useHeaderReveal } from "./use-header-reveal"
import { useWorkspaceSwitch } from "./use-workspace-switch"

/** How long logout waits for the voice session to end with the still-valid login. */
const END_VOICE_MS = 3000

/** The workspaces this login may switch to: the API's list, limited to the login's own ids. */
export function useWorkspaceChoices() {
  const token = useSession((s) => s.token)
  const ids = useSession((s) => s.workspaceIds)
  const query = useUsersWorkspaces({ query: { enabled: Boolean(token) } })
  const choices = (query.data?.data ?? []).filter((w) => ids.includes(w.id))
  return { query, choices }
}

/** First visit on touch, on the start screen only: a one-time hint that the header is a swipe away. */
const HINT_KEY = "flow.headerHint.seen"

/**
 * App bar: name, voice status (words, not only colour), workspace and account controls.
 * It floats over the frame and never pushes content: hidden until the pointer reaches the top
 * edge, focus enters it, the handle is pressed or (touch) a swipe down from just below the top.
 * Hidden it stays a focusable landmark (never aria-hidden), so Tab reaches and reveals it.
 */
export function Header({ phase }: { phase: SessionPhase["phase"] }) {
  const t = useTranslations()
  const ref = useRef<HTMLElement>(null)
  const pathname = usePathname()
  const { shown, enter, leave, toggle } = useHeaderReveal(ref, pathname)
  const [hint, setHint] = useState(false)
  // Never over a scene's title: shown (and counted as seen) on the start screen only.
  const home = pathname === "/"
  useEffect(() => {
    if (!home || !matchMedia("(pointer: coarse)").matches) return
    try {
      if (localStorage.getItem(HINT_KEY)) return
      localStorage.setItem(HINT_KEY, "1")
    } catch {
      return
    }
    const show = setTimeout(() => setHint(true), 0)
    const done = setTimeout(() => setHint(false), 4000)
    return () => {
      clearTimeout(show)
      clearTimeout(done)
      setHint(false)
    }
  }, [home])
  const logout = useLogout()
  const voice = useVoice()
  const [leaving, setLeaving] = useState(false)
  const user = useUsersMe().data?.data.user
  const workspaceId = useSession((s) => s.workspaceId)
  const { choices } = useWorkspaceChoices()
  const current = choices.find((w) => w.id === workspaceId)
  const [asking, setAsking] = useState(false)
  const live =
    voice.connected ||
    voice.starting ||
    voice.upgrading ||
    Boolean(voice.review)
  const signOut = async () => {
    setLeaving(true)
    // End the voice session while the login still works; a hung end never blocks logout.
    await settleWithin(
      // A throw inside end() becomes a rejection here, so logout still runs.
      Promise.resolve().then(() => voice.end()),
      END_VOICE_MS
    )
    logout.mutate()
  }
  return (
    <>
      <a
        href="#flow-content"
        className="sr-only z-50 rounded-xl bg-foreground px-4 py-3 text-sm text-background focus:not-sr-only focus:absolute focus:top-3 focus:left-3 focus:flex focus:min-h-11 focus:items-center"
      >
        {t("shell.skipToContent")}
      </a>
      <button
        type="button"
        data-flow-handle
        aria-expanded={shown}
        aria-controls="flow-header"
        aria-label={t(shown ? "shell.hideHeader" : "shell.showHeader")}
        // With no Menu button, this is the visible way to the menu on every device: named,
        // with a tooltip for the mouse, a 44 px target and a bar that stands out (3:1).
        title={t(shown ? "shell.hideHeader" : "shell.showHeader")}
        onClick={toggle}
        className="absolute top-0 left-1/2 z-50 flex h-11 w-24 -translate-x-1/2 justify-center pt-[calc(var(--flow-safe-top)+6px)] focus-visible:outline-2 focus-visible:outline-(--gold) [@media(max-height:500px)]:w-16 [@media(max-height:500px)]:pt-[calc(var(--flow-safe-top)+2px)]"
      >
        <span
          aria-hidden
          className="h-1.5 w-12 rounded-full bg-muted-foreground"
        />
      </button>
      {/* Only away from a conversation, whose bar sits where the hint would (P10-8). */}
      {hint && !shown && !live ? (
        <p
          role="status"
          className="pointer-events-none absolute top-11 left-1/2 z-50 max-w-[calc(100vw-2rem)] -translate-x-1/2 truncate rounded-full glass-strong px-3 py-1.5 text-xs"
        >
          {t("shell.headerHint")}
        </p>
      ) : null}
      <header
        ref={ref}
        id="flow-header"
        data-flow-header
        data-shown={shown || undefined}
        onPointerEnter={(e) => e.pointerType === "mouse" && enter()}
        // Sliding in, it passes under a pointer resting at the top edge: leaving upwards is not
        // a leave (moving down into the page still is, via the page's own pointer moves).
        onPointerLeave={(e) =>
          e.pointerType === "mouse" &&
          e.clientY > e.currentTarget.getBoundingClientRect().top &&
          leave()
        }
        // Focus inside shows it at once (no slide): a focused control is on screen the moment it
        // has focus, also at 200% zoom (K2).
        className={`absolute inset-x-3 top-[calc(var(--flow-safe-top)+0.75rem)] z-40 flex flex-wrap items-center gap-2 rounded-2xl glass-strong px-4 py-2.5 shadow-lg transition-[translate,opacity] duration-200 ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-opacity md:inset-x-4 md:top-[calc(var(--flow-safe-top)+1rem)] ${shown ? "" : "pointer-events-none -translate-y-[calc(100%+2rem)] opacity-0 focus-within:pointer-events-auto focus-within:translate-y-0 focus-within:opacity-100 focus-within:transition-none motion-reduce:translate-y-0"}`}
      >
        <Link href="/" className="mr-auto flex min-h-11 items-center gap-2">
          <LogoIcon aria-hidden="true" className="size-7 shrink-0" />
          <span className="text-[15px] font-semibold">{t("flow.name")}</span>
        </Link>
        <p className="min-w-0 truncate text-xs text-muted-foreground max-sm:order-last max-sm:basis-full">
          {user ? (
            <>
              <span className="sr-only">{t("flow.signedInAs")} </span>
              {user.name}
            </>
          ) : null}
          {user && current ? " · " : null}
          {current ? (
            <>
              <span className="sr-only">{t("flow.workspace")} </span>
              {current.display_name}
            </>
          ) : null}
        </p>
        <p
          data-phase={phase}
          // A fixed minimum width: a status change never shifts the controls after it (P4).
          className="flex min-h-11 min-w-40 items-center gap-2 text-xs font-medium tracking-wide whitespace-nowrap uppercase"
        >
          <span
            aria-hidden
            className={`size-2 rounded-full ${phase === "live" ? "bg-(--gold)" : phase === "ended" ? "bg-destructive" : "bg-muted-foreground"}`}
          />
          <span className="sr-only">{t("shell.phaseLabel")}: </span>
          {t(`shell.phase.${phase}`)}
        </p>
        <Link
          href="/storefront"
          prefetch={false}
          aria-current={pathname === "/storefront" ? "page" : undefined}
          className={buttonVariants({
            variant: "ghost",
            size: "sm",
            className: "min-h-11",
          })}
        >
          {t("flow.chat.storefront")}
        </Link>
        {voice.historyEnabled ? (
          <Link
            href="/history"
            prefetch={false}
            aria-current={pathname === "/history" ? "page" : undefined}
            className={buttonVariants({
              variant: "ghost",
              size: "sm",
              className: "min-h-11",
            })}
          >
            {t("history.title")}
          </Link>
        ) : null}
        <Link
          href="/imports/products"
          prefetch={false}
          aria-current={
            pathname.startsWith("/imports/products") ? "page" : undefined
          }
          className={buttonVariants({
            variant: "ghost",
            size: "sm",
            className: "min-h-11",
          })}
        >
          {t("productBulk.title")}
        </Link>
        <WorkspacePicker />
        {/* Every header control is a 44 px target (SW7). */}
        <LocaleSwitcher className="min-h-11" />
        <ThemeToggle label={t("common.theme")} className="size-11" />
        <Button
          variant="ghost"
          size="sm"
          // Last, 44 px, with clear space: a near-miss tap never lands on it (P10-3).
          className="ml-2 min-h-11"
          disabled={leaving || logout.isPending}
          onClick={() => {
            // While a conversation or a task is live, signing out ends it: ask first.
            if (live) setAsking(true)
            else void signOut()
          }}
        >
          <LogOut aria-hidden="true" />
          {t("common.logout")}
        </Button>
        <ConfirmDialog
          open={asking}
          onOpenChange={setAsking}
          title={t("shell.signOutTitle")}
          description={t("shell.signOutText")}
          confirm={t("common.logout")}
          cancel={t("shell.signOutStay")}
          onConfirm={() => void signOut()}
        />
      </header>
    </>
  )
}

/** Shown only with more than one workspace to choose from (header, and home's business line). */
export function WorkspacePicker({
  from = "header",
  label,
  className = "min-h-11 max-w-48",
}: {
  from?: "header" | "home"
  label?: string
  className?: string
}) {
  const t = useTranslations()
  const workspaceId = useSession((s) => s.workspaceId)
  const { choices } = useWorkspaceChoices()
  const { trigger, said, remember } = useWorkspaceSwitch(workspaceId, from) // P9-3
  if (choices.length < 2) return null
  const options = choices.map((w) => ({ value: w.id, label: w.display_name }))
  return (
    <>
      <p role="status" className="sr-only">
        {said}
      </p>
      <Select
        items={options}
        value={workspaceId}
        onValueChange={(v) => {
          if (!v) return
          remember(v, options.find((o) => o.value === v)?.label ?? "")
          sessionStore.getState().selectWorkspace(v)
        }}
      >
        <SelectTrigger
          ref={trigger}
          aria-label={label ?? t("common.workspace")}
          className={className}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {options.map((o) => (
            <SelectItem
              key={o.value}
              value={o.value}
              // A visible ring on the focused option (a background alone is not seen, P9-2).
              className="min-h-11 focus:ring-2 focus:ring-(--gold) focus:ring-inset"
            >
              {o.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </>
  )
}
