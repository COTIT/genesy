# Product bulk runs

Product bulk runs stage file or chat input, prepare native product changes, and execute independently of the browser after explicit review. The API supports create, update, delete and exact-key upsert. Deletions have their own approval scope.

Base path: `/v1/workspace/{workspace}/product-bulk-runs`. Every route requires a bearer token and active workspace membership. Reading runs requires `product.list` and `product.view`. A run belongs to its creator; workspace administrators can also inspect and manage it. Each native operation requires its own product permission. AI spending settings require a workspace administrator.

`PRODUCT_BULK_ENABLED` gates new runs and preparation changes. Existing runs remain readable and approved work can drain when the flag is disabled.

## Staging

`POST /` accepts `{name?, instruction?, mode?, match_key?}` and returns `{data: run}` with status 201. Modes are `create`, `update`, `delete`, `upsert`; the default is `create`. Exact match keys are `product_nr_formatted`, `reference`, `ean13`, `barcode`, `name`, `id`. The default is `product_nr_formatted`.

`GET /?after=&page_size=` returns `{data:{runs,next}}`. The cursor is a numeric timestamp supplied by the preceding response. `GET /{run}` returns the current run, counts, source progress, reviews, permissions and AI cost totals. List pages contain at most 100 runs.

`POST /{run}/sources` accepts one multipart `file`. Upload sources sequentially to add several files to the same run. Each upload queues extraction. The source is reserved in Postgres before its bytes enter private storage. Its key uses the `genesy/product-bulk/` namespace and is never returned to the client. A cancelled run cannot execute a late upload.

`POST /{run}/stage` accepts one of these inputs:

```json
{
  "revision": 1,
  "rows": [{
    "operation": "update",
    "target": {"product_id": "<uuid>"},
    "values": {"price": "12.50"},
    "included": true
  }]
}
```

```json
{"revision":1,"text":"Create a product called Rice with a price of 12.50"}
```

```json
{
  "revision": 1,
  "selection": {"category_id": "<uuid>"},
  "operation": "update",
  "changes": {"price": {"kind": "percent", "value": "5"}}
}
```

Explicit row batches contain at most 500 rows. A target can use `product_id` or an exact `key`. A row can carry `source_id`, `sheet`, `page` and `line` provenance. `values` uses native product fields; typed values belong in `custom_fields`, keyed by definition UUID.

A selection supports `ids`, `category_id`, `status`, literal substring `q`, and `custom_filters`. Filters use the native product-field array shape `[{field_id,operator,value?}]`. At least one explicit selector is required. Selection operations are `update` or `delete`. Changes support `set` and exact decimal `percent`; percentage changes use stored base values for `price`, `wholesale_price`, `supplier_wholesale_price`, `weight`, `order_unit` or `sale_unit`. A changed base value before review makes that row require correction. A set change for `custom_fields.<uuid>` writes the nested typed field value. Delete selections cannot contain changes.

Every preparation change increments `revision`. A request carrying an old revision returns 409. Staging and extraction write no products.

## Mapping and corrections

`PATCH /{run}/mapping` accepts:

```json
{
  "revision": 2,
  "source_id": "<uuid>",
  "sheet": "Catalogue",
  "header_row": 2,
  "columns": {
    "Item": "name",
    "Price": {"target":"price","transform":{"kind":"decimal","decimal_separator":","}},
    "Notes": ""
  }
}
```

Column values are a target string or `{target,transform?}`. An empty target explicitly ignores the column. Unknown source columns require a mapping decision. Canonical native headers can map directly. Typed targets use `custom_fields.<uuid>`. Reserved targets are `_operation`, `_key`, `_product_id` and `_clear`.

Each source exposes `header_rows`, keyed by worksheet name (the empty key is CSV). Flow shows the recorded header row, including zero for headerless data. If Claude is unavailable and a spreadsheet layout is unknown, exact rows remain available under letter columns. Choose the header row and mapping explicitly. PDFs still require the configured extraction provider. Their body-row positions are never treated as spreadsheet headers. PDF/text mapping changes reuse extracted rows and do not trigger paid extraction.

AI-derived monetary values require an explicit workspace `currency_id`; column mapping and unrelated row edits cannot substitute for it. This requirement survives remainder copies and source cleanup. Canonical native spreadsheet headers retain native defaults. Literal cells and unresolved semantic questions remain correctable; incomplete or truncated extraction never counts as complete. A clarification with no extractable rows remains visible on the source and requires a fresh clarified draft.

Blank cells keep existing values. `_clear` contains a comma-separated field list or JSON string array and assigns explicit nulls after mapped values. Native validation rejects clearing required fields. Rows resembling repeated or changed spreadsheet headers stay visible as unresolved candidates. Correcting their values explicitly confirms product data; exclusion removes a true repeated header from the approved set. Headerless rows remain literal. Formulas are never evaluated. Cached formula values can be imported; a mapped formula without a cached value produces a row issue. Raw formula metadata remains available for inspection.

The run's mapping object uses keys `${source_id}:${sheet ?? ""}`. Changing the header resets that source's cursor and reparses its rows. Other sources remain staged. Generation, cursor and mapping fences reject results from extraction calls started before the reset.

`GET /{run}/rows?after=&page_size=&status=&q=` returns `{data:{rows,next,total}}`, ordered by ordinal. `after` is the last returned ordinal. The maximum page size is 100 and serialized responses stay below 2 MiB. A single oversized review row returns an explicit validation error. `q` searches staged target and value text.

