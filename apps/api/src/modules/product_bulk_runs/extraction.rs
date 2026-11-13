//! Source bytes and AI suggestions become bounded, provenance-carrying rows, never writes.
use super::{ExtractionStep, RawRow, Source, claude, parser};
use crate::{
    AppError,
    jobs::{kinds::Ctx, repo::Claimed},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    collections::{BTreeMap, HashSet},
    sync::OnceLock,
};
use utoipa::PartialSchema;

/// The native request schema is the only field inventory; unknown keys never become no-op writes.
pub fn is_native_target(field: &str) -> bool {
    static FIELDS: OnceLock<HashSet<String>> = OnceLock::new();
    FIELDS
        .get_or_init(|| {
            serde_json::to_value(crate::modules::products::dto::ProductRequestDoc::schema())
                .ok()
                .and_then(|v| v.get("properties").and_then(Value::as_object).cloned())
                .map(|properties| {
                    properties
                        .into_iter()
                        .map(|(key, _)| key)
                        .filter(|k| {
                            !["custom_sections", "custom_section_forms"].contains(&k.as_str())
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .contains(field)
}

async fn catalog(ctx: &Ctx, source: &Source) -> Result<(Value, HashSet<String>), AppError> {
    let schema = serde_json::to_value(crate::modules::products::dto::ProductRequestDoc::schema())
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut fields: BTreeMap<String, Value> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| {
            ![
                "custom_sections",
                "custom_section_forms",
                "meta",
                "export_data",
                "multi_set_product_ids",
            ]
            .contains(&key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for key in ["_operation", "_key", "_product_id", "_clear"] {
        fields.insert(key.into(), json!({"type":"string"}));
    }
    let definitions: Vec<Value> = sqlx::query("SELECT to_jsonb(d)-'workspace_id' AS data FROM product_field_definitions d WHERE workspace_id=$1 AND status<>'archived' ORDER BY id")
        .bind(source.workspace_id).fetch_all(&ctx.db).await?.into_iter().map(|row| row.get("data")).collect();
    for field in &definitions {
        if let Some(id) = field["id"].as_str() {
            fields.insert(format!("custom_fields.{id}"), field.clone());
        }
    }
    let mut context = json!({"fields":fields,"custom_definitions":definitions});
    // Literal queries only. IDs are tenant-bound; the model cannot fetch URLs.
    for (key, query) in [
        (
            "categories",
            "SELECT to_jsonb(t) AS data FROM (SELECT id,name FROM product_categories WHERE workspace_id=$1 AND deleted_at IS NULL ORDER BY id LIMIT 1000) t",
        ),
        (
            "currencies",
            "SELECT to_jsonb(t) AS data FROM (SELECT id,name,asset_type,default_currency,symbol_prefix,symbol_suffix FROM currencies WHERE workspace_id=$1 AND deleted_at IS NULL ORDER BY id LIMIT 1000) t",
        ),
        (
            "vat_groups",
            "SELECT to_jsonb(t) AS data FROM (SELECT id,name FROM vat_groups WHERE workspace_id=$1 AND deleted_at IS NULL ORDER BY id LIMIT 1000) t",
        ),
    ] {
        let rows: Vec<Value> = sqlx::query(query)
            .bind(source.workspace_id)
            .fetch_all(&ctx.db)
            .await?
            .into_iter()
            .map(|row| row.get("data"))
            .collect();
        context[key] = json!(rows);
    }
    Ok((context, fields.into_keys().collect()))
}
fn key(text: &str) -> String {
    text.trim().to_ascii_lowercase().replace(' ', "_")
}
fn known_header(cells: &[String], targets: &HashSet<String>) -> bool {
    let nonempty: Vec<_> = cells.iter().filter(|c| !c.trim().is_empty()).collect();
    nonempty.len() >= 2 && nonempty.iter().all(|c| targets.contains(&key(c)))
}
fn check_mapping(
    mapping: BTreeMap<String, String>,
    columns: &[String],
    targets: &HashSet<String>,
) -> Result<BTreeMap<String, String>, AppError> {
    if mapping
        .iter()
        .any(|(source, target)| !columns.contains(source) || !targets.contains(target))
    {
        return Err(AppError::Unprocessable("AI suggested an unknown source column or product field. Select the mapping explicitly.".into()));
    }
    Ok(mapping)
}
fn layout_answer(
    result: Result<claude::Answer, AppError>,
    columns: &[String],
) -> Result<claude::Answer, AppError> {
    match result {
        Err(AppError::Validation(fields)) if fields.contains_key("clarification") => {
            // Spreadsheet literals are already parsed; an uncertain layout must not discard them.
            Ok(claude::Answer {
                columns: columns.to_vec(),
                rows: vec![],
                mapping: vec![],
                header_row: 0,
                selection_json: String::new(),
                needs_input: fields["clarification"].join(" "),
                complete: false,
                truncated: false,
            })
        }
        other => other,
    }
}

pub async fn extract_source(
    ctx: &Ctx,
    source: &Source,
    _job: &Claimed,
) -> Result<ExtractionStep, AppError> {
    let store = ctx.files.as_ref().ok_or(AppError::Unavailable(
        "Product imports require private file storage.",
    ))?;
    let bytes = store
        .get(&source.object_key)
        .await?
        .ok_or(AppError::NotFound)?;
    if bytes.is_empty() || bytes.len() > super::dto::MAX_SOURCE_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let expected: String = sqlx::query_scalar(
        "SELECT sha256 FROM product_bulk_sources WHERE workspace_id=$1 AND run_id=$2 AND id=$3",
    )
    .bind(source.workspace_id)
    .bind(source.run_id)
    .bind(source.id)
    .fetch_one(&ctx.db)
    .await?;
    use sha2::Digest;
    if hex::encode(sha2::Sha256::digest(&bytes)) != expected {
        return Err(AppError::Conflict(
            "Source bytes changed. Upload the original file into a fresh draft.",
        ));
    }
    let (context, targets) = catalog(ctx, source).await?;
    if source.media_type == "text/plain" {
        let text =
            std::str::from_utf8(&bytes).map_err(|_| AppError::field("text", "Use UTF-8 text."))?;
        if text.len() > 32 * 1024 {
            return Err(AppError::PayloadTooLarge);
        }
        let task = format!(
            "Catalog: {context}\nUser request: {text}\nFor a scoped update/delete request return selection_json containing {{selection:{{ids?:UUID[],category_id?:UUID,status?:active|disabled|nla,q?:string,custom_filters?:[{{field_id,operator,value}}]}},operation:update|delete,changes:{{field:{{kind:set|percent,value:JSON}}}}}}. Use only explicitly requested filters; never imply all products. Percent values must be exact decimal strings; price is stored native price, not an offer price. If selection is ambiguous return needs_input. For an explicit list of new products return columns and literal cells instead. Include _operation and _key when supplied. Never invent UUIDs. Mapping only uses catalog targets."
        );
        let answer = claude::ask(ctx, source, &task, None).await?;
        let selection = answer.selection()?;
        if selection.is_some() && !answer.needs_input.trim().is_empty() {
            return Err(AppError::field("clarification", answer.needs_input));
        }
        let mapping = check_mapping(answer.mapping(), &answer.columns, &targets)?;
        let needs_input =
            (!answer.needs_input.trim().is_empty()).then(|| answer.needs_input.clone());
        let rows = answer
            .rows
            .into_iter()
            .map(|r| RawRow {
                source_id: Some(source.id),
                sheet: None,
                page: None,
                line: r.line,
                cells: r.cells,
                formula_columns: vec![],
                columns: answer.columns.clone(),
                header_candidate: false,
                needs_input: needs_input.clone(),
            })
            .collect();
        return Ok(ExtractionStep {
            rows,
            columns: answer.columns,
            cursor: json!({"done":true}),
            done: true,
            total: None,
            selection,
            suggested_mapping: Some(mapping),
        });
    }
    if source.media_type == "application/pdf" {
        return pdf(ctx, source, &bytes, &context, &targets).await;
    }
    let sheet_index = source.cursor["sheet_index"].as_u64().unwrap_or(0) as usize;
    let after = source.cursor["after"].as_u64().unwrap_or(0) as u32;
    let mut header = source.cursor["header_row"].as_u64().map(|v| v as u32);
    let mut parsed = parser::read(parser::Request {
        media_type: source.media_type.clone(),
        data: STANDARD.encode(&bytes),
        sheet_index,
        after,
        header_row: header,
        ..Default::default()
    })
    .await?;
    let explicit_mapping: Value =
        sqlx::query_scalar("SELECT mapping FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2")
            .bind(source.workspace_id)
            .bind(source.run_id)
            .fetch_one(&ctx.db)
            .await?;
    let mapping_key = format!("{}:{}", source.id, parsed.sheet.as_deref().unwrap_or(""));
    if let Some(explicit) = explicit_mapping
        .get(&mapping_key)
        .and_then(|v| v.get("header_row"))
        .and_then(Value::as_u64)
    {
        let explicit = u32::try_from(explicit)
            .map_err(|_| AppError::field("header_row", "Header row is outside this source."))?;
        if header != Some(explicit) {
            header = Some(explicit);
            parsed = parser::read(parser::Request {
                media_type: source.media_type.clone(),
                data: STANDARD.encode(&bytes),
                sheet_index,
                after,
                header_row: header,
                ..Default::default()
            })
            .await?;
        }
    }
    let mut columns = parsed.columns.clone();
    let mut mapping = BTreeMap::new();
    let mut needs_input = source
        .cursor
        .get("needs_input")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if header.is_none() && !parsed.rows.is_empty() {
        if known_header(&parsed.rows[0].cells, &targets) {
            header = Some(parsed.rows[0].line);
            columns = parsed.rows[0].cells.clone();
            mapping = columns
                .iter()
                .filter(|&c| targets.contains(&key(c)))
                .map(|c| (c.clone(), key(c)))
                .collect();
        } else if ctx
            .invoice_state
            .as_ref()
            .is_none_or(|s| s.anthropic_api_key.is_none())
        {
            // Without AI, retain every exact row under letter columns until the user chooses a header/mapping.
            header = Some(0);
        } else {
            let samples:Vec<Value> = parsed.rows.iter().take(25).map(|r| json!({"line":r.line,"cells":r.cells.iter().map(|c|c.chars().take(160).collect::<String>()).collect::<Vec<_>>()})).collect();
            let task = format!(
                "Identify worksheet/header and suggest mappings. Never rewrite cell values. Catalog:{context}\nSheet:{:?}\nRows:{samples:?}\nReturn header_row as exact source line (0 if headerless), columns as exact header texts, mapping source labels->catalog targets. Unknown columns stay unmapped. Return no rows. A header may follow a title/preamble. Set complete=true when layout is resolved.",
                parsed.sheet
            );
            let answer =
                layout_answer(claude::ask(ctx, source, &task, None).await, &parsed.columns)?;
            needs_input =
                (!answer.needs_input.trim().is_empty()).then(|| answer.needs_input.clone());
            if answer.header_row > 0 {
                let row = parsed
                    .rows
                    .iter()
                    .find(|r| r.line == answer.header_row)
                    .ok_or_else(|| {
                        AppError::field(
                            "header_row",
                            "Suggested header is outside the inspected rows.",
                        )
                    })?;
                header = Some(row.line);
                columns = row.cells.clone();
            } else {
                header = Some(0);
            }
            mapping = check_mapping(answer.mapping(), &columns, &targets)?;
        }
    }
    let mut header_rows = source.cursor["header_rows"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    if let Some(header) = header {
        header_rows
            .entry(parsed.sheet.clone().unwrap_or_default())
            .or_insert(json!(header));
    }
    let mut current_columns = columns.clone();
    let mut rows = Vec::new();
    for row in parsed.rows {
        if header.is_some_and(|h| row.line <= h) {
            continue;
        }
        let header_candidate = header.is_some_and(|h| h > 0)
            && (row.cells == current_columns || known_header(&row.cells, &targets));
        // Repeated tables with canonical headers can change column order within a worksheet.
        if header_candidate && known_header(&row.cells, &targets) {
            current_columns = row.cells.clone();
            header = Some(row.line);
            for c in &current_columns {
                if targets.contains(&key(c)) {
                    mapping.insert(c.clone(), key(c));
                }
            }
            // Keep a changed-header candidate visible: source values resembling field names
            // must never silently disappear from the review.
        }
        rows.push(RawRow {
            source_id: Some(source.id),
            sheet: parsed.sheet.clone(),
            page: None,
            line: row.line,
            cells: row.cells,
            formula_columns: row.formula_columns,
            columns: current_columns.clone(),
            header_candidate,
            needs_input: needs_input.clone(),
        });
    }
    let next_sheet = parsed.next_sheet;
    let changed_sheet = next_sheet != sheet_index;
    let rows_read = source.cursor["rows_read"].as_u64().unwrap_or(0) + rows.len() as u64;
    Ok(ExtractionStep {
        rows,
        columns,
        cursor: json!({"sheet_index":next_sheet,"after":if changed_sheet {0}else{parsed.after},"rows_read":rows_read,"header_rows":header_rows,"header_row":if changed_sheet{None}else{header},"needs_input":if changed_sheet{None}else{needs_input}}),
        done: parsed.done,
        total: parsed.total,
        selection: None,
        suggested_mapping: (!mapping.is_empty()).then_some(mapping),
    })
}

async fn pdf(
    ctx: &Ctx,
    source: &Source,
    bytes: &[u8],
    context: &Value,
    targets: &HashSet<String>,
) -> Result<ExtractionStep, AppError> {
    let from = source.cursor["page"].as_u64().unwrap_or(1) as u32;
    let mut count = 5;
    loop {
        let parsed = parser::read(parser::Request {
            media_type: source.media_type.clone(),
            data: STANDARD.encode(bytes),
            pdf_from: from,
            pdf_count: count,
            ..Default::default()
        })
        .await?;
        let end = from + parsed.page_count - 1;
        let task = format!(
            "Extract product table rows from original primary pages {from} through {end}, inclusive. This PDF excerpt also has adjacent context; its first page corresponds to original page {}. Emit each record once on the primary page where that record starts. Cross-page continuation belongs to starting page. Do not emit context-only rows, titles, repeated headers or totals. Set page to ORIGINAL page number, line to stable row order on page, preserve literal strings including leading zeros and decimal separators. All rows must use columns array; blank means missing. Catalog:{context}. Return exact headers+cells and mapping suggestions; leave ambiguous meaning unmapped and record needs_input while retaining every literal row. Preserve currency/net/gross/unit annotations in labels; never infer FX. If literal rows cannot all be returned set truncated=true and complete=false. Do not silently drop rows.",
            from.saturating_sub(1).max(1)
        );
        let answer = claude::ask(ctx, source, &task, parsed.pdf.as_deref()).await;
        let answer = match answer {
            Err(AppError::Unprocessable(ref message))
                if count > 1
                    && (message.contains("incomplete")
                        || message.contains("smaller")
                        || message.contains("limit")) =>
            {
                count = if count > 2 { 2 } else { 1 };
                continue;
            }
            other => other?,
        };
        let mapping = check_mapping(answer.mapping(), &answer.columns, targets)?;
        let needs_input =
            (!answer.needs_input.trim().is_empty()).then(|| answer.needs_input.clone());
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for row in answer.rows {
            if row.page < from
                || row.page > end
                || row.line == 0
                || !seen.insert((row.page, row.line))
            {
                return Err(AppError::Unprocessable("PDF rows have ambiguous page ownership or duplicate source positions. Review this document part.".into()));
            }
            rows.push(RawRow {
                source_id: Some(source.id),
                sheet: None,
                page: Some(row.page),
                line: row.line,
                cells: row.cells,
                formula_columns: vec![],
                columns: answer.columns.clone(),
                header_candidate: false,
                needs_input: needs_input.clone(),
            });
        }
        return Ok(ExtractionStep {
            rows,
            columns: answer.columns,
            cursor: json!({"page":end+1,"total_pages":parsed.total_pages}),
            done: parsed.done,
            total: None,
            selection: None,
            suggested_mapping: Some(mapping),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_like_product_values_are_not_a_header() {
        let targets = ["name", "brand", "price"]
            .map(str::to_owned)
            .into_iter()
            .collect();
        assert!(!known_header(
            &["name".into(), "brand".into(), "1".into()],
            &targets
        ));
        assert!(known_header(
            &["name".into(), "brand".into(), "price".into()],
            &targets
        ));
    }
    #[test]
    fn native_inventory_rejects_typo_no_op_changes() {
        assert!(is_native_target("price"));
        assert!(is_native_target("custom_fields"));
        assert!(!is_native_target("prcie"));
        assert!(!is_native_target("_clear"));
    }
    #[test]
    fn spreadsheet_layout_question_keeps_deterministic_literals_available() {
        let answer = layout_answer(
            Err(AppError::field(
                "clarification",
                "Choose a header and currency.",
            )),
            &["A".into(), "B".into()],
        )
        .unwrap();
        assert_eq!(answer.header_row, 0);
        assert_eq!(answer.columns, vec!["A", "B"]);
        assert_eq!(answer.needs_input, "Choose a header and currency.");
        assert!(answer.mapping.is_empty());
        assert!(
            layout_answer(
                Err(AppError::Unprocessable("AI output was incomplete.".into())),
                &[]
            )
            .is_err()
        );
    }
}
