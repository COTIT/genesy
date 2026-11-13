//! Revisioned staging and immutable approvals. Domain data stays outside the jobs payload.
use super::dto::*;
use crate::{
    AppError, AppState,
    http::workspace::WorkspaceAccess,
    jobs::{
        kinds,
        repo::{self as jobs, NewJob},
    },
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use uuid::Uuid;

pub fn enabled(state: &AppState) -> Result<(), AppError> {
    if !state.product_bulk_enabled {
        return Err(AppError::Unavailable("Product bulk runs are disabled."));
    }
    Ok(())
}
pub fn read(access: &WorkspaceAccess) -> Result<(), AppError> {
    if !access.allows("product.list") || !access.allows("product.view") {
        return Err(AppError::Forbidden(
            "Product list and view permissions are required.",
        ));
    }
    Ok(())
}
pub fn operation_allowed(access: &WorkspaceAccess, operation: &str) -> Result<(), AppError> {
    let permission = match operation {
        "create" => "product.create",
        "update" => "product.update",
        "delete" => "product.delete",
        _ => {
            return Err(AppError::field(
                "operation",
                "Choose create, update or delete.",
            ));
        }
    };
    if !access.allows(permission) {
        return Err(AppError::Forbidden(
            "Permission for this product change is required.",
        ));
    }
    Ok(())
}
pub async fn owned(db: &PgPool, access: &WorkspaceAccess, id: Uuid) -> Result<(), AppError> {
    read(access)?;
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT created_by FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2",
    )
    .bind(access.workspace_id)
    .bind(id)
    .fetch_optional(db)
    .await?;
    if owner.is_none_or(|u| u != access.user_id && !access.is_admin()) {
        return Err(AppError::NotFound);
    }
    expire_draft(db, access.workspace_id, id).await?;
    Ok(())
}
/// Draft lifetime is fixed at creation; activity cannot silently extend consent.
pub async fn expire_draft(db: &PgPool, workspace: Uuid, run: Uuid) -> Result<bool, AppError> {
    let due:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_runs r WHERE r.workspace_id=$1 AND r.id=$2 AND r.created_at<=clock_timestamp()-interval '7 days' AND r.state NOT IN ('completed','cancelled','failed','expired') AND NOT EXISTS(SELECT 1 FROM product_bulk_reviews v WHERE v.run_id=r.id AND v.approved_at IS NOT NULL))").bind(workspace).bind(run).fetch_one(db).await?;
    if !due {
        return Ok(false);
    }
    let mut tx = db.begin().await?;
    let expired = expire_locked(&mut tx, workspace, run).await?;
    tx.commit().await?;
    Ok(expired)
}
pub async fn expire_locked(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    run: Uuid,
) -> Result<bool, AppError> {
    let row=sqlx::query("SELECT state,created_at<=clock_timestamp()-interval '7 days' AS old FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(workspace).bind(run).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let state: String = row.get("state");
    if state == "expired" {
        return Ok(true);
    }
    if matches!(state.as_str(), "completed" | "cancelled" | "failed") || !row.get::<bool, _>("old")
    {
        return Ok(false);
    }
    let approved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_reviews WHERE run_id=$1 AND approved_at IS NOT NULL)").bind(run).fetch_one(&mut **tx).await?;
    if approved {
        return Ok(false);
    }
    sqlx::query("UPDATE product_bulk_runs SET state='expired',cancelled_at=clock_timestamp(),finished_at=clock_timestamp(),updated_at=clock_timestamp(),issues='[{\"code\":\"draft_expired\",\"message\":\"This unapproved draft expired after seven days. Prepare a fresh run.\"}]' WHERE id=$1").bind(run).execute(&mut **tx).await?;
    sqlx::query(
        "UPDATE product_bulk_reviews SET state='expired' WHERE run_id=$1 AND state='pending'",
    )
    .bind(run)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}
