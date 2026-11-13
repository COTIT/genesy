"use client"

import Link from "next/link"
import { useState } from "react"
import { useRouter } from "next/navigation"
import { useTranslations } from "next-intl"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { sessionStore } from "@genesy/api-client/session"
import { Button, buttonVariants } from "@genesy/ui/components/button"
import { Input } from "@genesy/ui/components/input"
import { Textarea } from "@genesy/ui/components/textarea"
import { bulkRequest, validFiles, type BulkRun } from "@/lib/product-bulk"
import { useBulkOwner, useBulkRuns } from "@/lib/use-product-bulk"
import {
  BulkError,
  BulkNav,
  RunProgress,
  fieldClass,
  panel,
  selectClass,
} from "./shared"
import { TemplateDownloads } from "./templates"

export function ProductBulkList() {
  const owner = useBulkOwner()
  const t = useTranslations("productBulk")
  if (!owner.ready)
    return (
      <section
        className="mx-auto flex w-full max-w-5xl flex-col gap-5 pb-8"
        data-product-bulk-list
      >
        <BulkNav />
        <p role="status">{t("loading")}</p>
      </section>
    )
  return <List key={`${owner.workspace}:${owner.user}`} owner={owner} />
}
function List({
  owner: baseOwner,
}: {
  owner: ReturnType<typeof useBulkOwner>
}) {
  const [cursors, setCursors] = useState<(string | null)[]>([null])
  const owner = { ...baseOwner, ...useBulkRuns(cursors[cursors.length - 1]) }
  const t = useTranslations("productBulk")
  const router = useRouter()
  const client = useQueryClient()
  const [name, setName] = useState("")
  const [instruction, setInstruction] = useState("")
  const [mode, setMode] = useState("create")
  const [matchKey, setMatchKey] = useState("id")
  const [files, setFiles] = useState<File[]>([])
  const [uploadRun, setUploadRun] = useState<string | null>(null)
  const invalidFiles = !validFiles(files)
  const create = useMutation({
    mutationFn: async () => {
      // A failed upload leaves a real, reachable draft instead of silently creating duplicates.
      const run = await bulkRequest<BulkRun>(owner.workspace, "", "POST", {
        name: name.trim() || undefined,
        instruction: instruction.trim() || undefined,
        mode,
        match_key: matchKey,
      })
      setUploadRun(run.id)
      for (const file of files) {
        const body = new FormData()
        body.set("file", file)
        await bulkRequest(owner.workspace, `/${run.id}/sources`, "POST", body)
      }
      if (!files.length && instruction.trim())
        await bulkRequest(owner.workspace, `/${run.id}/stage`, "POST", {
          revision: run.revision,
          text: instruction.trim(),
        })
      return run
    },
    onSuccess: (run) => {
      void client.invalidateQueries({ queryKey: owner.key })
      if (sessionStore.getState().workspaceId === owner.workspace)
        router.push(`/imports/products/${run.id}`)
    },
    onError: () => void client.invalidateQueries({ queryKey: owner.key }),
  })
  return (
    <section
      className="mx-auto flex w-full max-w-5xl flex-col gap-5 pb-8"
      data-product-bulk-list
    >
      <BulkNav />
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{t("title")}</h1>
        <p className="mt-2 max-w-[65ch] text-sm text-muted-foreground">
          {t("intro")}
        </p>
      </div>
      <form
        className={panel}
        onSubmit={(event) => {
          event.preventDefault()
          create.mutate()
        }}
      >
        <h2 className="text-lg font-semibold">{t("newRun")}</h2>
        <label className={fieldClass}>
          {t("name")}
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            maxLength={255}
            className="min-h-11"
          />
        </label>
        <div className="grid gap-4 sm:grid-cols-2">
          <label className={fieldClass}>
            {t("mode")}
            <select
              className={selectClass}
              value={mode}
              onChange={(event) => setMode(event.target.value)}
            >
              {["create", "update", "upsert", "delete"].map((value) => (
                <option key={value} value={value}>
                  {t(`modes.${value}`)}
                </option>
              ))}
            </select>
          </label>
          <label className={fieldClass}>
            {t("matchKey")}
            <select
              className={selectClass}
              value={matchKey}
              onChange={(event) => setMatchKey(event.target.value)}
            >
              {[
                "id",
                "product_nr_formatted",
                "reference",
                "ean13",
                "barcode",
                "name",
              ].map((key) => (
                <option key={key} value={key}>
                  {key}
                </option>
              ))}
            </select>
          </label>
        </div>
        <label className={fieldClass}>
          {t("instruction")}
          <Textarea
            value={instruction}
            onChange={(event) => setInstruction(event.target.value)}
            maxLength={16000}
            rows={3}
          />
        </label>
        <label className={fieldClass}>
          {t("files")}
          <Input
            type="file"
            accept=".xlsx,.csv,.pdf"
            multiple
            onChange={(event) => setFiles(Array.from(event.target.files ?? []))}
            className="min-h-11"
            aria-describedby="bulk-file-help"
          />
        </label>
        <p id="bulk-file-help" className="text-sm text-muted-foreground">
          {t("fileHelp")}
        </p>
        {invalidFiles ? (
          <p role="alert" className="text-sm text-destructive">
            {t("fileInvalid")}
          </p>
        ) : null}
        <BulkError error={create.error} />
        {uploadRun ? (
          <Link
            href={`/imports/products/${uploadRun}`}
            className="min-h-11 py-3 text-sm underline"
          >
            {t("openCreatedDraft")}
          </Link>
        ) : null}
        <Button
          type="submit"
          className="min-h-11 self-start"
          disabled={
            !owner.ready ||
            create.isPending ||
            invalidFiles ||
            Boolean(uploadRun) ||
            !["product.create", "product.update", "product.delete"].some(
              owner.rights.may
            )
          }
        >
          {create.isPending ? t("creating") : t("createDraft")}
        </Button>
      </form>
      <TemplateDownloads workspace={owner.workspace} ready={owner.ready} />
      {owner.rights.may("workspace_admin") ? (
        <SpendSettings
          workspace={owner.workspace}
          ownerKey={owner.key}
          ready={owner.ready}
        />
      ) : null}
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">{t("runs")}</h2>
        <Button
          variant="outline"
          className="min-h-11"
          onClick={() => void owner.query.refetch()}
        >
          {t("refresh")}
        </Button>
      </div>
      <BulkError
        error={owner.query.error}
        retry={() => void owner.query.refetch()}
      />
      {owner.query.isPending ? <p role="status">{t("loading")}</p> : null}
      {owner.query.data?.runs.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("empty")}</p>
      ) : null}
      <ul className="grid gap-3">
        {owner.query.data?.runs.map((run) => (
          <li key={run.id} className={panel}>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h3 className="min-w-0 font-semibold break-words">
                {run.name || t("untitled")}
              </h3>
              <Link
                href={`/imports/products/${run.id}`}
                className={buttonVariants({
                  variant: "outline",
                  className: "min-h-11",
                })}
              >
                {t("open")}
              </Link>
            </div>
            <RunProgress run={run} />
          </li>
        ))}
      </ul>
      <nav className="flex flex-wrap gap-3" aria-label={t("runs")}>
        <Button
          className="min-h-11"
          variant="outline"
          disabled={cursors.length === 1 || owner.query.isFetching}
          onClick={() => setCursors(cursors.slice(0, -1))}
        >
          {t("previous")}
        </Button>
        <Button
          className="min-h-11"
          variant="outline"
          disabled={owner.query.data?.next == null || owner.query.isFetching}
          onClick={() => {
            if (owner.query.data?.next != null)
              setCursors([...cursors, String(owner.query.data.next)])
          }}
        >
          {t("next")}
        </Button>
      </nav>
    </section>
  )
}

