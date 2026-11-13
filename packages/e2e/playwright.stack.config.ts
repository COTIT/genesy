import { defineConfig } from "@playwright/test"
import { fileURLToPath } from "node:url"

import { viewports } from "./src/projects"

const ci = Boolean(process.env.CI)

/**
 * Full-stack specs (STACK.md): the real apps, API and a seeded Postgres, brought up by
 * `stack:up` beforehand (no webServer here, so one stack serves many runs). Specs open each
 * app by its absolute URL from `stack.urls`, so there is no baseURL.
 */
export default defineConfig({
  testDir: "tests-stack",
  metadata: {
    scannedCatalogPath: fileURLToPath(
      new URL("./fixtures/product-bulk/scanned-catalog.pdf", import.meta.url)
    ),
    scannedNoCredentials: true,
  },
  forbidOnly: ci,
  // No retries: a flaky cross-app flow is a bug to fix, not to hide.
  retries: 0,
  // One shared seed: specs follow the data rules in STACK.md, so workers can run side by side.
  workers: ci ? 2 : 3,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: ci ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: { trace: "retain-on-failure" },
  projects: [
    { name: "stack-desktop", use: viewports.desktop },
    { name: "stack-mobile", use: viewports.mobile },
  ],
})
