"use client"

import * as React from "react"
import { useRouter } from "next/navigation"
import { useQueryClient } from "@tanstack/react-query"

import { useSession } from "@genesy/api-client/auth"
import { sessionStore } from "@genesy/api-client/session"
import {
  GenesyVoiceProvider,
  useVoice,
} from "@genesy/voice/components/provider"
import { HistoryContinuity } from "@/components/history-continuity"
import { Shell } from "@/components/shell"
import { useDraftKeeper } from "@/lib/draft-keeper"
import { flowHref } from "@/lib/links"
import { focusLoginOnArrival } from "@/lib/login-focus"
import { followOtherTabs } from "@/lib/other-tabs"
import { useOpenView } from "@/lib/use-open-view"

/**
 * Signed-in frame: send a logged-out visitor to /login, then the assistant around the shell.
 * The voice provider lives here, inside (app) only, so routes outside it never reach /api/voice.
 * Its defaults (/api/voice, /voice-audio) are Flow's own proxy and audio files.
 */
export function SignedIn({
  office,
  children,
}: {
  office: string | null
  children: React.ReactNode
}) {
  const router = useRouter()
  const client = useQueryClient()
  const hydrated = useSession((s) => s.hydrated)
  const token = useSession((s) => s.token)
  const resolveHref = React.useCallback(
    (href: string) => flowHref(href, office),
    [office]
  )
  const openView = useOpenView() // P4: the assistant opens a scene or view by name

  useDraftKeeper()
  // A logout, login or workspace switch in another tab reaches this one too.
  React.useEffect(() => followOtherTabs(client), [client])

  React.useEffect(() => {
    if (!hydrated || token) return
    // After an expired login, back to the same place (the login page checks `next`); a logout by choice starts at home.
    const here = window.location.pathname + window.location.search
    const back = sessionStore.getState().expired && here !== "/"
    focusLoginOnArrival()
    router.replace(back ? `/login?next=${encodeURIComponent(here)}` : "/login")
  }, [hydrated, token, router])

  if (!hydrated || !token) return null
  return (
    <GenesyVoiceProvider
      resolveHref={resolveHref}
      openView={openView}
      historyEnabled
    >
      <HistoryContinuity />
      <BulkOutcomeRefresh />
      <Shell>{children}</Shell>
    </GenesyVoiceProvider>
  )
}

/** A voice-created run appears immediately even when the idle run list stopped polling. */
function BulkOutcomeRefresh() {
  const outcome = useVoice().outcome
  const client = useQueryClient()
  React.useEffect(() => {
    const data = outcome?.data
    if (
      data &&
      typeof data === "object" &&
      "flow_path" in data &&
      typeof data.flow_path === "string" &&
      data.flow_path.startsWith("/imports/products/")
    )
      void client.invalidateQueries({ queryKey: ["product-bulk"] })
  }, [outcome, client])
  return null
}