function SpendSettings({
  workspace,
  ownerKey,
  ready,
}: {
  workspace: string
  ownerKey: readonly string[]
  ready: boolean
}) {
  const t = useTranslations("productBulk")
  const [cap, setCap] = useState("")
  const client = useQueryClient()
  const settings = useQuery({
    queryKey: [...ownerKey, "settings"],
    queryFn: ({ signal }) =>
      bulkRequest<{ per_run_cap_usd: string | null }>(
        workspace,
        "/settings",
        "GET",
        undefined,
        signal
      ),
    enabled: ready,
    retry: false,
  })
  const save = useMutation({
    mutationFn: () =>
      bulkRequest(workspace, "/settings", "POST", { per_run_cap_usd: cap }),
    onSuccess: () => {
      setCap("")
      void client.invalidateQueries({ queryKey: [...ownerKey, "settings"] })
    },
  })
  return (
    <details className={panel}>
      <summary className="min-h-11 cursor-pointer py-3 font-medium">
        {t("budget.title")}
      </summary>
      <p className="text-sm text-muted-foreground">{t("budget.help")}</p>
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault()
          save.mutate()
        }}
      >
        <label className={fieldClass}>
          {t("budget.cap")}
          <Input
            className="min-h-11"
            inputMode="decimal"
            value={cap}
            placeholder={settings.data?.per_run_cap_usd ?? ""}
            onChange={(event) => setCap(event.target.value)}
            required
            pattern="[0-9]+([.][0-9]{1,2})?"
          />
        </label>
        <Button
          type="submit"
          disabled={save.isPending || !settings.data}
          className="min-h-11"
        >
          {t("save")}
        </Button>
      </form>
      <BulkError error={settings.error || save.error} />
      {save.isSuccess ? <p role="status">{t("saved")}</p> : null}
    </details>
  )
}
