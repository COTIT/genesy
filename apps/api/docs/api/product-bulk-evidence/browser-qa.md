# Product bulk local browser QA

Test: `/private/tmp/flow-product-bulk-ui/packages/e2e/tests-stack/flow-product-bulk.spec.ts`
Config: `/private/tmp/flow-product-bulk-qa.config.ts`
API: `http://127.0.0.1:4180`; Flow: `http://localhost:3180`.
Synthetic workspaces only. Actual API requests and native product reads; no successful API mocks.

## Current status

- **Final clean sweep: 22/22 passed, zero skips, 4.7 minutes.** Desktop 11/11; mobile 11/11. Frozen final API binary and Flow source. Playwright exit 0; `.last-run.json` status passed.
- ESLint, TypeScript and Prettier checks passed for the new test file. No staging or commits.
- Duplicate keys block both participants and preserve native values; explicit exclusion then permits exactly one approved update to price 8.
- Canonical mixed create/update/delete verifies actual native product values, independent stale conflict, separate deletion approval, navigation/reload, durable receipts and downloaded report.
- Unknown-source API/UI header row 0 remains literal until explicit row 1 selection. Unknown columns require explicit ignore. Script-like cells stay inert literal data.
- Native/custom blank preservation and explicit clear, upload rejection, duplicate headers, XLSX IDs/formulas, real interrupted/lost requests, stale reviews, tenant/cache isolation, provider pause/resume, keyboard correction, guidance and CSS reflow passed both viewports.
- Additional actual browser zoom case: 1/1 passed in Chromium 156.0.8078.4 using a fresh owned persistent profile. Chromium Settings shows 200%; CDP reports page zoom 2 and pinch scale 1. No CSS zoom or device emulation applied. Keyboard source correction and frozen review remain reachable; document and main horizontal overflow checks pass.

## Evidence

- Desktop: `/private/tmp/flow-product-bulk-qa-desktop-cleared`.
- Mobile: `/private/tmp/flow-product-bulk-qa-mobile-pass`.
- Earlier desktop adversarial failures: `/private/tmp/flow-product-bulk-qa-desktop-adversarial-first`.
- Full sweep: `/private/tmp/flow-product-bulk-qa-final`.
- Focused follow-ups: `/private/tmp/flow-product-bulk-qa-final-recovery`.
- Frozen-stack recovery: `/private/tmp/flow-product-bulk-qa-steady-recovery`.
- Final clean report: `/private/tmp/flow-product-bulk-qa-clean`.
- Actual zoom proof: `/private/tmp/flow-product-bulk-qa-actual-zoom/flow-product-bulk-actual-2-1b229--and-review-keyboard-access-bulk-browser-zoom/` contains full-surface Settings/review PNGs and `actual-browser-zoom-metrics.json`.
- Scanned fixture: `/private/tmp/flow-product-bulk-qa-fixtures/scanned-catalog.pdf`; truth JSON and source/render PNG beside it. One image, zero extracted text, visually inspected.

## Confirmed failures raised and corrected by parent

- Sparse run-list projection crashed UI; optional summary groups now supported without fabricated zeros.
- Parser child retained stdin, leaving source stored forever; stdin EOF fixed.
- Async XLSX sheet metadata left empty active sheet; remount key fixed and desktop browser verified.
- Missing Claude configuration correctly paused with no fabricated rows, but resume permission missing; fixed and explicit browser resume verified on both viewports.
- Duplicate exact keys targeting same product both became ready. No approval sent. Backend run-wide duplicate check now verified to block both rows.
- Unknown spreadsheet headers required Claude before manual mapping. Backend literal A/B/C fallback and explicit header selection now verified on both viewports.
- Duplicate source headers produced repeated React keys; stable keys, disabled ambiguous controls, and visible alert verified on both viewports.

## Limits

No live Claude/OCR quality claim. Raster PDF verifies honest missing-provider pause and recovery only. Linux resource limits and 50k mixed-load performance remain outside this browser QA.

## Reproduce local run

`pnpm exec playwright test -c /private/tmp/flow-product-bulk-qa.config.ts --output=/private/tmp/flow-product-bulk-qa-clean`

Run from `/private/tmp/flow-product-bulk-ui/packages/e2e`. External config selects local servers, installed Chromium, desktop/mobile viewports and the verified scanned fixture. Repository test falls back to the existing stack URLs; the credential-free scanned case requires explicit fixture/config metadata.

Actual browser zoom: `pnpm exec playwright test -c /private/tmp/flow-product-bulk-zoom.config.ts`. The added case requires `metadata.actualBrowserZoom=true`; existing 22-case clean proof remains unchanged.

## Actual zoom facts and primary sources

- Chromium reads the default storage-partition zoom preference from `partition.default_zoom_level.x`; empty relative partition path produces key `x`. [ChromeZoomLevelPrefs source](https://raw.githubusercontent.com/chromium/chromium/main/chrome/browser/ui/zoom/chrome_zoom_level_prefs.cc).
- Zoom factor is `1.2^level`; 200% therefore uses `log(2)/log(1.2)`. [Blink page zoom source](https://raw.githubusercontent.com/chromium/chromium/main/third_party/blink/common/page/page_zoom.cc).
- CDP defines visual viewport `zoom` as page zoom factor, separately from viewport `scale`. [Official Page protocol](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Page.pdl).
- Captured metrics: browser outer width 1280, inner/layout CSS width 640, browser outer height 800, inner height 356, page zoom 2, pinch scale 1, CSS zoom 1, main client/scroll widths both 624. Full native browser surface PNG is 1280×713. Profile is deleted after the test.