pub async fn get(db: &PgPool, workspace: Uuid, id: Uuid) -> Result<Value, AppError> {
    let mut run:Value=sqlx::query_scalar("SELECT to_jsonb(r)-'selection_cursor'-'prepare_cursor'-'retain_until' FROM product_bulk_runs r WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(id).fetch_optional(db).await?.ok_or(AppError::NotFound)?;
    run["sources"]=Value::Array(sqlx::query_scalar("SELECT (to_jsonb(s)-'object_key'-'sha256'-'workspace_id'-'run_id'-'cursor')||jsonb_build_object('header_rows',coalesce(cursor->'header_rows','{}'::jsonb),'progress',CASE WHEN cursor ? 'total_pages' THEN jsonb_build_object('pages_done',least((cursor->>'total_pages')::bigint,greatest(0,(cursor->>'page')::bigint-1)),'total_pages',cursor->'total_pages') ELSE jsonb_strip_nulls(jsonb_build_object('rows_read',cursor->'rows_read','sheet_index',cursor->'sheet_index')) END) FROM product_bulk_sources s WHERE workspace_id=$1 AND run_id=$2 ORDER BY created_at,id").bind(workspace).bind(id).fetch_all(db).await?);
    run["reviews"]=Value::Array(sqlx::query_scalar("SELECT to_jsonb(v)-'workspace_id'-'run_id' FROM product_bulk_reviews v WHERE workspace_id=$1 AND run_id=$2 ORDER BY created_at DESC,id DESC LIMIT 100").bind(workspace).bind(id).fetch_all(db).await?);
    let counts:Value=sqlx::query_scalar("SELECT jsonb_build_object('total',count(*),'ready',count(*) FILTER(WHERE status='ready'),'errors',count(*) FILTER(WHERE status='error'),'applied',count(*) FILTER(WHERE status='applied'),'excluded',count(*) FILTER(WHERE NOT included),'stale',count(*) FILTER(WHERE status='stale'),'deletes',count(*) FILTER(WHERE included AND operation='delete'),'validated',count(*) FILTER(WHERE validated_revision=(SELECT revision FROM product_bulk_runs WHERE id=$2))) FROM product_bulk_rows WHERE workspace_id=$1 AND run_id=$2").bind(workspace).bind(id).fetch_one(db).await?;
    run["progress"] = json!({"phase":run["state"],"done":counts["applied"],"total":counts["total"],"validated":counts["validated"],"applied":counts["applied"],"failed":counts["errors"].as_i64().unwrap_or(0)+counts["stale"].as_i64().unwrap_or(0),"skipped":counts["excluded"]});
    run["counts"] = counts;
    run["spend"]=sqlx::query_scalar::<_,Value>("SELECT jsonb_build_object('reserved_usd',coalesce(sum(upper_bound_usd) FILTER(WHERE state='reserved'),0)::text,'actual_usd',coalesce(sum(actual_usd),0)::text) FROM product_bulk_ai_costs WHERE workspace_id=$1 AND run_id=$2").bind(workspace).bind(id).fetch_one(db).await?;
    Ok(run)
}
pub fn with_permissions(mut run: Value, access: &WorkspaceAccess) -> Value {
    let frozen = run["reviews"]
        .as_array()
        .is_some_and(|a| a.iter().any(|r| !r["approved_at"].is_null()));
    let mutable = !frozen
        && !matches!(
            run["state"].as_str(),
            Some("queued" | "running" | "completed" | "cancelled" | "expired" | "extracting")
        );
    let resumable_sources = run["sources"].as_array().is_none_or(|sources| {
        sources.iter().all(|s| {
            s["state"] != "error"
                || matches!(
                    s["issues"][0]["code"].as_str(),
                    Some("budget" | "provider" | "unknown_reservation")
                )
        })
    });
    let resume = mutable
        && resumable_sources
        && matches!(
            run["state"].as_str(),
            Some("paused" | "needs_input" | "failed")
        );
    run["permissions"] = json!({"edit":mutable,"resume":resume,"cancel":!matches!(run["state"].as_str(),Some("completed"|"cancelled"|"expired")),"approve":!matches!(run["state"].as_str(),Some("completed"|"cancelled"|"expired"|"failed"))&&(access.allows("product.create")||access.allows("product.update")||access.allows("product.delete"))});
    run
}
pub async fn create(
    state: &AppState,
    access: &WorkspaceAccess,
    input: CreateInput,
) -> Result<Value, AppError> {
    enabled(state)?;
    read(access)?;
    let mode = input.mode.unwrap_or_else(|| "create".into());
    if !["create", "update", "delete", "upsert"].contains(&mode.as_str()) {
        return Err(AppError::field(
            "mode",
            "Choose create, update, delete or upsert.",
        ));
    }
    if mode == "upsert" {
        operation_allowed(access, "create")?;
        operation_allowed(access, "update")?;
    } else {
        operation_allowed(access, &mode)?;
    }
    let key = input
        .match_key
        .unwrap_or_else(|| "product_nr_formatted".into());
    if ![
        "product_nr_formatted",
        "barcode",
        "name",
        "id",
        "ean13",
        "reference",
    ]
    .contains(&key.as_str())
    {
        return Err(AppError::field(
            "match_key",
            "Choose a supported exact product key.",
        ));
    }
    let name = input.name.unwrap_or_else(|| "Product changes".into());
    let instruction = input.instruction.unwrap_or_default();
    if name.trim().is_empty() || name.len() > 255 || instruction.len() > MAX_CELL_BYTES {
        return Err(AppError::field(
            "name",
            "Use a name up to 255 bytes and an instruction up to 16 KiB.",
        ));
    }
    let id:Uuid=sqlx::query_scalar("INSERT INTO product_bulk_runs(workspace_id,created_by,name,instruction,mode,match_key) VALUES($1,$2,$3,$4,$5,$6) RETURNING id").bind(access.workspace_id).bind(access.user_id).bind(name).bind(instruction).bind(mode).bind(key).fetch_one(&state.db).await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, id).await?,
        access,
    ))
}
pub async fn lock_edit<'a>(
    tx: &mut Transaction<'a, Postgres>,
    workspace: Uuid,
    id: Uuid,
    revision: i64,
) -> Result<(), AppError> {
    let row = sqlx::query(
        "SELECT revision,state FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(workspace)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if row.get::<i64, _>("revision") != revision {
        return Err(AppError::Conflict(
            "This run changed. Reload its latest revision.",
        ));
    }
    let approved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_reviews WHERE run_id=$1 AND approved_at IS NOT NULL)").bind(id).fetch_one(&mut **tx).await?;
    if approved
        || matches!(
            row.get::<String, _>("state").as_str(),
            "queued" | "running" | "completed" | "cancelled" | "expired"
        )
    {
        return Err(AppError::Conflict(
            "Cancel or copy this run before changing its staged rows.",
        ));
    }
    Ok(())
}
pub async fn invalidate(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("UPDATE product_bulk_reviews SET state='superseded' WHERE workspace_id=$1 AND run_id=$2 AND state IN ('pending','approved')").bind(workspace).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE product_bulk_runs SET revision=revision+1,state='preparing',prepare_cursor=0,updated_at=now(),approved_until=NULL WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE product_bulk_rows SET validated_revision=NULL,status=CASE WHEN included THEN 'staged' ELSE 'excluded' END,material=NULL,issues='[]' WHERE workspace_id=$1 AND run_id=$2 AND status<>'applied'").bind(workspace).bind(id).execute(&mut **tx).await?;
    enqueue(tx, workspace, id, "prepare").await
}
pub async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    run: Uuid,
    phase: &str,
) -> Result<(), AppError> {
    // The run lock serializes producers; a queued follow-up survives a running chunk finishing.
    let queued:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE workspace_id=$1 AND kind=$2 AND status='queued' AND payload->>'run_id'=$3 AND payload->>'phase'=$4)").bind(workspace).bind(kinds::PRODUCT_BULK).bind(run.to_string()).bind(phase).fetch_one(&mut **tx).await?;
    if !queued {
        jobs::enqueue(
            tx,
            &NewJob {
                workspace_id: Some(workspace),
                payload: json!({"run_id":run,"phase":phase}),
                concurrency_key: Some(format!("product_bulk:{run}")),
                ..NewJob::new(kinds::PRODUCT_BULK)
            },
        )
        .await?;
    }
    Ok(())
}
pub fn check_values(v: &Value) -> Result<usize, AppError> {
    fn count(v: &Value, depth: usize) -> Result<usize, AppError> {
        if depth > 8 {
            return Err(AppError::field("values", "Values are too deeply nested."));
        }
        match v {
            Value::String(s) if s.len() > MAX_CELL_BYTES || s.contains('\0') => Err(
                AppError::field("values", "Each value must fit 16 KiB and contain no NUL."),
            ),
            Value::Array(a) => a
                .iter()
                .try_fold(0usize, |n, v| Ok(n + count(v, depth + 1)?)),
            Value::Object(o) => o
                .values()
                .try_fold(0usize, |n, v| Ok(n + count(v, depth + 1)?)),
            _ => Ok(1),
        }
    }
    if !v.is_object() {
        return Err(AppError::field(
            "values",
            "Use an object of product fields.",
        ));
    }
    count(v, 0)
}
pub async fn recount(tx: &mut Transaction<'_, Postgres>, run: Uuid) -> Result<(), AppError> {
    sqlx::query("UPDATE product_bulk_runs SET row_count=(SELECT count(*) FROM product_bulk_rows WHERE run_id=$1),cell_count=(SELECT coalesce(sum(cell_count),0) FROM product_bulk_rows WHERE run_id=$1) WHERE id=$1").bind(run).execute(&mut **tx).await?;
    Ok(())
}
pub async fn insert_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    run: Uuid,
    ordinal: i64,
    row: &StagedRow,
    raw: Option<&RawRow>,
) -> Result<(), AppError> {
    if !["create", "update", "delete", "upsert"].contains(&row.operation.as_str())
        || !row.target.is_object()
    {
        return Err(AppError::field(
            "row",
            "Use a valid operation and target object.",
        ));
    }
    let cells = check_values(&row.values)?;
    if let Some(source) = row.source_id {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_sources WHERE workspace_id=$1 AND run_id=$2 AND id=$3)").bind(workspace).bind(run).bind(source).fetch_one(&mut **tx).await?;
        if !valid {
            return Err(AppError::NotFound);
        }
    }
    sqlx::query("INSERT INTO product_bulk_rows(workspace_id,run_id,ordinal,source_id,sheet,page,line,raw,operation,target,values,included,cell_count) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(workspace).bind(run).bind(ordinal).bind(row.source_id).bind(&row.sheet).bind(row.page.map(|n|n as i32)).bind(row.line.map(|n|n as i32)).bind(raw.map(serde_json::to_value).transpose().map_err(|_|AppError::field("row","Invalid raw row."))?).bind(&row.operation).bind(&row.target).bind(&row.values).bind(row.included).bind(cells as i32).execute(&mut **tx).await?;
    sqlx::query("UPDATE product_bulk_runs SET row_count=row_count+1,cell_count=cell_count+$3 WHERE workspace_id=$1 AND id=$2 AND row_count<50000 AND cell_count+$3<=2000000").bind(workspace).bind(run).bind(cells as i64).execute(&mut **tx).await?.rows_affected().eq(&1).then_some(()).ok_or(AppError::field("rows","A run supports at most 50,000 rows and 2 million cells."))?;
    Ok(())
}
pub fn validate_selection(plan: &SelectionPlan) -> Result<(), AppError> {
    let s = &plan.selection;
    if !matches!(plan.operation.as_str(), "update" | "delete") {
        return Err(AppError::field(
            "operation",
            "Selections support update or explicit delete.",
        ));
    }
    if s.ids
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > MAX_ROWS as usize)
    {
        return Err(AppError::field(
            "selection",
            "Select one through 50,000 product IDs.",
        ));
    }
    if s.ids.is_none()
        && s.category_id.is_none()
        && s.status.is_none()
        && s.q.as_ref().is_none_or(|v| v.trim().is_empty())
        && s.custom_filters
            .as_ref()
            .is_none_or(|v| v.as_array().is_none_or(Vec::is_empty))
    {
        return Err(AppError::field(
            "selection",
            "Choose explicit products or a filter before staging changes.",
        ));
    }
    if s.status
        .as_deref()
        .is_some_and(|v| !["active", "disabled", "nla"].contains(&v))
    {
        return Err(AppError::field(
            "selection.status",
            "Choose active, disabled or nla.",
        ));
    }
    if s.q.as_ref().is_some_and(|s| s.len() > 255) {
        return Err(AppError::field(
            "selection.q",
            "Search text cannot exceed 255 bytes.",
        ));
    }
    if plan.operation == "update" && plan.changes.is_empty() {
        return Err(AppError::field(
            "changes",
            "Choose at least one product field change.",
        ));
    }
    if plan.operation == "delete" && !plan.changes.is_empty() {
        return Err(AppError::field(
            "changes",
            "Delete selections cannot contain field changes.",
        ));
    }
    for (field, c) in &plan.changes {
        let custom = field.strip_prefix("custom_fields.");
        if custom.is_some_and(|id| Uuid::parse_str(id).is_err())
            || custom.is_none() && !super::extraction::is_native_target(field)
        {
            return Err(AppError::field(
                "changes",
                format!("Unsupported product field: {field}."),
            ));
        }
        if !["set", "percent"].contains(&c.kind.as_str()) || field.len() > 100 {
            return Err(AppError::field(
                "changes",
                "Choose set or percent for each field.",
            ));
        }
        if c.kind == "percent"
            && (![
                "price",
                "wholesale_price",
                "supplier_wholesale_price",
                "weight",
                "order_unit",
                "sale_unit",
            ]
            .contains(&field.as_str())
                || c.value
                    .as_str()
                    .and_then(|v| Decimal::from_str(v).ok())
                    .is_none())
        {
            return Err(AppError::field(
                "changes",
                "Percent changes require a supported raw numeric field and an exact decimal string.",
            ));
        }
    }
    Ok(())
}
pub async fn stage(
    state: &AppState,
    access: &WorkspaceAccess,
    id: Uuid,
    input: StageInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, id).await?;
    enabled(state)?;
    if input.rows.len() > 500 {
        return Err(AppError::field(
            "rows",
            "Stage at most 500 explicit rows per request.",
        ));
    }
    let mut pending_source = None;
    let mut tx = state.db.begin().await?;
    lock_edit(&mut tx, access.workspace_id, id, input.revision).await?;
    if let Some(selection) = input.selection {
        let plan = SelectionPlan {
            selection,
            operation: input
                .operation
                .ok_or(AppError::field("operation", "Choose update or delete."))?,
            changes: input.changes.unwrap_or_default(),
        };
        validate_selection(&plan)?;
        operation_allowed(access, &plan.operation)?;
        if !input.rows.is_empty() || input.text.is_some() {
            return Err(AppError::field(
                "selection",
                "Stage a selection separately from text or explicit rows.",
            ));
        }
        sqlx::query("DELETE FROM product_bulk_rows WHERE run_id=$1 AND selection_generated AND status<>'applied'").bind(id).execute(&mut *tx).await?;
        recount(&mut tx, id).await?;
        sqlx::query("UPDATE product_bulk_runs SET selection=$3,selection_done=false,selection_cursor=NULL WHERE workspace_id=$1 AND id=$2").bind(access.workspace_id).bind(id).bind(serde_json::to_value(plan).unwrap()).execute(&mut *tx).await?;
    } else if let Some(text) = input.text {
        if !input.rows.is_empty() {
            return Err(AppError::field(
                "text",
                "Stage text separately from explicit rows.",
            ));
        }
        if text.trim().is_empty() || text.len() > MAX_CELL_BYTES {
            return Err(AppError::field(
                "text",
                "Use a nonempty instruction up to 16 KiB.",
            ));
        }
        let bytes = text.into_bytes();
        let source = save_source(
            state,
            &mut tx,
            access.workspace_id,
            id,
            "chat.txt",
            "text/plain",
            &bytes,
        )
        .await?;
        pending_source = Some((source, bytes));
    } else if input.rows.is_empty() {
        return Err(AppError::field(
            "rows",
            "Stage rows, a file, text or an explicit selection.",
        ));
    }
    let mut ordinal: i64 = sqlx::query_scalar(
        "SELECT coalesce(max(ordinal),0) FROM product_bulk_rows WHERE run_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    for row in &input.rows {
        if row.operation != "upsert" {
            operation_allowed(access, &row.operation)?;
        }
        ordinal += 1;
        insert_row(&mut tx, access.workspace_id, id, ordinal, row, None).await?;
    }
    invalidate(&mut tx, access.workspace_id, id).await?;
    tx.commit().await?;
    if let Some((source, bytes)) = pending_source {
        finish_source(state, source, bytes).await?;
    }
    Ok(with_permissions(
        get(&state.db, access.workspace_id, id).await?,
        access,
    ))
}
pub async fn save_source(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    run: Uuid,
    name: &str,
    media: &str,
    bytes: &[u8],
) -> Result<Uuid, AppError> {
    if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES || name.len() > 255 {
        return Err(AppError::field(
            "file",
            "Use a nonempty source up to 10 MiB with a name up to 255 bytes.",
        ));
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_bulk_sources WHERE workspace_id=$1 AND run_id=$2",
    )
    .bind(workspace)
    .bind(run)
    .fetch_one(&mut **tx)
    .await?;
    if count >= 10 {
        return Err(AppError::field(
            "file",
            "A run supports at most ten sources.",
        ));
    }
    let id = Uuid::now_v7();
    let key = format!("genesy/product-bulk/{workspace}/{run}/{id}");
    let size = bytes.len() as i64;
    let hash = hex::encode(Sha256::digest(bytes));
    let _store = state.files.as_ref().ok_or(AppError::Unavailable(
        "Private import storage is not configured.",
    ))?;
    sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending')").bind(id).bind(workspace).bind(run).bind(&key).bind(name).bind(media).bind(size).bind(hash).execute(&mut **tx).await?;
    Ok(id)
}
pub async fn finish_source(state: &AppState, id: Uuid, bytes: Vec<u8>) -> Result<(), AppError> {
    let r=sqlx::query("SELECT object_key,name,media_type FROM product_bulk_sources WHERE id=$1 AND state='pending'").bind(id).fetch_one(&state.db).await?;
    let store = state.files.as_ref().ok_or(AppError::Unavailable(
        "Private import storage is not configured.",
    ))?;
    let result = store
        .put(
            r.get("object_key"),
            bytes,
            r.get("media_type"),
            r.get("name"),
            false,
        )
        .await;
    if result.is_ok() {
        let active:bool=sqlx::query_scalar("SELECT r.cancelled_at IS NULL FROM product_bulk_sources s JOIN product_bulk_runs r ON r.id=s.run_id WHERE s.id=$1").bind(id).fetch_one(&state.db).await?;
        if active {
            sqlx::query(
                "UPDATE product_bulk_sources SET state='stored' WHERE id=$1 AND state='pending'",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
        } else {
            store.delete(r.get("object_key")).await?;
            sqlx::query("UPDATE product_bulk_sources SET state='cleaned' WHERE id=$1")
                .bind(id)
                .execute(&state.db)
                .await?;
        }
    } else {
        sqlx::query("UPDATE product_bulk_sources SET state='error',issues='[{\"code\":\"upload\",\"message\":\"Source storage failed. Start a fresh run with the original source.\"}]' WHERE id=$1").bind(id).execute(&state.db).await?;
    }
    result
}
pub async fn mapping(
    state: &AppState,
    access: &WorkspaceAccess,
    id: Uuid,
    input: MappingInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, id).await?;
    enabled(state)?;
    if input.columns.is_empty()
        || input.columns.len() > 500
        || input.columns.iter().any(|(k, v)| {
            k.len() > MAX_CELL_BYTES
                || v.as_str().is_some_and(|s| s.len() > 150)
                || v.is_object() && v.get("target").and_then(Value::as_str).is_none()
        })
    {
        return Err(AppError::field(
            "columns",
            "Choose a bounded nonempty column mapping.",
        ));
    }
    let mut tx = state.db.begin().await?;
    lock_edit(&mut tx, access.workspace_id, id, input.revision).await?;
    let media_type: String = sqlx::query_scalar(
        "SELECT media_type FROM product_bulk_sources WHERE workspace_id=$1 AND run_id=$2 AND id=$3",
    )
    .bind(access.workspace_id)
    .bind(id)
    .bind(input.source_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let header_applicable = matches!(
        media_type.as_str(),
        "text/csv" | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    );
    let header_row = if header_applicable {
        input.header_row
    } else {
        Some(0)
    };
    let key = format!(
        "{}:{}",
        input.source_id,
        input.sheet.as_deref().unwrap_or("")
    );
    let previous: Value = sqlx::query_scalar("SELECT mapping FROM product_bulk_runs WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let prior = previous
        .get(&key)
        .and_then(|v| v.get("header_row"))
        .and_then(Value::as_u64);
    if header_applicable && header_row.is_some() && prior != header_row.map(u64::from) {
        sqlx::query(
            "DELETE FROM product_bulk_rows WHERE run_id=$1 AND source_id=$2 AND status<>'applied'",
        )
        .bind(id)
        .bind(input.source_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE product_bulk_sources SET cursor='{}',generation=generation+1,state='stored',columns='{}',suggested_mapping='{}',issues='[]' WHERE id=$1").bind(input.source_id).execute(&mut *tx).await?;
        recount(&mut tx, id).await?;
    }
    let v = json!({"source_id":input.source_id,"sheet":input.sheet,"header_row":header_row,"columns":input.columns});
    sqlx::query("UPDATE product_bulk_runs SET mapping=mapping||jsonb_build_object($3::text,$4::jsonb) WHERE workspace_id=$1 AND id=$2").bind(access.workspace_id).bind(id).bind(key).bind(v).execute(&mut *tx).await?;
    invalidate(&mut tx, access.workspace_id, id).await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, id).await?,
        access,
    ))
}
pub async fn edit_row(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
    row: Uuid,
    input: RowInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    enabled(state)?;
    if let Some(v) = &input.values {
        check_values(v)?;
    }
    if input.target.as_ref().is_some_and(|v| !v.is_object()) {
        return Err(AppError::field("target", "Use an exact target object."));
    }
    if let Some(op) = &input.operation {
        operation_allowed(access, op)?;
    }
    let mut tx = state.db.begin().await?;
    lock_edit(&mut tx, access.workspace_id, run, input.revision).await?;
    let n=sqlx::query("UPDATE product_bulk_rows SET included=coalesce($4,included),operation=coalesce($5,operation),target=coalesce($6,target),values=coalesce($7,values),manual_override=manual_override OR $5::text IS NOT NULL OR $6::jsonb IS NOT NULL OR $7::jsonb IS NOT NULL WHERE workspace_id=$1 AND run_id=$2 AND id=$3 AND status<>'applied'").bind(access.workspace_id).bind(run).bind(row).bind(input.included).bind(input.operation).bind(input.target).bind(input.values).execute(&mut *tx).await?.rows_affected();
    if n != 1 {
        return Err(AppError::NotFound);
    }
    let stored =
        sqlx::query("SELECT values,raw,manual_override FROM product_bulk_rows WHERE id=$1")
            .bind(row)
            .fetch_one(&mut *tx)
            .await?;
    let raw: Option<Value> = stored.get("raw");
    let cells = if !stored.get::<bool, _>("manual_override") {
        raw.as_ref()
            .and_then(|r| r["cells"].as_array())
            .map(|cells| {
                cells
                    .iter()
                    .filter(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
                    .count()
            })
            .unwrap_or(check_values(&stored.get::<Value, _>("values"))?)
    } else {
        check_values(&stored.get::<Value, _>("values"))?
    };
    sqlx::query("UPDATE product_bulk_rows SET cell_count=$2 WHERE id=$1")
        .bind(row)
        .bind(cells as i32)
        .execute(&mut *tx)
        .await?;
    recount(&mut tx, run).await?;
    invalidate(&mut tx, access.workspace_id, run).await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, run).await?,
        access,
    ))
}
pub async fn review(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
    input: ReviewInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    if !matches!(input.scope.as_str(), "write" | "delete") {
        return Err(AppError::field("scope", "Choose write or delete."));
    }
    let mut tx = state.db.begin().await?;
    let current:i64=sqlx::query_scalar("SELECT revision FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 AND cancelled_at IS NULL FOR UPDATE").bind(access.workspace_id).bind(run).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound)?;
    if current != input.revision {
        return Err(AppError::Conflict(
            "This run changed. Reload its current revision.",
        ));
    }
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM product_bulk_sources WHERE run_id=$1 AND state<>'done')",
    )
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    if blocked {
        return Err(AppError::Conflict(
            "Every source must finish extraction before review. Correct or replace failed sources in a fresh run.",
        ));
    }
    let state_name: String = sqlx::query_scalar("SELECT state FROM product_bulk_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    if !["ready", "queued", "running"].contains(&state_name.as_str()) {
        return Err(AppError::Conflict(
            "Wait for extraction, mapping and validation before reviewing.",
        ));
    }
    let duplicates:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_rows WHERE run_id=$1 AND included AND issues @> '[{\"code\":\"duplicate_target\"}]'::jsonb)")
        .bind(run).fetch_one(&mut *tx).await?;
    if duplicates {
        return Err(AppError::Conflict(
            "Duplicate source keys or resolved products remain. Correct or exclude conflicting rows before preparing any review.",
        ));
    }
    let already:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_reviews WHERE run_id=$1 AND revision=$2 AND scope=$3 AND approved_at IS NOT NULL)").bind(run).bind(input.revision).bind(&input.scope).fetch_one(&mut *tx).await?;
    if already {
        return Err(AppError::Conflict(
            "This scope was already approved. Use a fresh remainder review for unapplied rows.",
        ));
    }
    let ops:Vec<String>=sqlx::query_scalar("SELECT DISTINCT operation FROM product_bulk_rows WHERE run_id=$1 AND included AND status='ready' AND ($2='delete')=(operation='delete')").bind(run).bind(&input.scope).fetch_all(&mut *tx).await?;
    for op in &ops {
        operation_allowed(access, op)?;
    }
    if ops.is_empty() {
        return Err(AppError::field(
            "scope",
            "No valid included rows belong to this approval scope.",
        ));
    }
    sqlx::query("UPDATE product_bulk_reviews SET state='superseded' WHERE run_id=$1 AND scope=$2 AND state='pending'").bind(run).bind(&input.scope).execute(&mut *tx).await?;
    let id:Uuid=sqlx::query_scalar("INSERT INTO product_bulk_reviews(workspace_id,run_id,revision,scope,fingerprint,row_count) VALUES($1,$2,$3,$4,'',0) RETURNING id").bind(access.workspace_id).bind(run).bind(input.revision).bind(&input.scope).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO product_bulk_review_rows(review_id,row_id,ordinal,request,material) SELECT $1,id,ordinal,jsonb_build_object('operation',operation,'target',target,'values',values,'resolved_id',resolved_id),material FROM product_bulk_rows WHERE run_id=$2 AND included AND status='ready' AND validated_revision=$3 AND ($4='delete')=(operation='delete')").bind(id).bind(run).bind(input.revision).bind(&input.scope).execute(&mut *tx).await?;
    let header:Value=sqlx::query_scalar("UPDATE product_bulk_reviews v SET row_count=(SELECT count(*) FROM product_bulk_review_rows WHERE review_id=$1),fingerprint=(SELECT encode(sha256(convert_to(coalesce(string_agg(ordinal::text||':'||encode(sha256(convert_to(jsonb_build_object('request',request,'material',material)::text,'UTF8')),'hex'),',' ORDER BY ordinal),''),'UTF8')),'hex') FROM product_bulk_review_rows WHERE review_id=$1) WHERE id=$1 RETURNING to_jsonb(v)-'workspace_id'-'run_id'").bind(id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(header)
}
pub async fn approve(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
    input: ApproveInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    let mut tx = state.db.begin().await?;
    let r=sqlx::query("SELECT revision,cancelled_at,state FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(access.workspace_id).bind(run).fetch_one(&mut *tx).await?;
    if !matches!(
        r.get::<String, _>("state").as_str(),
        "ready" | "queued" | "running"
    ) || r.get::<i64, _>("revision") != input.revision
        || r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancelled_at")
            .is_some()
    {
        return Err(AppError::Conflict(
            "This run changed or was cancelled. Prepare a fresh review.",
        ));
    }
    let review=sqlx::query("SELECT revision,state,expires_at>now() AS live FROM product_bulk_reviews WHERE run_id=$1 AND id=$2 FOR UPDATE").bind(run).bind(input.review_id).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound)?;
    if review.get::<i64, _>("revision") != input.revision
        || !review.get::<bool, _>("live")
        || review.get::<String, _>("state") != "pending"
    {
        return Err(AppError::Conflict(
            "This exact review expired, changed or was already approved.",
        ));
    }
    let ops: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT request->>'operation' FROM product_bulk_review_rows WHERE review_id=$1",
    )
    .bind(input.review_id)
    .fetch_all(&mut *tx)
    .await?;
    for op in &ops {
        operation_allowed(access, op)?;
    }
    sqlx::query("UPDATE product_bulk_reviews SET state='approved',approved_by=$2,approved_at=now(),expires_at=now()+interval '24 hours' WHERE id=$1").bind(input.review_id).bind(access.user_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE product_bulk_runs SET state='queued',approved_until=now()+interval '24 hours',updated_at=now() WHERE id=$1").bind(run).execute(&mut *tx).await?;
    enqueue(&mut tx, access.workspace_id, run, "apply").await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, run).await?,
        access,
    ))
}
pub async fn cancel(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(access.workspace_id)
        .bind(run)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_bulk_runs SET state='cancelled',cancelled_at=coalesce(cancelled_at,now()),updated_at=now(),finished_at=coalesce(finished_at,now()) WHERE id=$1 AND state NOT IN ('completed','expired')").bind(run).execute(&mut *tx).await?;
    sqlx::query("UPDATE product_bulk_reviews SET state='cancelled' WHERE run_id=$1 AND state IN ('pending','approved')").bind(run).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, run).await?,
        access,
    ))
}
pub async fn resume(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
    input: ResumeInput,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    enabled(state)?;
    let mut tx = state.db.begin().await?;
    let row = sqlx::query(
        "SELECT revision,state FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.workspace_id)
    .bind(run)
    .fetch_one(&mut *tx)
    .await?;
    if row.get::<i64, _>("revision") != input.revision
        || !matches!(
            row.get::<String, _>("state").as_str(),
            "paused" | "needs_input" | "failed"
        )
    {
        return Err(AppError::Conflict(
            "Only a paused, unapproved preparation can resume at its current revision.",
        ));
    }
    let approved:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_reviews WHERE run_id=$1 AND approved_at IS NOT NULL)").bind(run).fetch_one(&mut *tx).await?;
    if approved {
        return Err(AppError::Conflict(
            "Approved execution cannot resume. Prepare a fresh remainder review.",
        ));
    }
    let invalid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_sources WHERE run_id=$1 AND state='error' AND coalesce(issues->0->>'code','') NOT IN ('budget','provider','unknown_reservation'))").bind(run).fetch_one(&mut *tx).await?;
    if invalid {
        return Err(AppError::Conflict(
            "An unreadable source needs correction in a fresh run.",
        ));
    }
    sqlx::query("UPDATE product_bulk_sources SET state=CASE WHEN state='error' THEN 'stored' ELSE state END,generation=generation+1,issues='[]' WHERE run_id=$1 AND state IN ('stored','extracting','error')").bind(run).execute(&mut *tx).await?;
    sqlx::query("UPDATE product_bulk_runs SET issues='[]' WHERE id=$1")
        .bind(run)
        .execute(&mut *tx)
        .await?;
    invalidate(&mut tx, access.workspace_id, run).await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, run).await?,
        access,
    ))
}
pub async fn remainder(
    state: &AppState,
    access: &WorkspaceAccess,
    run: Uuid,
) -> Result<Value, AppError> {
    owned(&state.db, access, run).await?;
    enabled(state)?;
    let mut tx = state.db.begin().await?;
    let old=sqlx::query("SELECT name,instruction,mode,match_key,state FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE").bind(access.workspace_id).bind(run).fetch_one(&mut *tx).await?;
    if !matches!(
        old.get::<String, _>("state").as_str(),
        "completed" | "cancelled" | "failed" | "needs_input" | "expired"
    ) {
        return Err(AppError::Conflict(
            "Finish or cancel this run before preparing its remainder.",
        ));
    }
    let id:Uuid=sqlx::query_scalar("INSERT INTO product_bulk_runs(workspace_id,created_by,name,instruction,mode,match_key,state) VALUES($1,$2,$3,$4,$5,$6,'preparing') RETURNING id").bind(access.workspace_id).bind(access.user_id).bind(format!("{} (remainder)",old.get::<String,_>("name"))).bind(old.get::<String,_>("instruction")).bind(old.get::<String,_>("mode")).bind(old.get::<String,_>("match_key")).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO product_bulk_rows(workspace_id,run_id,ordinal,operation,target,values,included,line,sheet,page,cell_count,provenance) SELECT r.workspace_id,$2,row_number() OVER(ORDER BY r.ordinal),r.operation,r.target,r.values,r.included,r.line,r.sheet,r.page,r.cell_count,CASE WHEN r.provenance<>'{}'::jsonb THEN r.provenance ELSE jsonb_build_object('origin_run_id',r.run_id,'row_id',r.id,'source_id',r.source_id,'source_name',s.name) END FROM product_bulk_rows r LEFT JOIN product_bulk_sources s ON s.id=r.source_id AND s.run_id=r.run_id WHERE r.run_id=$1 AND r.status<>'applied'").bind(run).bind(id).execute(&mut *tx).await?;
    recount(&mut tx, id).await?;
    enqueue(&mut tx, access.workspace_id, id, "prepare").await?;
    tx.commit().await?;
    Ok(with_permissions(
        get(&state.db, access.workspace_id, id).await?,
        access,
    ))
}

