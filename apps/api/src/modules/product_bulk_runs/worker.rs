//! Durable extraction, selection expansion, preparation and bounded native application.
use super::{dto::*, service};
use crate::{
    AppError,
    jobs::{kinds::Ctx, repo::Claimed, runner::Step},
    modules::{permissions, products},
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, QueryBuilder, Row, Transaction};
use std::{
    str::FromStr,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub async fn run(ctx: &Ctx, job: &Claimed) -> Result<Step, AppError> {
    let result = run_inner(ctx, job).await;
    if let Err(error) = &result {
        let missing_provider = matches!(
            error,
            AppError::Unavailable(
                "PDF and AI mapping require ANTHROPIC_API_KEY."
                    | "Import services are not configured."
            )
        );
        if (missing_provider || error.is_permanent() || job.attempts >= job.max_attempts)
            && let (Some(workspace), Some(run)) = (
                job.workspace_id,
                job.payload
                    .get("run_id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok()),
            )
        {
            let mut tx = ctx.db.begin().await?;
            sqlx::query("UPDATE product_bulk_runs SET state=$3,issues=$4,finished_at=CASE WHEN $3='failed' THEN clock_timestamp() ELSE NULL END WHERE workspace_id=$1 AND id=$2 AND cancelled_at IS NULL").bind(workspace).bind(run).bind(if missing_provider{"paused"}else if error.is_permanent(){"needs_input"}else if job.payload["phase"]=="apply"{"failed"}else{"paused"}).bind(json!([issue(error)])).execute(&mut *tx).await?;
            fence(&mut tx, job, 0, None).await?;
            tx.commit().await?;
            return Ok(Step::Done);
        }
    }
    result
}
async fn run_inner(ctx: &Ctx, job: &Claimed) -> Result<Step, AppError> {
    let state = ctx
        .invoice_state
        .as_ref()
        .ok_or(AppError::Unavailable("Import services are not configured."))?;
    let _ = state;
    let workspace = job
        .workspace_id
        .ok_or(AppError::BadRequest("Import jobs need a workspace.".into()))?;
    let run = job
        .payload
        .get("run_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(AppError::BadRequest("Import jobs need a run ID.".into()))?;
    if service::expire_draft(&ctx.db, workspace, run).await? {
        return Ok(Step::Done);
    }
    let found=sqlx::query("SELECT state,cancelled_at,revision,selection_done,mode FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(run).fetch_optional(&ctx.db).await?;
    let Some(found) = found else {
        return Ok(Step::Done);
    };
    if found
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancelled_at")
        .is_some()
    {
        return Ok(Step::Done);
    }
    if job.payload["phase"] == "apply" {
        return apply(ctx, job, workspace, run).await;
    }
    let revision = found.get::<i64, _>("revision");
    let creator: Uuid = sqlx::query_scalar("SELECT created_by FROM product_bulk_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&ctx.db)
        .await?;
    let access = permissions::repo::access(&ctx.db, workspace, creator).await?;
    let permitted = access.is_some_and(|a| {
        a.active && {
            let grants = permissions::model::Granted::new(a.owner, a.permissions);
            let mode = found.get::<String, _>("mode");
            grants.allows("product.list")
                && grants.allows("product.view")
                && match mode.as_str() {
                    "create" => grants.allows("product.create"),
                    "update" => grants.allows("product.update"),
                    "delete" => grants.allows("product.delete"),
                    _ => grants.allows("product.create") && grants.allows("product.update"),
                }
        }
    });
    if !permitted {
        return Err(AppError::Forbidden(
            "Live creator permissions are required before extraction or preparation.",
        ));
    }
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_sources WHERE run_id=$1 AND state='pending' AND created_at>now()-interval '60 seconds')").bind(run).fetch_one(&ctx.db).await?;
    if pending {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Ok(Step::More);
    }
    sqlx::query("UPDATE product_bulk_sources SET state='error',issues='[{\"code\":\"upload_interrupted\",\"message\":\"Source upload did not complete. Start a fresh run with the original source.\"}]' WHERE run_id=$1 AND state='pending' AND created_at<=now()-interval '60 seconds'").bind(run).execute(&ctx.db).await?;
    if let Some(s) = next_source(&ctx.db, workspace, run).await? {
        sqlx::query("UPDATE product_bulk_runs SET state='extracting' WHERE workspace_id=$1 AND id=$2 AND cancelled_at IS NULL").bind(workspace).bind(run).execute(&ctx.db).await?;
        let step = super::extraction::extract_source(ctx, &s, job).await;
        return persist_extraction(ctx, job, workspace, run, revision, s, step).await;
    }
    if !found.get::<bool, _>("selection_done") {
        return expand_selection(ctx, job, workspace, run, revision).await;
    }
    prepare(ctx, job, workspace, run, revision).await
}
fn mapping_fingerprint(mapping: &Value, source: Uuid) -> String {
    let prefix = format!("{source}:");
    let relevant: serde_json::Map<String, Value> = mapping
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    hex::encode(Sha256::digest(
        Value::Object(relevant).to_string().as_bytes(),
    ))
}
async fn next_source(
    db: &sqlx::PgPool,
    workspace: Uuid,
    run: Uuid,
) -> Result<Option<Source>, AppError> {
    let row=sqlx::query("SELECT s.*,r.mapping FROM product_bulk_sources s JOIN product_bulk_runs r ON r.id=s.run_id WHERE s.workspace_id=$1 AND s.run_id=$2 AND s.state IN ('stored','extracting') ORDER BY s.created_at,s.id LIMIT 1").bind(workspace).bind(run).fetch_optional(db).await?;
    Ok(row.map(|r| {
        let id = r.get("id");
        let mut cursor: Value = r.get("cursor");
        cursor["__source_generation"] = json!(r.get::<i64, _>("generation"));
        cursor["__mapping_fingerprint"] =
            json!(mapping_fingerprint(&r.get::<Value, _>("mapping"), id));
        Source {
            id,
            workspace_id: workspace,
            run_id: run,
            object_key: r.get("object_key"),
            media_type: r.get("media_type"),
            name: r.get("name"),
            cursor,
        }
    }))
}

async fn fence(
    tx: &mut Transaction<'_, Postgres>,
    job: &Claimed,
    done: i64,
    total: Option<i64>,
) -> Result<(), AppError> {
    if !job.checkpoint(&mut **tx, &job.payload, done, total).await? {
        return Err(AppError::Conflict("Import worker lease was lost."));
    }
    Ok(())
}
async fn lock_run(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    run: Uuid,
    revision: i64,
) -> Result<bool, AppError> {
    if service::expire_locked(tx, workspace, run).await? {
        return Ok(false);
    }
    let r=sqlx::query("SELECT revision,cancelled_at FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(workspace).bind(run).fetch_optional(&mut **tx).await?.ok_or(AppError::NotFound)?;
    Ok(r.get::<i64, _>("revision") == revision
        && r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancelled_at")
            .is_none())
}
async fn persist_extraction(
    ctx: &Ctx,
    job: &Claimed,
    workspace: Uuid,
    run: Uuid,
    revision: i64,
    source: Source,
    result: Result<ExtractionStep, AppError>,
) -> Result<Step, AppError> {
    let mut tx = ctx.db.begin().await?;
    if service::expire_locked(&mut tx, workspace, run).await? {
        fence(&mut tx, job, 0, None).await?;
        tx.commit().await?;
        return Ok(Step::Done);
    }
    let current=sqlx::query("SELECT revision,cancelled_at FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(workspace).bind(run).fetch_one(&mut *tx).await?;
    if current
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancelled_at")
        .is_some()
    {
        return Ok(Step::Done);
    }
    let _ = revision;
    let stored=sqlx::query("SELECT s.cursor,s.generation,s.state,r.mapping FROM product_bulk_sources s JOIN product_bulk_runs r ON r.id=s.run_id WHERE s.id=$1 AND s.workspace_id=$2 AND s.run_id=$3 FOR UPDATE OF s").bind(source.id).bind(workspace).bind(run).fetch_one(&mut *tx).await?;
    let mut expected_cursor = source.cursor.clone();
    let generation = expected_cursor
        .as_object_mut()
        .and_then(|m| m.remove("__source_generation"))
        .and_then(|v| v.as_i64());
    let fingerprint = expected_cursor
        .as_object_mut()
        .and_then(|m| m.remove("__mapping_fingerprint"));
    if generation != Some(stored.get::<i64, _>("generation"))
        || fingerprint
            != Some(json!(mapping_fingerprint(
                &stored.get::<Value, _>("mapping"),
                source.id
            )))
        || expected_cursor != stored.get::<Value, _>("cursor")
        || !matches!(
            stored.get::<String, _>("state").as_str(),
            "stored" | "extracting"
        )
    {
        return Ok(Step::More);
    }
    let step = match result {
        Ok(v) => v,
        Err(e) => {
            if !e.is_permanent() && !matches!(e, AppError::Conflict(_)) {
                return Err(e);
            }
            sqlx::query("UPDATE product_bulk_sources SET state='error',issues=$2 WHERE id=$1")
                .bind(source.id)
                .bind(json!([issue(&e)]))
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE product_bulk_runs SET state='needs_input' WHERE id=$1")
                .bind(run)
                .execute(&mut *tx)
                .await?;
            fence(&mut tx, job, 0, None).await?;
            tx.commit().await?;
            return Ok(Step::More);
        }
    };
    if step.rows.len() > 500 {
        return Err(AppError::field(
            "source",
            "Extraction batches must not exceed 500 rows.",
        ));
    }
    let mut ordinal: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(ordinal),0) FROM product_bulk_rows WHERE run_id=$1",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    let mode: String = sqlx::query_scalar("SELECT mode FROM product_bulk_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    let mut cells = 0usize;
    for original in &step.rows {
        let mut owned_raw = original.clone();
        owned_raw.source_id = Some(source.id);
        let raw = &owned_raw;
        if raw
            .cells
            .iter()
            .any(|c| c.len() > MAX_CELL_BYTES || c.contains('\0'))
        {
            return Err(AppError::field(
                "source",
                "Each source cell must fit 16 KiB and contain no NUL.",
            ));
        }
        cells += raw.cells.iter().filter(|v| !v.trim().is_empty()).count();
        ordinal += 1;
        let staged = StagedRow {
            operation: mode.clone(),
            target: json!({}),
            values: json!({}),
            source_id: Some(source.id),
            sheet: raw.sheet.clone(),
            page: raw.page,
            line: Some(raw.line),
            included: true,
        };
        service::insert_row(&mut tx, workspace, run, ordinal, &staged, Some(raw)).await?;
        sqlx::query("UPDATE product_bulk_rows SET cell_count=$3 WHERE run_id=$1 AND ordinal=$2")
            .bind(run)
            .bind(ordinal)
            .bind(raw.cells.iter().filter(|v| !v.trim().is_empty()).count() as i32)
            .execute(&mut *tx)
            .await?;
    }
    let sheet = step
        .rows
        .first()
        .and_then(|r| r.sheet.as_deref())
        .unwrap_or("");
    sqlx::query("UPDATE product_bulk_sources SET cursor=$2,state=$3,columns=columns||jsonb_build_object($4::text,$5::jsonb) WHERE id=$1").bind(source.id).bind(&step.cursor).bind(if step.done{"done"}else{"extracting"}).bind(sheet).bind(json!(step.columns)).execute(&mut *tx).await?;
    if let Some(suggestion) = step.suggested_mapping {
        sqlx::query("UPDATE product_bulk_sources SET suggested_mapping=suggested_mapping||jsonb_build_object($2::text,$3::jsonb) WHERE id=$1").bind(source.id).bind(sheet).bind(json!(suggestion)).execute(&mut *tx).await?;
    }
    if let Some(plan) = step.selection {
        service::validate_selection(&plan)?;
        sqlx::query("UPDATE product_bulk_runs SET selection=$2,selection_done=false,selection_cursor=NULL WHERE id=$1").bind(run).bind(json!(plan)).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE product_bulk_runs SET cell_count=cell_count+$2,state='preparing',updated_at=now() WHERE id=$1 AND cell_count+$2<=2000000").bind(run).bind(cells as i64).execute(&mut *tx).await?.rows_affected().eq(&1).then_some(()).ok_or(AppError::field("source","A run supports at most two million cells."))?;
    fence(&mut tx, job, ordinal, step.total.map(|n| n as i64)).await?;
    tx.commit().await?;
    Ok(Step::More)
}
async fn expand_selection(
    ctx: &Ctx,
    job: &Claimed,
    workspace: Uuid,
    run: Uuid,
    revision: i64,
) -> Result<Step, AppError> {
    let mut tx = ctx.db.begin().await?;
    if !lock_run(&mut tx, workspace, run, revision).await? {
        fence(&mut tx, job, 0, None).await?;
        tx.commit().await?;
        return Ok(Step::More);
    }
    let r = sqlx::query(
        "SELECT selection,selection_cursor,row_count FROM product_bulk_runs WHERE id=$1",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    let plan: SelectionPlan = serde_json::from_value(r.get("selection"))
        .map_err(|_| AppError::field("selection", "Invalid selection plan."))?;
    service::validate_selection(&plan)?;
    let after: Option<Uuid> = r.get("selection_cursor");
    let mut ordinal: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(ordinal),0) FROM product_bulk_rows WHERE run_id=$1",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    let filters = plan
        .selection
        .custom_filters
        .clone()
        .unwrap_or_else(|| json!([]));
    let filters: Vec<crate::modules::product_fields::dto::FieldFilter> =
        serde_json::from_value(filters)
            .map_err(|_| AppError::field("custom_filters", "Invalid typed field filters."))?;
    let filters =
        crate::modules::product_fields::service::validate_filters(&mut tx, workspace, &filters)
            .await?;
    let mut q = QueryBuilder::<Postgres>::new(
        "SELECT p.id,to_jsonb(p) AS product FROM products p WHERE p.workspace_id=",
    );
    q.push_bind(workspace).push(" AND p.deleted_at IS NULL");
    if let Some(after) = after {
        q.push(" AND p.id>").push_bind(after);
    }
    if let Some(ids) = &plan.selection.ids {
        q.push(" AND p.id=ANY(").push_bind(ids).push(")");
    }
    if let Some(category) = plan.selection.category_id {
        q.push(" AND p.category_id=").push_bind(category);
    }
    if let Some(status) = &plan.selection.status {
        q.push(" AND p.status=").push_bind(status);
    }
    if let Some(text) = &plan.selection.q {
        q.push(" AND (p.name ILIKE ")
            .push_bind(format!(
                "%{}%",
                text.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ))
            .push(" OR p.product_nr_formatted ILIKE ")
            .push_bind(format!(
                "%{}%",
                text.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ))
            .push(")");
    }
    crate::modules::product_fields::repo::push_filters(&mut q, &filters);
    q.push(" ORDER BY p.id LIMIT 500");
    let rows = q.build().fetch_all(&mut *tx).await?;
    let mut last = after;
    for p in &rows {
        ordinal += 1;
        let product: Value = p.get("product");
        let mut values = serde_json::Map::new();
        let mut base_values = serde_json::Map::new();
        let mut error = None;
        for (field, change) in &plan.changes {
            if change.kind == "percent" {
                base_values.insert(field.clone(), product[field].clone());
            }
            let v = if change.kind == "set" {
                Ok(change.value.clone())
            } else {
                percent_value(&product[field], &change.value)
            };
            match v {
                Ok(v) => {
                    put_value(&mut values, field, v)?;
                }
                Err(e) => {
                    error = Some(e);
                }
            }
        }
        let id: Uuid = p.get("id");
        last = Some(id);
        let row = StagedRow {
            operation: plan.operation.clone(),
            target: json!({"product_id":id}),
            values: Value::Object(values),
            source_id: None,
            sheet: None,
            page: None,
            line: None,
            included: true,
        };
        service::insert_row(&mut tx, workspace, run, ordinal, &row, None).await?;
        sqlx::query(
            "UPDATE product_bulk_rows SET selection_generated=true,selection_base=$3 WHERE run_id=$1 AND ordinal=$2",
        )
        .bind(run)
        .bind(ordinal)
        .bind(Value::Object(base_values))
        .execute(&mut *tx)
        .await?;
        if let Some(e) = error {
            sqlx::query("UPDATE product_bulk_rows SET status='error',issues=$3,validated_revision=$4 WHERE run_id=$1 AND ordinal=$2").bind(run).bind(ordinal).bind(json!([issue(&e)])).bind(revision).execute(&mut *tx).await?;
        }
    }
    sqlx::query("UPDATE product_bulk_runs SET selection_cursor=$2,selection_done=$3,state='preparing' WHERE id=$1").bind(run).bind(last).bind(rows.len()<500).execute(&mut *tx).await?;
    fence(&mut tx, job, ordinal, None).await?;
    tx.commit().await?;
    Ok(Step::More)
}
pub fn percent_value(base: &Value, percent: &Value) -> Result<Value, AppError> {
    let decimal = |v: &Value| {
        v.as_str()
            .map(str::to_owned)
            .or_else(|| v.as_number().map(ToString::to_string))
            .and_then(|s| Decimal::from_str(&s).ok())
            .ok_or(AppError::field(
                "percent",
                "Percent changes require an existing exact numeric value.",
            ))
    };
    let base = decimal(base)?;
    let delta = decimal(percent)?;
    let next = base
        .checked_mul(delta)
        .and_then(|n| n.checked_div(Decimal::from(100)))
        .and_then(|n| base.checked_add(n))
        .ok_or(AppError::field(
            "percent",
            "This percentage calculation is out of range.",
        ))?;
    Ok(Value::String(next.normalize().to_string()))
}
async fn map_raw(
    conn: &mut PgConnection,
    run: Uuid,
    raw: &RawRow,
    row: &mut StagedRow,
) -> Result<(), AppError> {
    let source = raw
        .source_id
        .ok_or(AppError::field("source", "Source identity is missing."))?;
    let r=sqlx::query("SELECT r.mapping,r.match_key,s.media_type,s.columns,s.suggested_mapping FROM product_bulk_runs r JOIN product_bulk_sources s ON s.run_id=r.id WHERE r.id=$1 AND s.id=$2").bind(run).bind(source).fetch_one(&mut *conn).await?;
    let match_key: String = r.get("match_key");
    let key = format!("{source}:{}", raw.sheet.as_deref().unwrap_or(""));
    let mappings: Value = r.get("mapping");
    let source_columns: Value = r.get("columns");
    let suggestions: Value = r.get("suggested_mapping");
    let columns = if !raw.columns.is_empty() {
        raw.columns.clone()
    } else {
        serde_json::from_value(source_columns[raw.sheet.as_deref().unwrap_or("")].clone())
            .unwrap_or_default()
    };
    let mapping = mappings
        .get(&key)
        .and_then(|v| v.get("columns"))
        .or_else(|| suggestions.get(raw.sheet.as_deref().unwrap_or("")));
    let header = mappings
        .get(&key)
        .and_then(|v| v.get("header_row"))
        .and_then(Value::as_u64);
    if matches!(
        r.get::<String, _>("media_type").as_str(),
        "text/csv" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    ) && raw.page.is_none()
        && header.is_some_and(|h| u64::from(raw.line) <= h)
    {
        row.included = false;
        return Ok(());
    }
    let mut clear_fields = Vec::new();
    let mut semantic_confirmation = false;
    let mut mapping_error = None;
    let cells = raw
        .cells
        .len()
        .max(raw.formula_columns.iter().max().map_or(0, |n| n + 1));
    if cells > MAX_CELLS {
        return Err(AppError::field("source", "Invalid source column indices."));
    }
    // Retain unambiguous identity even when another mapped cell needs correction.
    let mut indices: Vec<usize> = (0..cells).collect();
    indices.sort_by_key(|index| {
        let Some(column) = columns.get(*index) else {
            return 3;
        };
        let spec = mapping.and_then(|m| m.get(column));
        let field = spec
            .and_then(Value::as_str)
            .or_else(|| spec.and_then(|s| s["target"].as_str()))
            .unwrap_or(column);
        if field == "_product_id" {
            0
        } else if field == "_key" {
            1
        } else if field == match_key {
            2
        } else {
            3
        }
    });
    for index in indices {
        let text = raw.cells.get(index).map(String::as_str).unwrap_or("");
        let formula = raw.formula_columns.contains(&index);
        if text.trim().is_empty() && !formula {
            continue;
        }
        let column = columns.get(index).ok_or(AppError::field(
            "mapping",
            "Choose column mappings for this source.",
        ))?;
        let spec = mapping.and_then(|m| m.get(column));
        let explicit = mappings
            .get(&key)
            .and_then(|v| v.get("columns"))
            .and_then(|v| v.get(column))
            .is_some();
        semantic_confirmation |= !explicit
            && raw
                .needs_input
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty());
        let field = spec
            .and_then(Value::as_str)
            .or_else(|| spec.and_then(|s| s.get("target")).and_then(Value::as_str))
            .unwrap_or(column);
        if field.is_empty() {
            continue;
        }
        if formula && text.trim().is_empty() {
            return Err(AppError::field(
                "formula",
                "This mapped formula has no cached value. Recalculate the workbook or correct this row explicitly.",
            ));
        }
        if !matches!(field, "_operation" | "_key" | "_product_id" | "_clear")
            && !field.starts_with("custom_fields.")
            && !super::extraction::is_native_target(field)
        {
            mapping_error.get_or_insert_with(|| AppError::field(
                "mapping",
                format!(
                    "Map column {column} to a supported product field, a typed field, or explicit ignore."
                ),
            ));
            continue;
        }
        if columns.iter().filter(|c| *c == column).count() > 1 {
            return Err(AppError::field(
                "mapping",
                "Duplicate column headers require a source correction.",
            ));
        }
        if field == "_clear" {
            let fields: Vec<String> = if text.trim().starts_with('[') {
                serde_json::from_str(text).map_err(|_| {
                    AppError::field(
                        "_clear",
                        "Use a comma-separated field list or a JSON string array.",
                    )
                })?
            } else {
                text.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            clear_fields.extend(fields);
            continue;
        }
        let value = if let Some(transform) = spec.and_then(|s| s.get("transform")) {
            super::parser::transform_cell(text, transform)?
        } else {
            cell_value(field, text)?
        };
        match field {
            "_operation" => row.operation = value.as_str().unwrap_or("").to_owned(),
            "_key" => row.target["key"] = value,
            "_product_id" => row.target["product_id"] = value,
            _ if field.starts_with("custom_fields.") => {
                let id = &field[14..];
                if Uuid::parse_str(id).is_err() {
                    return Err(AppError::field(
                        "mapping",
                        "Choose a valid typed field UUID.",
                    ));
                }
                if row.values.get("custom_fields").is_none() {
                    row.values["custom_fields"] = json!({});
                }
                row.values["custom_fields"][id] = value;
            }
            _ => {
                row.values[field] = value;
            }
        }
    }
    for field in clear_fields {
        put_value(row.values.as_object_mut().unwrap(), &field, Value::Null)?;
    }
    if semantic_confirmation {
        return Err(AppError::field(
            "mapping",
            format!(
                "Confirm the source column meanings by explicitly mapping or ignoring them, or correct this row. {}",
                raw.needs_input.as_deref().unwrap_or("")
            ),
        ));
    }
    if let Some(error) = mapping_error {
        return Err(error);
    }
    if raw.header_candidate {
        return Err(AppError::field(
            "header_candidate",
            "This row resembles a repeated or changed header. Confirm it as product data by correcting its values, or exclude it explicitly.",
        ));
    }
    Ok(())
}
fn put_value(
    values: &mut serde_json::Map<String, Value>,
    field: &str,
    value: Value,
) -> Result<(), AppError> {
    if let Some(id) = field.strip_prefix("custom_fields.") {
        Uuid::parse_str(id)
            .map_err(|_| AppError::field("custom_fields", "Choose a typed field UUID."))?;
        let custom = values.entry("custom_fields").or_insert_with(|| json!({}));
        custom
            .as_object_mut()
            .ok_or(AppError::field(
                "custom_fields",
                "Use typed field values by UUID.",
            ))?
            .insert(id.into(), value);
    } else {
        if !super::extraction::is_native_target(field) {
            return Err(AppError::field(
                "values",
                format!("Unsupported product field: {field}."),
            ));
        }
        values.insert(field.into(), value);
    }
    Ok(())
}
fn validate_targets(values: &Value) -> Result<(), AppError> {
    let object = values
        .as_object()
        .ok_or(AppError::field("values", "Use product field values."))?;
    for (field, value) in object {
        if field == "custom_fields" {
            let custom = value.as_object().ok_or(AppError::field(
                "custom_fields",
                "Use typed field values by UUID.",
            ))?;
            for id in custom.keys() {
                Uuid::parse_str(id)
                    .map_err(|_| AppError::field("custom_fields", "Choose a typed field UUID."))?;
            }
        } else if !super::extraction::is_native_target(field) {
            return Err(AppError::field(
                "values",
                format!("Unsupported product field: {field}."),
            ));
        }
    }
    Ok(())
}
fn cell_value(field: &str, text: &str) -> Result<Value, AppError> {
    if [
        "has_stock",
        "is_serialized",
        "export_enabled",
        "store_only",
        "exclude_price_corrections",
        "export_price_sync",
        "export_product_data",
    ]
    .contains(&field)
    {
        return match text.trim() {
            "true" | "1" => Ok(json!(true)),
            "false" | "0" => Ok(json!(false)),
            _ => Err(AppError::field(
                field,
                "Map an explicit boolean value or correct this cell.",
            )),
        };
    }
    Ok(Value::String(text.to_owned()))
}
async fn staged_row(
    tx: &mut Transaction<'_, Postgres>,
    run: Uuid,
    r: &sqlx::postgres::PgRow,
    mode: &str,
) -> (StagedRow, Option<AppError>) {
    let included: bool = r.get("included");
    let raw: Option<Value> = r.get("raw");
    let mut row = StagedRow {
        operation: r.get("operation"),
        target: r.get("target"),
        values: r.get("values"),
        source_id: r.get("source_id"),
        sheet: r.get("sheet"),
        page: r.get::<Option<i32>, _>("page").map(|v| v as u32),
        line: r.get::<Option<i32>, _>("line").map(|v| v as u32),
        included,
    };
    let mut error = None;
    if let Some(raw) = raw.filter(|_| included) {
        match serde_json::from_value::<RawRow>(raw) {
            Ok(raw) => {
                if !r.get::<bool, _>("manual_override") {
                    row.operation = mode.into();
                    row.target = json!({});
                    row.values = json!({});
                    error = map_raw(tx, run, &raw, &mut row).await.err();
                }
                if row.included
                    && let Err(currency) = source_currency(tx, run, &raw, &row).await
                {
                    error = Some(currency);
                }
            }
            Err(_) => error = Some(AppError::field("row", "Invalid source row.")),
        }
    } else if included
        && !r.get::<Value, _>("provenance")["source_id"].is_null()
        && !unresolved_source_money(&row).is_empty()
    {
        error = Some(AppError::field("currency_id", SOURCE_CURRENCY_MESSAGE));
    }
    if let Some(id) = row.target["product_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        row.target["product_id"] = json!(id);
    }
    (row, error)
}
const SOURCE_CURRENCY_MESSAGE: &str = "Set an explicit workspace currency_id for source-derived monetary values; column mapping or unrelated row edits do not confirm currency. No FX conversion is inferred.";
fn unresolved_source_money(row: &StagedRow) -> Vec<&'static str> {
    if row
        .values
        .get("currency_id")
        .and_then(Value::as_str)
        .is_some_and(|s| Uuid::parse_str(s).is_ok())
    {
        return Vec::new();
    }
    [
        "price",
        "gross_price",
        "wholesale_price",
        "supplier_wholesale_price",
    ]
    .into_iter()
    .filter(|f| row.values.get(*f).is_some_and(|v| !v.is_null()))
    .collect()
}
async fn source_currency(
    conn: &mut PgConnection,
    run: Uuid,
    raw: &RawRow,
    row: &StagedRow,
) -> Result<(), AppError> {
    let money = unresolved_source_money(row);
    if money.is_empty() {
        return Ok(());
    }
    let source = raw
        .source_id
        .ok_or(AppError::field("source", "Source identity is missing."))?;
    let meta=sqlx::query("SELECT s.media_type,s.columns,EXISTS(SELECT 1 FROM product_bulk_ai_costs a WHERE a.workspace_id=s.workspace_id AND a.run_id=s.run_id AND a.source_id=s.id) AS ai_assisted FROM product_bulk_sources s WHERE s.run_id=$1 AND s.id=$2")
        .bind(run).bind(source).fetch_one(conn).await?;
    let stored: Value = meta.get("columns");
    let columns: Vec<String> = if raw.columns.is_empty() {
        serde_json::from_value(stored[raw.sheet.as_deref().unwrap_or("")].clone())
            .unwrap_or_default()
    } else {
        raw.columns.clone()
    };
    if meta.get::<bool, _>("ai_assisted")
        || matches!(
            meta.get::<String, _>("media_type").as_str(),
            "application/pdf" | "text/plain"
        )
        || money.iter().any(|f| !columns.iter().any(|c| c == f))
    {
        return Err(AppError::field("currency_id", SOURCE_CURRENCY_MESSAGE));
    }
    Ok(())
}
fn bundle_ids(values: &Value, ids: &mut Vec<Uuid>) {
    if let Some(children) = values["multi_set_product_ids"].as_array() {
        ids.extend(
            children
                .iter()
                .take(100)
                .filter_map(|v| v["id"].as_str().and_then(|s| Uuid::parse_str(s).ok())),
        );
    }
}
async fn prepare(
    ctx: &Ctx,
    job: &Claimed,
    workspace: Uuid,
    run: Uuid,
    revision: i64,
) -> Result<Step, AppError> {
    let mut tx = ctx.db.begin().await?;
    if !lock_run(&mut tx, workspace, run, revision).await? {
        fence(&mut tx, job, 0, None).await?;
        tx.commit().await?;
        return Ok(Step::More);
    }
    let creator: Uuid = sqlx::query_scalar("SELECT created_by FROM product_bulk_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    let live = permissions::repo::access(&mut *tx, workspace, creator).await?;
    if live.as_ref().is_none_or(|a| !a.active) {
        sqlx::query("UPDATE product_bulk_runs SET state='needs_input' WHERE id=$1")
            .bind(run)
            .execute(&mut *tx)
            .await?;
        fence(&mut tx, job, 0, None).await?;
        tx.commit().await?;
        return Ok(Step::Done);
    }
    let live = live.unwrap();
    let grants = permissions::model::Granted::new(live.owner, live.permissions);
    products::service::lock_identity(&mut tx, workspace).await?;
    let meta = sqlx::query("SELECT mode,match_key,row_count FROM product_bulk_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    let mode: String = meta.get("mode");
    let match_key: String = meta.get("match_key");
    let rows=sqlx::query("SELECT * FROM product_bulk_rows WHERE run_id=$1 AND validated_revision IS DISTINCT FROM $2 AND status<>'applied' ORDER BY ordinal LIMIT 25 FOR UPDATE").bind(run).bind(revision).fetch_all(&mut *tx).await?;
    // Match under identity serialization, then lock in native product-ID order, never CSV order.
    let mut mapped = Vec::with_capacity(rows.len());
    let mut product_ids = Vec::new();
    for r in &rows {
        sqlx::query("SAVEPOINT bulk_match")
            .execute(&mut *tx)
            .await?;
        let (mut row, mut error) = staged_row(&mut tx, run, r, &mode).await;
        if match_key == "id" {
            for (container, field) in [(&mut row.target, "key"), (&mut row.values, "id")] {
                if let Some(value) = container.get_mut(field)
                    && let Some(id) = value.as_str().and_then(|s| Uuid::parse_str(s).ok())
                {
                    *value = json!(id);
                }
            }
        }
        let mut resolved = None;
        let matched = async {
            if r.get::<bool, _>("included") && row.included {
                let candidates = service::resolve_unlocked(
                    &mut tx,
                    workspace,
                    &match_key,
                    &row.target,
                    &row.values,
                )
                .await?;
                if candidates.len() == 1 {
                    resolved = candidates.first().copied();
                }
                product_ids.extend(candidates);
                bundle_ids(&row.values, &mut product_ids);
            }
            Ok::<_, AppError>(())
        }
        .await;
        if let Err(e) = matched
            && error.is_none()
        {
            error = Some(e);
        }
        if error.is_some() {
            sqlx::query("ROLLBACK TO SAVEPOINT bulk_match")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("RELEASE SAVEPOINT bulk_match")
            .execute(&mut *tx)
            .await?;
        mapped.push((row, error, resolved));
    }
    service::lock_products(&mut tx, workspace, &product_ids).await?;
    let started = Instant::now();
    let mut done = 0i64;
    for (r, (mapped, mapping_error, matched_id)) in rows.iter().zip(mapped) {
        sqlx::query("SAVEPOINT bulk_row").execute(&mut *tx).await?;
        let id: Uuid = r.get("id");
        let included: bool = r.get("included");
        let result: Result<(StagedRow, Option<Uuid>, Value), AppError> = async {
            let mut row = mapped.clone();
            if !included || !row.included {
                return Ok((row, None, json!({})));
            }
            if let Some(error) = mapping_error { return Err(error); }
            validate_targets(&row.values)?;
            let candidates =
                service::resolve(&mut tx, workspace, &match_key, &row.target, &row.values).await?;
            if candidates.len() > 1 {
                return Err(AppError::field(
                    "target",
                    "Several products match this exact key. Choose one product ID.",
                ));
            }
            let resolved = candidates.first().copied();
            if row.operation == "upsert" {
                row.operation = if resolved.is_some() {
                    "update"
                } else {
                    "create"
                }
                .into();
            }
            if !matches!(row.operation.as_str(), "create" | "update" | "delete") {
                return Err(AppError::field(
                    "operation",
                    "Choose create, update or delete.",
                ));
            }
            if row.operation == "create" && resolved.is_some() {
                return Err(AppError::field(
                    "target",
                    "This create row already matches an existing product.",
                ));
            }
            if row.operation != "create" && resolved.is_none() {
                return Err(AppError::field(
                    "target",
                    "No live product matches this exact target.",
                ));
            }
            let permission = match row.operation.as_str() {
                "create" => "product.create",
                "update" => "product.update",
                _ => "product.delete",
            };
            if !grants.allows(permission) {
                return Err(AppError::Forbidden(
                    "Product permission is required for preparation.",
                ));
            }
            let selection_base: Option<Value> = r.get("selection_base");
            if !r.get::<bool, _>("manual_override")
                && let (Some(base), Some(product)) = (selection_base, resolved)
                && let Some(fields) = base.as_object()
                && !fields.is_empty()
            {
                let current: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(p) FROM products p WHERE workspace_id=$1 AND id=$2",
                )
                .bind(workspace)
                .bind(product)
                .fetch_one(&mut *tx)
                .await?;
                for (key, value) in fields {
                    if current[key] != *value {
                        return Err(AppError::Conflict(
                            "A selected base value changed before review. Stage the selection again to recompute its percentage.",
                        ));
                    }
                }
            }
            let material = products::service::material_in(
                &mut tx,
                workspace,
                &row.operation,
                resolved,
                &row.values,
            )
            .await?;
            Ok((
                row,
                resolved,
                json!({"native":material,"candidates":candidates,"match_key":match_key}),
            ))
        }
        .await;
        match result {
            Ok((row, resolved, material)) => {
                sqlx::query("RELEASE SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE product_bulk_rows SET operation=$2,target=$3,values=$4,included=$5,resolved_id=$6,material=$7,status=$8,issues='[]',validated_revision=$9 WHERE id=$1").bind(id).bind(&row.operation).bind(row.target).bind(row.values).bind(row.included&&included).bind(resolved).bind(material).bind(if row.included&&included{"ready"}else{"excluded"}).bind(revision).execute(&mut *tx).await?;
            }
            Err(e) => {
                sqlx::query("ROLLBACK TO SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE product_bulk_rows SET status='error',issues=$2,validated_revision=$3,operation=$4,target=$5,values=$6,resolved_id=$7,material=NULL WHERE id=$1").bind(id).bind(json!([issue(&e)])).bind(revision).bind(&mapped.operation).bind(&mapped.target).bind(&mapped.values).bind(matched_id).execute(&mut *tx).await?;
            }
        }
        done += 1;
        if started.elapsed() >= Duration::from_secs(8) {
            break;
        }
    }
    let left:i64=sqlx::query_scalar("SELECT count(*) FROM product_bulk_rows WHERE run_id=$1 AND validated_revision IS DISTINCT FROM $2 AND status<>'applied'").bind(run).bind(revision).fetch_one(&mut *tx).await?;
    let source_errors: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM product_bulk_sources WHERE run_id=$1 AND state='error')",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    if left == 0 && !source_errors {
        let sources_done: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM product_bulk_sources WHERE run_id=$1 AND state<>'done')")
            .bind(run).fetch_one(&mut *tx).await?;
        if sources_done {
            service::reject_duplicates(&mut tx, run, revision, &match_key).await?;
        }
    }
    sqlx::query("UPDATE product_bulk_runs SET state=$2,updated_at=now() WHERE id=$1")
        .bind(run)
        .bind(if left > 0 {
            "preparing"
        } else if source_errors {
            "needs_input"
        } else {
            "ready"
        })
        .execute(&mut *tx)
        .await?;
    fence(&mut tx, job, done, Some(meta.get("row_count"))).await?;
    tx.commit().await?;
    Ok(if left > 0 { Step::More } else { Step::Done })
}
pub fn issue(error: &AppError) -> Value {
    match error {
        AppError::Validation(v) if v.contains_key("clarification") => {
            json!({"code":"clarification","message":v["clarification"].first().cloned().unwrap_or_default(),"fields":v})
        }
        AppError::Validation(v) if v.contains_key("header_candidate") => {
            json!({"code":"header_candidate","message":"Confirm this header-looking row as product data by correcting its values, or exclude it explicitly.","fields":v})
        }
        AppError::Validation(v) => {
            json!({"code":"validation","message":"Correct this row before reviewing.","fields":v})
        }
        AppError::NotFound => {
            json!({"code":"not_found","message":"A referenced product or field no longer exists."})
        }
        AppError::Forbidden(_) => {
            json!({"code":"permission","message":"The approving actor no longer has permission."})
        }
        AppError::Conflict(m) if m.contains("spending limit") => {
            json!({"code":"budget","message":m})
        }
        AppError::Conflict(m) if m.contains("reservation") => {
            json!({"code":"unknown_reservation","message":m})
        }
        AppError::Conflict(m) => json!({"code":"stale","message":m}),
        AppError::Unavailable(_) => {
            json!({"code":"provider","message":"Extraction is paused. Check provider configuration and available budget before resuming."})
        }
        AppError::BadRequest(m) => json!({"code":"invalid","message":m}),
        AppError::Unprocessable(m) => {
            json!({"code":"source_unreadable","message":m.chars().take(512).collect::<String>()})
        }
        _ => {
            json!({"code":"failed","message":"This row could not be completed. Check its input and retry through a fresh review."})
        }
    }
}
/// Drafts expire in seven days; raw input lasts seven terminal days, outcomes ninety.
pub async fn prune(
    db: &sqlx::PgPool,
    store: Option<&crate::storage::Store>,
) -> Result<u64, AppError> {
    let mut cleaned = 0;
    let drafts=sqlx::query("SELECT r.id,r.workspace_id FROM product_bulk_runs r WHERE r.created_at<=clock_timestamp()-interval '7 days' AND r.state NOT IN ('completed','cancelled','failed','expired') AND NOT EXISTS(SELECT 1 FROM product_bulk_reviews v WHERE v.run_id=r.id AND v.approved_at IS NOT NULL) ORDER BY r.created_at,r.id LIMIT 5").fetch_all(db).await?;
    for draft in drafts {
        if service::expire_draft(db, draft.get("workspace_id"), draft.get("id")).await? {
            cleaned += 1;
        }
    }

    let interrupted=sqlx::query("SELECT r.id,r.workspace_id FROM product_bulk_runs r WHERE r.state IN ('queued','running','extracting','preparing') AND EXISTS(SELECT 1 FROM jobs j WHERE j.workspace_id=r.workspace_id AND j.kind=$1 AND j.status='failed' AND j.payload->>'run_id'=r.id::text) AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.workspace_id=r.workspace_id AND j.kind=$1 AND j.status IN ('queued','running') AND j.payload->>'run_id'=r.id::text) ORDER BY r.updated_at,r.id LIMIT 5").bind(crate::jobs::kinds::PRODUCT_BULK).fetch_all(db).await?;
    for candidate in interrupted {
        let run: Uuid = candidate.get("id");
        let workspace: Uuid = candidate.get("workspace_id");
        let mut tx = db.begin().await?;
        let owned=sqlx::query("SELECT state FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE SKIP LOCKED").bind(workspace).bind(run).fetch_optional(&mut *tx).await?;
        let Some(owned) = owned else {
            continue;
        };
        if !matches!(
            owned.get::<String, _>("state").as_str(),
            "queued" | "running" | "extracting" | "preparing"
        ) {
            continue;
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE workspace_id=$1 AND kind=$2 AND status IN ('queued','running') AND payload->>'run_id'=$3)").bind(workspace).bind(crate::jobs::kinds::PRODUCT_BULK).bind(run.to_string()).fetch_one(&mut *tx).await?;
        if active {
            continue;
        }
        let approved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_reviews WHERE run_id=$1 AND approved_at IS NOT NULL)").bind(run).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE product_bulk_runs SET state=$3,issues=$4,updated_at=now(),finished_at=CASE WHEN $3='failed' THEN clock_timestamp() ELSE NULL END WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(run).bind(if approved{"failed"}else{"paused"}).bind(json!([{"code":"worker_interrupted","message":if approved{"The background worker exhausted its recovery attempts. Check recorded receipts and prepare a fresh remainder review."}else{"Preparation stopped after repeated worker interruptions. Resume to continue from its saved cursor."}}])).execute(&mut *tx).await?;
        tx.commit().await?;
        cleaned += 1;
    }
    if let Some(store) = store {
        let rows=sqlx::query("SELECT s.id,s.object_key FROM product_bulk_sources s JOIN product_bulk_runs r ON r.id=s.run_id WHERE r.finished_at<=clock_timestamp()-interval '7 days' AND r.state IN ('completed','cancelled','failed','expired') AND s.state<>'cleaned' ORDER BY s.created_at LIMIT 100").fetch_all(db).await?;
        for r in rows {
            store.delete(r.get("object_key")).await?;
            sqlx::query("UPDATE product_bulk_sources SET state='cleaned',cursor='{}',columns='{}',suggested_mapping='{}' WHERE id=$1")
                .bind(r.get::<Uuid, _>("id"))
                .execute(db)
                .await?;
            cleaned += 1;
        }
    }
    cleaned+=sqlx::query("WITH due AS (SELECT x.id FROM product_bulk_rows x JOIN product_bulk_runs r ON r.id=x.run_id WHERE x.raw IS NOT NULL AND r.finished_at<=clock_timestamp()-interval '7 days' AND r.state IN ('completed','cancelled','failed','expired') ORDER BY r.finished_at,x.id LIMIT 500 FOR UPDATE OF x SKIP LOCKED) UPDATE product_bulk_rows x SET raw=NULL FROM due WHERE x.id=due.id").execute(db).await?.rows_affected();
    let due=sqlx::query("SELECT r.id,r.workspace_id FROM product_bulk_runs r WHERE r.finished_at<=clock_timestamp()-interval '90 days' AND r.state IN ('completed','cancelled','failed','expired') AND NOT EXISTS(SELECT 1 FROM product_bulk_sources s WHERE s.run_id=r.id AND s.state<>'cleaned') AND (NOT r.issues @> '[{\"code\":\"outcomes_expired\"}]'::jsonb OR NOT EXISTS(SELECT 1 FROM product_bulk_ai_costs c WHERE c.run_id=r.id AND c.state='reserved')) ORDER BY r.finished_at,r.id LIMIT 1").fetch_optional(db).await?;
    if let Some(due) = due {
        let run: Uuid = due.get("id");
        let workspace: Uuid = due.get("workspace_id");
        let mut tx = db.begin().await?;
        let locked=sqlx::query("SELECT id FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE SKIP LOCKED").bind(workspace).bind(run).fetch_optional(&mut *tx).await?;
        if locked.is_some() {
            let reserved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_ai_costs WHERE workspace_id=$1 AND run_id=$2 AND state='reserved')").bind(workspace).bind(run).fetch_one(&mut *tx).await?;
            sqlx::query("DELETE FROM product_bulk_reviews WHERE run_id=$1")
                .bind(run)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM product_bulk_rows WHERE run_id=$1")
                .bind(run)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM product_bulk_sources WHERE run_id=$1")
                .bind(run)
                .execute(&mut *tx)
                .await?;
            if reserved {
                sqlx::query("UPDATE product_bulk_runs SET name='Expired product bulk run',instruction='',selection=NULL,mapping='{}',row_count=0,cell_count=0,selection_cursor=NULL,selection_done=true,prepare_cursor=0,state='expired',cancelled_at=coalesce(cancelled_at,finished_at),issues='[{\"code\":\"outcomes_expired\",\"message\":\"Outcome retention expired; minimal AI cost reconciliation metadata remains.\"}]' WHERE id=$1").bind(run).execute(&mut *tx).await?;
            } else {
                sqlx::query("DELETE FROM product_bulk_runs WHERE id=$1")
                    .bind(run)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            cleaned += 1;
        }
    }
    Ok(cleaned)
}
async fn apply(ctx: &Ctx, job: &Claimed, workspace: Uuid, run: Uuid) -> Result<Step, AppError> {
    let mut tx = ctx.db.begin().await?;
    let r=sqlx::query("SELECT revision,cancelled_at FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(workspace).bind(run).fetch_one(&mut *tx).await?;
    if r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancelled_at")
        .is_some()
    {
        return Ok(Step::Done);
    }
    let revision: i64 = r.get("revision");
    products::service::lock_identity(&mut tx, workspace).await?;
    sqlx::query("UPDATE product_bulk_runs SET state='running' WHERE id=$1")
        .bind(run)
        .execute(&mut *tx)
        .await?;
    let rows=sqlx::query("SELECT x.*,v.id AS review,v.approved_by,v.expires_at,v.revision FROM product_bulk_review_rows x JOIN product_bulk_reviews v ON v.id=x.review_id WHERE v.run_id=$1 AND v.state='approved' AND x.state='pending' ORDER BY x.ordinal LIMIT 25 FOR UPDATE OF x,v").bind(run).fetch_all(&mut *tx).await?;
    let mut product_ids = Vec::new();
    for r in &rows {
        let request: Value = r.get("request");
        let material: Value = r.get("material");
        sqlx::query("SAVEPOINT bulk_match")
            .execute(&mut *tx)
            .await?;
        match service::resolve_unlocked(
            &mut tx,
            workspace,
            material["match_key"]
                .as_str()
                .unwrap_or("product_nr_formatted"),
            &request["target"],
            &request["values"],
        )
        .await
        {
            Ok(ids) => product_ids.extend(ids),
            Err(_) => {
                sqlx::query("ROLLBACK TO SAVEPOINT bulk_match")
                    .execute(&mut *tx)
                    .await?;
            }
        }
        sqlx::query("RELEASE SAVEPOINT bulk_match")
            .execute(&mut *tx)
            .await?;
        if let Some(id) = request["resolved_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            product_ids.push(id);
        }
        bundle_ids(&request["values"], &mut product_ids);
    }
    service::lock_products(&mut tx, workspace, &product_ids).await?;
    let started = Instant::now();
    let mut n = 0i64;
    for r in &rows {
        let request: Value = r.get("request");
        let material: Value = r.get("material");
        let row: Uuid = r.get("row_id");
        let review: Uuid = r.get("review");
        let actor: Option<Uuid> = r.get("approved_by");
        sqlx::query("SAVEPOINT bulk_row").execute(&mut *tx).await?;
        let result:Result<Value,AppError>=async{
            if r.get::<i64,_>("revision")!=revision||r.get::<chrono::DateTime<chrono::Utc>,_>("expires_at")<=chrono::Utc::now(){return Err(AppError::Conflict("This exact approval expired or changed. Prepare a new review."));}
            let actor=actor.ok_or(AppError::Forbidden("Approving actor is unavailable."))?;
            let access=permissions::repo::access(&mut *tx,workspace,actor).await?.ok_or(AppError::Forbidden("Approving membership ended."))?;
            let grants=permissions::model::Granted::new(access.owner,access.permissions);
            let operation=request["operation"].as_str().unwrap_or("");let permission=match operation{"create"=>"product.create","update"=>"product.update","delete"=>"product.delete",_=>return Err(AppError::field("operation","Unknown reviewed operation."))};
            if !access.active||!grants.allows(permission){return Err(AppError::Forbidden("Approving permission ended."));}
            let key=material["match_key"].as_str().unwrap_or("product_nr_formatted");
            let candidates=service::resolve(&mut tx,workspace,key,&request["target"],&request["values"]).await?;
            if json!(candidates)!=material["candidates"]{return Err(AppError::Conflict("Exact product candidates changed after review."));}
            let id=request["resolved_id"].as_str().map(Uuid::parse_str).transpose().map_err(|_|AppError::Conflict("Reviewed target changed."))?;
            let current=products::service::material_in(&mut tx,workspace,operation,id,&request["values"]).await?;
            if current!=material["native"]{return Err(AppError::Conflict("Product, variants, dependencies or field definitions changed after review."));}
            let actor=products::service::Actor{user_id:actor,flow:"manual"};
            let product=match operation{
                "create"=>products::service::create_in(&mut tx,workspace,products::dto::decode_values(request["values"].clone(),true)?,actor).await?,
                "update"=>{let id=id.ok_or(AppError::NotFound)?;products::service::update_in(&mut tx,workspace,id,products::dto::decode_values(request["values"].clone(),false)?,actor).await?;id},
                "delete"=>{let id=id.ok_or(AppError::NotFound)?;products::service::delete_in(&mut tx,workspace,id).await?;id},_=>unreachable!()
            };
            Ok(json!({"status":"completed","product_id":product,"operation":operation,"approved_by":actor.user_id,"review_id":review,"completed_at":chrono::Utc::now()}))
        }.await;
        let (status, receipt, issues) = match result {
            Ok(receipt) => {
                sqlx::query("RELEASE SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                ("applied", receipt, json!([]))
            }
            Err(e) => {
                sqlx::query("ROLLBACK TO SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT bulk_row")
                    .execute(&mut *tx)
                    .await?;
                let status = if matches!(e, AppError::Conflict(_)) {
                    "stale"
                } else {
                    "error"
                };
                (
                    status,
                    json!({"status":status,"issue":issue(&e),"review_id":review}),
                    json!([issue(&e)]),
                )
            }
        };
        sqlx::query("UPDATE product_bulk_review_rows SET state=$3,receipt=$4 WHERE review_id=$1 AND row_id=$2").bind(review).bind(row).bind(status).bind(&receipt).execute(&mut *tx).await?;
        sqlx::query("UPDATE product_bulk_rows SET status=$2,receipt=$3,issues=$4 WHERE id=$1")
            .bind(row)
            .bind(status)
            .bind(receipt)
            .bind(issues)
            .execute(&mut *tx)
            .await?;
        n += 1;
        if started.elapsed() >= Duration::from_secs(8) {
            break;
        }
    }
    let left:i64=sqlx::query_scalar("SELECT count(*) FROM product_bulk_review_rows x JOIN product_bulk_reviews v ON v.id=x.review_id WHERE v.run_id=$1 AND v.state='approved' AND x.state='pending'").bind(run).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE product_bulk_reviews v SET state='completed' WHERE run_id=$1 AND state='approved' AND NOT EXISTS(SELECT 1 FROM product_bulk_review_rows x WHERE x.review_id=v.id AND x.state='pending')").bind(run).execute(&mut *tx).await?;
    if left == 0 {
        sqlx::query("UPDATE product_bulk_runs SET state=CASE WHEN EXISTS(SELECT 1 FROM product_bulk_rows WHERE run_id=$1 AND included AND status='ready') THEN 'ready' ELSE 'completed' END,finished_at=CASE WHEN EXISTS(SELECT 1 FROM product_bulk_rows WHERE run_id=$1 AND included AND status='ready') THEN NULL ELSE clock_timestamp() END,updated_at=now() WHERE id=$1").bind(run).execute(&mut *tx).await?;
    }
    fence(&mut tx, job, n, None).await?;
    tx.commit().await?;
    Ok(if left > 0 { Step::More } else { Step::Done })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test]
    async fn product_bulk_prune_drains_expired_raw_in_bounded_continuations(db: sqlx::PgPool) {
        let user:Uuid=sqlx::query_scalar("INSERT INTO users(email,first_name,last_name,password_hash) VALUES('raw-drain@test.invalid','Raw','Drain','unused') RETURNING id").fetch_one(&db).await.unwrap();
        let workspace: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces(name,created_by_user_id) VALUES('raw-drain',$1) RETURNING id",
        )
        .bind(user)
        .fetch_one(&db)
        .await
        .unwrap();
        let run:Uuid=sqlx::query_scalar("INSERT INTO product_bulk_runs(workspace_id,created_by,state,finished_at,row_count) VALUES($1,$2,'completed',now()-interval '8 days',1200) RETURNING id").bind(workspace).bind(user).fetch_one(&db).await.unwrap();
        sqlx::query("INSERT INTO product_bulk_rows(workspace_id,run_id,ordinal,operation,target,values,raw) SELECT $1,$2,n,'create','{}','{}','{\"cells\":[\"private source\"]}' FROM generate_series(1,1200) n").bind(workspace).bind(run).execute(&db).await.unwrap();
        let ctx = Ctx {
            invoice_state: None,
            assistant_api_key: None,
            db: db.clone(),
            files: None,
            http: crate::outbound::Outbound::new(&[]).unwrap(),
            credentials: None,
            payments: Default::default(),
        };
        let job = Claimed {
            id: Uuid::now_v7(),
            kind: crate::jobs::kinds::PRODUCT_BULK_PRUNE.into(),
            workspace_id: None,
            payload: json!({}),
            attempts: 1,
            max_attempts: 5,
            created_by: None,
            token: Uuid::now_v7(),
        };
        for remaining in [700i64, 200, 0] {
            assert!(matches!(
                crate::jobs::kinds::dispatch(&ctx, job.clone())
                    .await
                    .unwrap(),
                Step::More
            ));
            let actual: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM product_bulk_rows WHERE run_id=$1 AND raw IS NOT NULL",
            )
            .bind(run)
            .fetch_one(&db)
            .await
            .unwrap();
            assert_eq!(actual, remaining);
        }
        assert!(matches!(
            crate::jobs::kinds::dispatch(&ctx, job).await.unwrap(),
            Step::Done
        ));
    }
    #[test]
    fn percent_uses_exact_raw_decimal() {
        assert_eq!(
            percent_value(&json!("0.1"), &json!("5")).unwrap(),
            json!("0.105")
        );
        assert!(percent_value(&Value::Null, &json!("5")).is_err());
    }
    #[test]
    fn file_boolean_requires_explicit_interpretation() {
        assert_eq!(cell_value("has_stock", "false").unwrap(), json!(false));
        assert!(cell_value("has_stock", "yes").is_err());
    }
    #[test]
    fn parser_failure_keeps_safe_actionable_reason() {
        let failure = issue(&AppError::Unprocessable(
            "Document parsing exceeded 60 seconds. Split the file.".into(),
        ));
        assert_eq!(failure["code"], "source_unreadable");
        assert!(failure["message"].as_str().unwrap().contains("60 seconds"));
    }
    #[sqlx::test]
    async fn header_reset_fences_old_extraction_even_when_cursor_is_empty(db: sqlx::PgPool) {
        let user:Uuid=sqlx::query_scalar("INSERT INTO users(email,first_name,last_name,password_hash) VALUES('source-fence@test.invalid','Source','Fence','unused') RETURNING id").fetch_one(&db).await.unwrap();
        let workspace:Uuid=sqlx::query_scalar("INSERT INTO workspaces(name,created_by_user_id) VALUES('source-fence',$1) RETURNING id").bind(user).fetch_one(&db).await.unwrap();
        let run: Uuid = sqlx::query_scalar(
            "INSERT INTO product_bulk_runs(workspace_id,created_by) VALUES($1,$2) RETURNING id",
        )
        .bind(workspace)
        .bind(user)
        .fetch_one(&db)
        .await
        .unwrap();
        let source = Uuid::now_v7();
        sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256) VALUES($1,$2,$3,$4,'source.csv','text/csv',1,repeat('0',64))").bind(source).bind(workspace).bind(run).bind(format!("genesy/product-bulk/{workspace}/{run}/{source}")).execute(&db).await.unwrap();
        let source = next_source(&db, workspace, run).await.unwrap().unwrap();
        sqlx::query(
            "UPDATE product_bulk_sources SET generation=generation+1,cursor='{}' WHERE id=$1",
        )
        .bind(source.id)
        .execute(&db)
        .await
        .unwrap();
        let mut conn = db.acquire().await.unwrap();
        crate::jobs::repo::enqueue(
            &mut conn,
            &crate::jobs::repo::NewJob {
                workspace_id: Some(workspace),
                payload: json!({"run_id":run,"phase":"prepare"}),
                ..crate::jobs::repo::NewJob::new(crate::jobs::kinds::PRODUCT_BULK)
            },
        )
        .await
        .unwrap();
        drop(conn);
        let job = crate::jobs::repo::claim(
            &db,
            &[crate::jobs::kinds::PRODUCT_BULK],
            2,
            crate::jobs::LEASE,
        )
        .await
        .unwrap()
        .unwrap();
        let ctx = Ctx {
            invoice_state: None,
            assistant_api_key: None,
            db: db.clone(),
            files: None,
            http: crate::outbound::Outbound::new(&[]).unwrap(),
            credentials: None,
            payments: Default::default(),
        };
        let step = ExtractionStep {
            rows: vec![RawRow {
                source_id: Some(source.id),
                sheet: None,
                page: None,
                line: 2,
                cells: vec!["old data".into()],
                columns: vec!["name".into()],
                formula_columns: vec![],
                header_candidate: false,
                needs_input: None,
            }],
            columns: vec!["name".into()],
            cursor: json!({"after":2}),
            done: true,
            total: Some(1),
            selection: None,
            suggested_mapping: None,
        };
        assert_eq!(
            persist_extraction(&ctx, &job, workspace, run, 1, source, Ok(step))
                .await
                .unwrap(),
            Step::More
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM product_bulk_rows WHERE run_id=$1")
                .bind(run)
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, Value>(
                "SELECT cursor FROM product_bulk_sources WHERE run_id=$1"
            )
            .bind(run)
            .fetch_one(&db)
            .await
            .unwrap(),
            json!({})
        );
        let fresh = next_source(&db, workspace, run).await.unwrap().unwrap();
        sqlx::query("UPDATE product_bulk_runs SET created_at=clock_timestamp()-interval '8 days' WHERE id=$1").bind(run).execute(&db).await.unwrap();
        let expired_step = ExtractionStep {
            rows: vec![RawRow {
                source_id: Some(fresh.id),
                sheet: None,
                page: None,
                line: 2,
                cells: vec!["late data".into()],
                columns: vec!["name".into()],
                formula_columns: vec![],
                header_candidate: false,
                needs_input: None,
            }],
            columns: vec!["name".into()],
            cursor: json!({"after":2}),
            done: true,
            total: Some(1),
            selection: None,
            suggested_mapping: None,
        };
        assert_eq!(
            persist_extraction(&ctx, &job, workspace, run, 1, fresh, Ok(expired_step))
                .await
                .unwrap(),
            Step::Done
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM product_bulk_runs WHERE id=$1")
                .bind(run)
                .fetch_one(&db)
                .await
                .unwrap(),
            "expired"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM product_bulk_rows WHERE run_id=$1")
                .bind(run)
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
    }
}
