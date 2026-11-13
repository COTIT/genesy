"use client"

import Link from "next/link"
import { useTranslations } from "next-intl"
import { buttonVariants } from "@genesy/ui/components/button"
import {
  migrationGuide,
  type MigrationRun,
} from "@genesy/voice/lib/product-migration"
import { panel } from "./shared"

/** Preparation is automatic; surface the next necessary decision using saved run facts. */
export function MigrationGuidance({ run }: { run: MigrationRun }) {
  const t = useTranslations("productBulk")
  const guide = migrationGuide(run)
  const target =
    guide.step === "approval"
      ? "bulk-approval-title"
      : guide.step === "clarify"
        ? "bulk-rows-title"
        : null
  return (
    <section
      className={panel}
      aria-labelledby="bulk-guidance-title"
      data-migration-step={guide.step}
    >
      <h2 id="bulk-guidance-title" className="text-lg font-semibold">
        {t("guidance.title")}
      </h2>
      <p className="max-w-[65ch] text-sm" aria-live="polite">
        {t(`guidance.${guide.step}`)}
      </p>
      {target ? (
        <a
          href={`#${target}`}
          className={buttonVariants({
            variant: "outline",
            className: "min-h-11 self-start",
          })}
        >
          {t(
            guide.step === "approval" ? "guidance.review" : "guidance.resolve"
          )}
        </a>
      ) : null}
    </section>
  )
}

/** A frozen AI-prepared review stays in Flow and uses the same explicit approval controls. */
export function MigrationReviewHandoff({ href }: { href: string }) {
  const t = useTranslations("productBulk")
  return (
    <section className={panel} aria-label={t("approvalTitle")}>
      <p className="font-medium">{t("guidance.approval")}</p>
      <Link
        href={href}
        className={buttonVariants({ className: "min-h-11 self-start" })}
      >
        {t("guidance.review")}
      </Link>
    </section>
  )
}