pub async fn resolve(
    conn: &mut PgConnection,
    workspace: Uuid,
    key_name: &str,
    target: &Value,
    values: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let ids = resolve_unlocked(conn, workspace, key_name, target, values).await?;
    lock_products(conn, workspace, &ids).await?;
    Ok(ids)
}

/// One grouped pass after this revision's extraction and all bounded row chunks finish.
pub(super) async fn reject_duplicates(
    conn: &mut PgConnection,
    run: Uuid,
    revision: i64,
    match_key: &str,
) -> Result<(), AppError> {
    sqlx::query(r#"WITH rows AS MATERIALIZED (
        SELECT id,target,values,resolved_id FROM product_bulk_rows
        WHERE run_id=$1 AND included AND validated_revision=$2
    ), identities AS MATERIALIZED (
        SELECT id,'key' AS kind,(CASE WHEN jsonb_typeof(target->'key')='string' AND btrim(target->>'key')<>'' THEN target->>'key'
            WHEN jsonb_typeof(values->$3)='string' THEN values->>$3 END) COLLATE "C" AS value FROM rows
        UNION SELECT id,'product',target->>'product_id' FROM rows WHERE jsonb_typeof(target->'product_id')='string'
        UNION SELECT id,'product',resolved_id::text FROM rows WHERE resolved_id IS NOT NULL
    ), duplicates AS (
        SELECT kind,value FROM identities WHERE value IS NOT NULL AND btrim(value)<>''
        GROUP BY kind,value HAVING count(*)>1
    ), members AS (
        SELECT DISTINCT i.id FROM identities i JOIN duplicates d USING(kind,value)
    ) UPDATE product_bulk_rows r SET status='error',issues=
        coalesce((SELECT jsonb_agg(issue) FROM jsonb_array_elements(r.issues) issue
                  WHERE issue->>'code'<>'duplicate_target'),'[]'::jsonb)
        || '[{"code":"duplicate_target","message":"Several included rows use the same exact source key or resolve to the same product. Correct their identities or exclude conflicting rows."}]'::jsonb
      FROM members m WHERE r.id=m.id AND r.run_id=$1 AND r.validated_revision=$2 AND r.included"#)
        .bind(run).bind(revision).bind(match_key).execute(conn).await?;
    Ok(())
}

