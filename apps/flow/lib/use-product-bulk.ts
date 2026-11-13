"use client"

import { useSyncExternalStore } from "react"
import { useQuery } from "@tanstack/react-query"
import { useSession } from "@genesy/api-client/auth"
import { useUsersMe } from "@genesy/api-client/endpoints/users"
import { useRights } from "@genesy/api-client/rights"
import {
  bulkKey,
  bulkPollInterval,
  bulkRequest,
  type RunsPage,
} from "./product-bulk"

const subscribe = (notify: () => void) => {
  document.addEventListener("visibilitychange", notify)
  return () => document.removeEventListener("visibilitychange", notify)
}
export const usePageVisible = () =>
  useSyncExternalStore(
    subscribe,
    () => document.visibilityState === "visible",
    () => false
  )

export function useBulkOwner() {
  const workspace = useSession((state) => state.workspaceId) ?? ""
  const token = useSession((state) => state.token)
  const user =
    useUsersMe({ query: { enabled: Boolean(token) } }).data?.data.user.id ?? ""
  const rights = useRights()
  return {
    workspace,
    user,
    ready: Boolean(workspace && user && token),
    key: bulkKey(workspace, user),
    rights,
  }
}

export function useBulkRuns(after?: string | null) {
  const owner = useBulkOwner()
  const visible = usePageVisible()
  const query = useQuery({
    queryKey: [...owner.key, "runs", after ?? null],
    queryFn: ({ signal }) =>
      bulkRequest<RunsPage>(
        owner.workspace,
        after
          ? `?page_size=100&after=${encodeURIComponent(after)}`
          : "?page_size=100",
        "GET",
        undefined,
        signal
      ),
    enabled:
      owner.ready &&
      visible &&
      owner.rights.may("product.list") &&
      owner.rights.may("product.view"),
    refetchInterval: (query) =>
      bulkPollInterval(
        query.state.data?.runs.map((run) => run.state) ?? ["queued"],
        visible
      ),
    refetchOnWindowFocus: "always",
    retry: false,
  })
  return { ...owner, query }
}
