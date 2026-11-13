# Product bulk verification — 2026-11-14

New runs remain disabled by default. Local correctness, browser, Linux load and live synthetic OCR checks have completed. Normal production optimization, staging promotion and staging acceptance remain release gates.

## Correctness and browser checks

- 140 native product/API regressions passed after ordered locking and delete-impact changes. The final focused bulk suite passed 27 tests, including source-currency, retained-remainder and PDF mapping recovery; its 50k test is opt-in. A separate real-parser API regression passed ambiguous-header confirmation/exclusion.
- Product/parser/admission checks passed 30 tests before live verification; final bulk extraction/parser/recovery units passed 27, including the real parser process. Flow: 418 unit/component tests passed; shared migration/template/context tests passed 15, and Office tool/route/discovery/policy/catalog checks passed 144. Flow, shared voice, Office, API client and E2E TypeScript checks passed.
- OpenAPI matches generated code; clients and the voice catalog were regenerated. The catalog classifies 561 operations. Optional deployment overlays passed Compose validation.
- Flow production Webpack build and client-secret scan passed. Default Turbopack was blocked by the isolated worktree's shared dependency paths; no application configuration workaround was added.
- 22 real browser scenarios passed: desktop 11, mobile 11, zero skips. They verify native writes, separate delete approval, duplicate exclusion recovery, stale conflicts, reloads, lost responses, malformed uploads, tenant isolation, explicit clears, worksheet mapping, formula caches and honest provider pause/resume.
- An additional test verified actual Chromium 200% browser zoom through Settings and CDP. Keyboard correction and frozen review remained reachable without horizontal overflow. Pinch scale and CSS zoom remained 1.

Browser evidence: [browser-qa.md](product-bulk-evidence/browser-qa.md), [zoom metrics](product-bulk-evidence/browser-zoom.json).

## Linux 50k workload

Linux arm64 under Docker Desktop, Rust 1.99.0. The API/worker ran as UID/GID 65532 with read-only root, no capabilities, no new privileges, 1 GiB RAM/no extra swap and 1.5 CPUs. Disposable Postgres had a separate 1 GiB/2 CPU ceiling. The fixture performs 16,667 creates, 16,667 updates and 16,666 deletes, with typed fields, inheriting and independent variants, and positive stock on update targets.

| Measurement | Result |
|---|---:|
| Applied rows / unique receipts | 50,000 / 50,000 |
| Errors / conflicts | 0 / 0 |
| Staging | 53.611 s |
| Preparation | 226.294 s |
| Applying | 266.487 s |
| Complete test including fixture setup | 666.74 s |
| API/worker peak memory | 155,918,336 bytes (148.7 MiB) |
| Cgroup OOM / OOM kills | 0 / 0 |
| Foreground native GET p95 | 71.941 ms |
| Chunk p95 / maximum | 222.744 / 839.439 ms |

Independent variant prices and existing positive stock stayed unchanged; inheriting variants received native price updates. Preparation lease loss was rejected and reclaimed safely. Foreground measurements invoke native HTTP handlers in process; TCP/browser latency and foreground write throughput were not measured.

The measured source manifest is `e85b0ba62cc33631e9ac6d993ee0efddb676b7d2cc09e5d375eb7b66f1822916`. Later UUID-key normalization, duplicate-update scoping, header metadata and ambiguous-header retention changes are recorded separately and covered by focused API/browser checks. This is a captured-snapshot measurement.

The root crate used release configuration with optimization level 1 and 256 codegen units; dependencies used normal release optimization. Full optimization level 3 compilation exceeded the 3 GiB and 5 GiB build ceilings. The production optimization build is unverified.

Evidence: [report.json](product-bulk-evidence/report.json), [benchmark log](product-bulk-evidence/benchmark-final.log), [recorded source changes](product-bulk-evidence/known-source-deltas-final.diff), [later checks](product-bulk-evidence/post-benchmark-notes.md).

## Process failure and parser isolation

An isolated Linux API container received SIGKILL while the second create was blocked after the first product, native effects and receipt existed inside the uncommitted transaction. External reads after the kill found zero committed chunk effects and no checkpoint. Restart used natural lease expiry and the existing reaper. It produced exactly 60 products, default variants, stocks, events and row/review receipts, plus 420 native history rows. Product numbers and receipt targets were unique. Peak memory was 157,786,112 bytes under a 512 MiB ceiling. The test removed its container and database.

Parser checks passed 11 fixtures. Actual child limits were 256 MiB address space, 30 CPU seconds, 60-second parent wall deadline and zero core dumps. A real allocation failure killed the child; the parent survived and parsed the next valid source in 2 ms. A separate debug process test verified request EOF and malformed-source recovery.

Evidence: [process-crash.json](product-bulk-evidence/process-crash.json). The opt-in reproduction harness is `tests/manual/product_bulk_process_crash.py`.

## Remaining release gates

A live synthetic run used the approved staging Claude credential with an aggregate $3.00 cap. Five calls settled at **$0.110652** total, with no pending reservations. The seven-page raster-only PDF produced exactly seven products: names, leading-zero barcodes and decimal prices matched golden truth; descriptions matched after whitespace normalization only. Page 5 coffee included its page 6 continuation once, page 6 tea retained body row 2, and context pages produced no duplicate products.

The separate one-page fixture deliberately used ambiguous `Code` and `Price` headers. Claude requested identifier and net/gross clarification rather than inventing their meaning. Live verification also exposed a monetary safety gap: an explicit EUR price could inherit the USD workspace default. The repaired preparation guard now requires an explicit workspace currency UUID for AI-derived monetary values. Mapping, unrelated edits and retained remainder copies cannot bypass it. Canonical native spreadsheet headers keep documented defaults. Focused API and unit regressions passed; the live OCR measurement predates this repair. No products were approved or written in the live run. PDF/text mapping now preserves body row 1, source cursors and extracted rows; semantic questions retain literal data, while truncation remains rejected.

Evidence: [live-claude.json](product-bulk-evidence/live-claude.json). Both raster fixtures and golden truth are retained under `packages/e2e/fixtures/product-bulk/`. These generated cases establish a smoke result; they do not establish accuracy for arbitrary customer scans.

Run the normal production optimization build and repeat deployment-host load checks before enabling admission. The recorded workload used Docker Desktop Linux arm64 and a validation optimization profile. At this capture, no deployment or production-data access had occurred. Activation is authorized only after the staging gates pass.

After the reviewed images are promoted to staging, the native browser acceptance check runs with `GENESY_STAGING_BULK_SMOKE=1 node packages/e2e/stack/flow-product-bulk.staging.mjs`. It accepts only fixed staging origins and creates a synthetic workspace with a zero AI cap. CSV upload, independent conflict, separate deletion approval, navigation/reload, native receipts, report download and mobile layout are checked against deployed services.

The optional staging OCR phase additionally requires `GENESY_STAGING_BULK_OCR=1` and `GENESY_STAGING_BULK_OCR_CAP_USD` set to the reconciled remaining authorization (at most $2.889348 for this capture). It uses one new run and checks both raster fixtures, explicit EUR resolution, mapping recovery, native products and receipts. The recorded prior spend is $0.110652; repeating a paid phase requires reconciling every earlier actual cost and uncertain reservation first. The runner never receives the provider key.