/// Called under the workspace identity lock before taking a chunk's product locks.
pub(super) async fn resolve_unlocked(
    conn: &mut PgConnection,
    workspace: Uuid,
    key_name: &str,
    target: &Value,
    values: &Value,
) -> Result<Vec<Uuid>, AppError> {
    let direct = target
        .get("product_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| AppError::field("target", "Use a valid product UUID."))?;
    let key = target
        .get("key")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            values
                .get(key_name)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .map(|s| {
            if key_name == "id" {
                Uuid::parse_str(s).map(|id| id.to_string()).map_err(|_| {
                    AppError::field("target", "Use a valid product UUID as the ID matching key.")
                })
            } else {
                Ok(s.to_owned())
            }
        })
        .transpose()?;
    if direct.is_none() && key.is_none() {
        return Ok(vec![]);
    }
    Ok(sqlx::query_scalar("SELECT id FROM products WHERE workspace_id=$1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id=$2) AND ($3::text IS NULL OR CASE $4 WHEN 'id' THEN id::text WHEN 'barcode' THEN barcode WHEN 'ean13' THEN ean13 WHEN 'reference' THEN reference WHEN 'name' THEN name ELSE product_nr_formatted END=$3) ORDER BY id LIMIT 3").bind(workspace).bind(direct).bind(key).bind(key_name).fetch_all(conn).await?)
}

pub(super) async fn lock_products(
    conn: &mut PgConnection,
    workspace: Uuid,
    ids: &[Uuid],
) -> Result<(), AppError> {
    sqlx::query("SELECT id FROM products WHERE workspace_id=$1 AND id=ANY($2) AND deleted_at IS NULL ORDER BY id FOR UPDATE")
        .bind(workspace).bind(ids).execute(conn).await?;
    Ok(())
}