Rows include canonical `before` and `after` projections, requested `values`, issues, receipts and provenance. Raw previews contain the first eight cells, each limited to 256 characters; `raw_truncated` identifies a clipped preview. `GET /{run}/rows/{row}/raw?after=0&page_size=8` returns exact cells as `{data:{cells:[{column,index,value,formula}],next}}`. This endpoint returns at most eight columns per page and preserves full cell values.

`PATCH /{run}/rows/{row}` accepts `{revision,included?,operation?,target?,values?}`. A supplied values object replaces that row's staged values. Corrections preserve original raw evidence and set a manual override. Inclusion changes alone preserve the mapping behavior. Corrections trigger fresh validation.

`GET /mappings` lists saved workspace rules. `POST /mappings` accepts `{name,mapping}`; saving shared rules requires `product.update`. `GET /template` returns native schema, field descriptors, typed definitions, supported keys and limits. The UI can produce CSV/XLSX templates from this response.

Duplicate exact source keys or resolved products block every participating included row, even across files and preparation chunks. Other validation failures preserve mapped identity and values. Correct the identity or exclude competing rows, then wait for fresh validation. UUID match keys are canonicalized; other keys retain exact case and spelling. Blank explicit keys fall back to the chosen identity field.

## Guided Flow migration

Flow displays the next necessary step from persisted run facts. Preparation continues without a user decision when mappings and values are clear. Flagged rows lead to their source evidence; validated changes lead to exact approval. Paused preparation exposes recovery, and terminal runs expose recorded outcomes.

The shared voice tool `assistant_product_bulk` supports `create`, `stage`, `status`, `template`, `rows`, `raw`, `mapping`, `correct`, `review`, `cancel`, `remainder` and `resume`. Row inspection reads 20 rows per page; exact raw inspection reads eight columns. Mapping and corrections bind the current revision. A correction replaces the row's staged values, so preserve every intended field. The agent can prepare a frozen review and show its Flow confirmation card. The tool cannot approve product writes or create custom definitions/options. Unresolved meaning, custom-field choices and final approval require the user.

## Review and execution

`POST /{run}/reviews` accepts `{revision,scope:"write"|"delete"}`. All sources must finish extraction. The review captures exact included valid rows, native defaults, current material and candidate identities. The response includes its fingerprint and row count. Excluded or invalid rows are shown separately and cannot execute.

`POST /{run}/approve` accepts `{revision,review_id}`. The approving actor must hold every permission needed by that scope. Approval binds the current immutable requests for 24 hours. Once any scope is approved, the run's revision and rows cannot be edited. The other scope can still receive its separate review and approval.

The existing Postgres job rail executes at most 25 native changes per chunk and yields after about eight seconds. Before each row, it checks the approving actor's current membership and permission, approval expiry, exact candidate identities and current material. Product numbering and identity checks share the native numbering lock. A savepoint isolates row failures so later valid rows can proceed.

Native mutation, row receipt and token-fenced progress commit in the same transaction. Losing a worker lease rolls back the chunk. A retry cannot duplicate committed rows. Changes already completed remain committed when a later row fails or the user cancels.

Chunks acquire existing product locks in UUID order before processing source order. Category deletion takes the same workspace identity lock. Delete reviews capture all variants and exact stock slots; changed impact becomes a conflict. A busy stock snapshot refuses the row for fresh review.

`POST /{run}/cancel` stops remaining work. `POST /{run}/remainder` creates a fresh unapproved run containing unapplied rows. It requires an ended or stopped run and new review before execution. Copied rows retain immutable origin run, row and source metadata without linking private sources across runs.

`POST /{run}/resume` accepts `{revision}` for preapproval preparation paused by provider configuration or budget. It continues from saved extraction cursors and revalidates staged material. An unreadable source requires correction in a fresh run. Approved or expired execution grants cannot resume through this endpoint; use a remainder run.

`GET /{run}/report` streams a CSV of status, provenance, targets, issues and receipts. It reads 500 rows at a time, quotes CSV cells and neutralizes formula-leading strings. Full raw sources and private material snapshots are omitted.

## Limits and retention

| Limit | Value |
|---|---|
| Rows per run | 50,000 |
| Sources per run | 10 |
| Source bytes | 10 MiB each |
| Cells per run | 2,000,000 |
| Cell bytes | 16 KiB |
| Inflated source bytes | 128 MiB |
| Explicit stage request | 3 MiB, at most 500 rows |
| Raw columns per exact page | 8 |
| Execution approval | 24 hours |

`GET /settings` returns `{data:{per_run_cap_usd,can_edit}}`. `POST /settings` accepts `{per_run_cap_usd:"1.50"}`. The default cap is zero. USD amounts use exact decimals. Before an AI request, the worker reserves its upper cost bound. A confirmed response settles actual usage. Unknown calls retain their reservation, and a duplicate request key cannot send another call. Progress uses page or row units supplied by extraction; no synthetic completion percentage is reported.

Unapproved drafts expire seven days after creation, including during extraction. Raw cells and private source files expire seven days after the run becomes terminal. Canonical reviewed material, receipts and provenance remain for 90 days after that terminal timestamp. An unresolved AI reservation retains only minimal run identity and cost reconciliation metadata after day 90; source records, outcomes and instructions are erased. Cleanup uses the existing job rail in bounded chunks.

## Validation

Run focused correctness tests with `cargo test --test api product_bulk_runs`. The source-generation race also has a module SQLx test. The ignored Linux load gate runs 50,000 native mixed changes with typed fields, variant/stock fanout, concurrent foreground reads and a replaced worker lease:

```sh
cargo test --test api bulk_50000_native_mixed_load -- --ignored --nocapture
```

The load test prints measured staging, preparation and execution duration, chunk latency percentiles, foreground latency and pool size. A benchmark result is valid only with its machine and database configuration recorded.
