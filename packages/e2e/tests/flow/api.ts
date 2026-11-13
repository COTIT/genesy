import type { Page } from "@playwright/test"

import { mockApi, type MockReply } from "../../src/api-mock"
import { sceneTable } from "./scenes-api" // F2

export const AMINA = {
  id: "u1",
  name: "Amina Tester",
  initials: "AT",
  email: "amina@genesy.test",
  permissions: [],
}
export const BARAKA = {
  id: "u2",
  name: "Baraka Second",
  initials: "BS",
  email: "baraka@genesy.test",
  permissions: [],
}
export const ILUNDO = {
  id: "w1",
  workspace_name: "ilundo-honey",
  display_name: "Ilundo Honey Co.",
}
export const RUAHA = {
  id: "w2",
  workspace_name: "ruaha",
  display_name: "Ruaha Growers",
}

/** Flow noticed with nothing to show (the priorities reply). */
export const NO_ALERTS = {
  data: { generated_at: "2026-11-11T08:00:00Z", groups: [], restricted: [] },
}

/** Flow's /v1 calls for one signed-in member; override keys per test. */
export const flowApi = (
  over: Record<string, MockReply> = {}
): Record<string, MockReply> => ({
  "GET /me": { body: { data: { user: AMINA } } },
  "GET /me/workspaces": { body: { data: [ILUNDO] } },
  "GET /workspace/w1/product-bulk-runs": {
    body: { data: { runs: [], next: null } },
  },
  "GET /workspace/w2/product-bulk-runs": {
    body: { data: { runs: [], next: null } },
  },
  // F3: every signed-in screen ranks suggestions with Flow noticed; nothing noticed by default.
  "GET /workspace/w1/attention/priorities": { body: NO_ALERTS },
  "GET /workspace/w2/attention/priorities": { body: NO_ALERTS },
  "POST /logout": { body: { message: "Logged out." } },
  "POST /login": {
    body: {
      data: {
        access_token: "e2e-token",
        token_type: "Bearer",
        expires_at: "2099-01-01T00:00:00Z",
        workspace_ids: ["w1"],
      },
    },
  },
  ...sceneTable("w1"), // F2: home reads scenes, Flow noticed and their data
  ...sceneTable("w2"), // F2
  ...over,
})

/** The assistant readiness answer the browser sees (Office's 200 / 503 shape). */
export async function readiness(page: Page, status: number, body: unknown) {
  await page.route("**/api/voice/readiness", (route) =>
    route.fulfill({
      status,
      contentType: "application/json",
      body: JSON.stringify(body),
    })
  )
}

export { mockApi }

/** The saved login token on this page's origin, or null (no entry, or a logged-out one). */
export async function savedToken(page: Page): Promise<string | null> {
  const raw = await page.evaluate(() => localStorage.getItem("genesy.session"))
  return raw ? ((JSON.parse(raw).state?.token as string | null) ?? null) : null
}

export async function logIn(page: Page, email = "e2e@genesy.test") {
  await page.getByLabel("Email").fill(email)
  await page.getByLabel("Password").fill("e2e-password-only")
  await page.getByRole("button", { name: "Log in" }).click()
}
