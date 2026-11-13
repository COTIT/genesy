//! Native workspace routes; imported content cannot approve its own changes.
use super::{dto::*, service};
use crate::{
    AppError, AppState,
    error::UnprocessableBody,
    http::{
        json::ValidatedJson as Json,
        response::{Data, Envelope, MessageBody},
        workspace::WorkspaceAccess,
    },
};
use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;
type RunPath = Path<(Uuid, Uuid)>;
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct ProductBulkSourceUpload {
    #[schema(value_type=String,format=Binary)]
    file: String,
}

#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs",tag="product_bulk",operation_id="product_bulk_runs_list",params(("workspace"=Uuid,Path),RowsQuery),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn list(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Query(q): Query<RowsQuery>,
) -> Result<Data<Value>, AppError> {
    service::read(&access)?;
    let limit = q.page_size.unwrap_or(25).clamp(1, 100);
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)-'mapping'-'selection'-'selection_cursor'-'prepare_cursor' FROM product_bulk_runs r WHERE workspace_id=$1 AND ($2 OR created_by=$3) AND ($4::bigint IS NULL OR extract(epoch FROM created_at)*1000000<$4) ORDER BY created_at DESC,id DESC LIMIT $5").bind(access.workspace_id).bind(access.is_admin()).bind(access.user_id).bind(q.after).bind(limit).fetch_all(&state.db).await?;
    let next = if rows.len() == limit as usize {
        rows.last()
            .and_then(|r| r["created_at"].as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_micros())
    } else {
        None
    };
    Ok(Data(json!({"runs":rows,"next":next})))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs",tag="product_bulk",operation_id="product_bulk_runs_create",params(("workspace"=Uuid,Path)),request_body=CreateInput,responses((status=201,body=Envelope<Value>,description="Product bulk run created"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn create(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Json(input): Json<CreateInput>,
) -> Result<(StatusCode, Data<Value>), AppError> {
    Ok((
        StatusCode::CREATED,
        Data(service::create(&state, &access, input).await?),
    ))
}
#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/{run}",tag="product_bulk",operation_id="product_bulk_runs_get",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn get(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
) -> Result<Data<Value>, AppError> {
    service::owned(&state.db, &access, id).await?;
    Ok(Data(service::with_permissions(
        service::get(&state.db, access.workspace_id, id).await?,
        &access,
    )))
}

#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/sources",tag="product_bulk",operation_id="product_bulk_runs_source",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body(content=ProductBulkSourceUpload,content_type="multipart/form-data"),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn source(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    mut form: Multipart,
) -> Result<Data<Value>, AppError> {
    service::owned(&state.db, &access, id).await?;
    service::enabled(&state)?;
    let _slot = state.slots.heavy(access.workspace_id).await?;
    let mut file = None;
    while let Some(mut part) = form
        .next_field()
        .await
        .map_err(|_| AppError::field("file", "Invalid source upload."))?
    {
        if part.name() != Some("file") || file.is_some() {
            return Err(AppError::field("file", "Upload exactly one file field."));
        }
        let name = part.file_name().unwrap_or("source").to_owned();
        let declared = part.content_type().unwrap_or("").to_owned();
        let mut bytes = Vec::new();
        while let Some(chunk) = part
            .chunk()
            .await
            .map_err(|_| AppError::field("file", "The source upload was interrupted."))?
        {
            if bytes.len() + chunk.len() > MAX_SOURCE_BYTES {
                return Err(AppError::PayloadTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let media = sniff(&bytes, &name, &declared)?;
        file = Some((name, media, bytes));
    }
    let (name, media, bytes) = file.ok_or(AppError::field(
        "file",
        "Choose one PDF, XLSX or CSV source.",
    ))?;
    let mut tx = state.db.begin().await?;
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(access.workspace_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    service::lock_edit(&mut tx, access.workspace_id, id, revision).await?;
    let source = service::save_source(
        &state,
        &mut tx,
        access.workspace_id,
        id,
        &name,
        &media,
        &bytes,
    )
    .await?;
    service::invalidate(&mut tx, access.workspace_id, id).await?;
    tx.commit().await?;
    service::finish_source(&state, source, bytes).await?;
    Ok(Data(service::with_permissions(
        service::get(&state.db, access.workspace_id, id).await?,
        &access,
    )))
}
fn sniff(bytes: &[u8], name: &str, declared: &str) -> Result<String, AppError> {
    let lower = name.to_lowercase();
    if bytes.starts_with(b"%PDF-") && lower.ends_with(".pdf") {
        return Ok("application/pdf".into());
    }
    if bytes.starts_with(b"PK\x03\x04") && lower.ends_with(".xlsx") {
        return Ok("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into());
    }
    if lower.ends_with(".csv")
        && !bytes.starts_with(b"PK")
        && !bytes.starts_with(&[0xd0, 0xcf, 0x11])
        && std::str::from_utf8(bytes).is_ok()
        && (!declared.starts_with("image/") && declared != "application/pdf")
    {
        return Ok("text/csv".into());
    }
    Err(AppError::field(
        "file",
        "Use a matching PDF, XLSX or UTF-8 CSV file; legacy XLS and macro workbooks are not supported.",
    ))
}

#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/stage",tag="product_bulk",operation_id="product_bulk_runs_stage",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body=StageInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn stage(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    Json(input): Json<StageInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::stage(&state, &access, id, input).await?))
}
#[utoipa::path(patch,path="/workspace/{workspace}/product-bulk-runs/{run}/mapping",tag="product_bulk",operation_id="product_bulk_runs_mapping",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body=MappingInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn mapping(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    Json(input): Json<MappingInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::mapping(&state, &access, id, input).await?))
}

#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/{run}/rows",tag="product_bulk",operation_id="product_bulk_runs_rows",params(("workspace"=Uuid,Path),("run"=Uuid,Path),RowsQuery),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn rows(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    Query(q): Query<RowsQuery>,
) -> Result<Data<Value>, AppError> {
    service::owned(&state.db, &access, id).await?;
    let limit = q.page_size.unwrap_or(50).clamp(1, 100);
    if q.q.as_ref().is_some_and(|s| s.len() > 255) {
        return Err(AppError::field("q", "Search text cannot exceed 255 bytes."));
    }
    let fetched:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r)-'workspace_id'-'run_id' FROM product_bulk_rows r WHERE workspace_id=$1 AND run_id=$2 AND ordinal>$3 AND ($4::text IS NULL OR status=$4) AND ($5::text IS NULL OR strpos(lower(target::text||values::text),lower($5))>0) ORDER BY ordinal LIMIT $6").bind(access.workspace_id).bind(id).bind(q.after.unwrap_or(0)).bind(q.status).bind(q.q).bind(limit).fetch_all(&state.db).await?;
    let fetched_len = fetched.len();
    let mut rows = Vec::new();
    let mut budget = 0usize;
    let mut limited = false;
    for mut row in fetched {
        let material = row
            .as_object_mut()
            .unwrap()
            .remove("material")
            .unwrap_or(Value::Null);
        let before = &material["native"]["before"];
        row["before"] = public_projection(
            before.get("product").unwrap_or(before),
            before.get("custom_fields"),
            &row["values"],
            before,
        );
        row["after"] = public_projection(
            &material["native"]["projected"],
            material["native"].get("custom_fields"),
            &row["values"],
            before,
        );
        if let Some(raw) = row.get_mut("raw").filter(|v| v.is_object()) {
            let cells = raw
                .get("cells")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let clipped = cells.len() > 8
                || cells
                    .iter()
                    .any(|v| v.as_str().is_some_and(|s| s.chars().count() > 256));
            raw["cells"] = json!(
                cells
                    .iter()
                    .take(8)
                    .map(|v| Value::String(v.as_str().unwrap_or("").chars().take(256).collect()))
                    .collect::<Vec<_>>()
            );
            if let Some(cols) = raw.get("columns").and_then(Value::as_array) {
                raw["columns"] = json!(
                    cols.iter()
                        .take(8)
                        .map(|v| Value::String(
                            v.as_str().unwrap_or("").chars().take(256).collect()
                        ))
                        .collect::<Vec<_>>()
                );
            }
            row["raw_truncated"] = json!(clipped);
        }
        let size = serde_json::to_vec(&row)
            .map_err(|_| AppError::Internal("Invalid review row".into()))?
            .len();
        if size > 2 * 1024 * 1024 - 65536 {
            return Err(AppError::field(
                "row",
                "This row exceeds the 2 MiB review response limit. Reduce its fields in a fresh staged run.",
            ));
        }
        if budget + size > 2 * 1024 * 1024 - 65536 {
            limited = true;
            break;
        }
        budget += size;
        rows.push(row);
    }
    let next = if limited || fetched_len == limit as usize {
        rows.last().and_then(|r| r["ordinal"].as_i64())
    } else {
        None
    };
    let total: i64 = sqlx::query_scalar("SELECT row_count FROM product_bulk_runs WHERE id=$1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(Data(json!({"rows":rows,"next":next,"total":total})))
}
fn public_projection(
    product: &Value,
    custom: Option<&Value>,
    changes: &Value,
    context: &Value,
) -> Value {
    if product.is_null() {
        return Value::Null;
    }
    let mut out = serde_json::Map::new();
    for key in [
        "id",
        "name",
        "currency_id",
        "price",
        "gross_price",
        "order_unit",
        "sale_unit",
    ] {
        if let Some(v) = product.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(changes) = changes.as_object() {
        for (key, value) in changes {
            if key == "custom_fields" {
                let selected = value
                    .as_object()
                    .map(|m| {
                        m.keys()
                            .map(|k| {
                                (
                                    k.clone(),
                                    custom
                                        .and_then(|v| v.get(k))
                                        .cloned()
                                        .unwrap_or(Value::Null),
                                )
                            })
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                out.insert(key.clone(), Value::Object(selected));
            } else {
                out.insert(
                    key.clone(),
                    product
                        .get(if key == "type" { "kind" } else { key })
                        .cloned()
                        .unwrap_or(Value::Null),
                );
            }
        }
    }
    if let Some(impact) = context.get("delete_impact") {
        out.insert("delete_impact".into(), impact.clone());
        out.insert("affected_variants".into(), impact["variant_count"].clone());
    } else if let Some(variants) = context.get("inheriting_variants").and_then(Value::as_array) {
        out.insert("affected_variants".into(), json!(variants.len()));
    }
    Value::Object(out)
}

#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/{run}/rows/{row}/raw",tag="product_bulk",operation_id="product_bulk_runs_raw",params(("workspace"=Uuid,Path),("run"=Uuid,Path),("row"=Uuid,Path),RowsQuery),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn raw(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, run, row)): Path<(Uuid, Uuid, Uuid)>,
    Query(q): Query<RowsQuery>,
) -> Result<Data<Value>, AppError> {
    service::owned(&state.db, &access, run).await?;
    let raw: Option<Value> = sqlx::query_scalar(
        "SELECT raw FROM product_bulk_rows WHERE workspace_id=$1 AND run_id=$2 AND id=$3",
    )
    .bind(access.workspace_id)
    .bind(run)
    .bind(row)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let raw = raw.ok_or(AppError::NotFound)?;
    let cells = raw["cells"].as_array().ok_or(AppError::NotFound)?;
    let after = q.after.unwrap_or(0).max(0) as usize;
    let limit = q.page_size.unwrap_or(8).clamp(1, 8) as usize;
    let end = cells.len().min(after.saturating_add(limit));
    let empty = Vec::new();
    let formulas = raw["formula_columns"].as_array().unwrap_or(&empty);
    let rows:Vec<Value>=cells.iter().enumerate().skip(after).take(limit).map(|(index,value)|json!({"column":raw["columns"].get(index).cloned().unwrap_or(json!(index+1)),"index":index,"value":value,"formula":formulas.iter().any(|v|v.as_u64()==Some(index as u64))})).collect();
    Ok(Data(
        json!({"cells":rows,"next":(end<cells.len()).then_some(end)}),
    ))
}
#[utoipa::path(patch,path="/workspace/{workspace}/product-bulk-runs/{run}/rows/{row}",tag="product_bulk",operation_id="product_bulk_runs_row",params(("workspace"=Uuid,Path),("run"=Uuid,Path),("row"=Uuid,Path)),request_body=RowInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn row(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, run, row)): Path<(Uuid, Uuid, Uuid)>,
    Json(input): Json<RowInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(
        service::edit_row(&state, &access, run, row, input).await?,
    ))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/reviews",tag="product_bulk",operation_id="product_bulk_runs_review",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body=ReviewInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn review(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    Json(input): Json<ReviewInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::review(&state, &access, id, input).await?))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/approve",tag="product_bulk",operation_id="product_bulk_runs_approve",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body=ApproveInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn approve(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
    Json(input): Json<ApproveInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::approve(&state, &access, id, input).await?))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/cancel",tag="product_bulk",operation_id="product_bulk_runs_cancel",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn cancel(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::cancel(&state, &access, id).await?))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/remainder",tag="product_bulk",operation_id="product_bulk_runs_remainder",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn remainder(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, id)): RunPath,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::remainder(&state, &access, id).await?))
}

#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/{run}/resume",tag="product_bulk",operation_id="product_bulk_runs_resume",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),request_body=ResumeInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn resume(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, run)): RunPath,
    Json(input): Json<ResumeInput>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(service::resume(&state, &access, run, input).await?))
}
#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/settings",tag="product_bulk",operation_id="product_bulk_settings_get",params(("workspace"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn settings_get(
    State(state): State<AppState>,
    access: WorkspaceAccess,
) -> Result<Data<Value>, AppError> {
    service::read(&access)?;
    let cap:Decimal=sqlx::query_scalar("SELECT coalesce((SELECT per_run_cap_usd FROM product_bulk_settings WHERE workspace_id=$1),0::numeric)").bind(access.workspace_id).fetch_one(&state.db).await?;
    Ok(Data(
        json!({"per_run_cap_usd":cap.to_string(),"can_edit":access.is_admin()}),
    ))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/settings",tag="product_bulk",operation_id="product_bulk_settings_set",params(("workspace"=Uuid,Path)),request_body=SettingsInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn settings_set(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Json(input): Json<SettingsInput>,
) -> Result<Data<Value>, AppError> {
    if !access.is_admin() {
        return Err(AppError::Forbidden(
            "Only workspace administrators can set AI spending limits.",
        ));
    }
    let cap = Decimal::from_str(&input.per_run_cap_usd)
        .map_err(|_| AppError::field("per_run_cap_usd", "Use an exact nonnegative USD decimal."))?;
    if cap < Decimal::ZERO || cap.scale() > 6 || cap > Decimal::from(1_000_000) {
        return Err(AppError::field(
            "per_run_cap_usd",
            "Use a USD cap from 0 through 1000000 with at most six decimals.",
        ));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('product_bulk_budget:'||$1::uuid::text,0))",
    )
    .bind(access.workspace_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO product_bulk_settings(workspace_id,per_run_cap_usd,updated_by) VALUES($1,$2,$3) ON CONFLICT(workspace_id) DO UPDATE SET per_run_cap_usd=$2,updated_by=$3,updated_at=now()").bind(access.workspace_id).bind(cap).bind(access.user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Data(
        json!({"per_run_cap_usd":cap.to_string(),"can_edit":true}),
    ))
}
#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/mappings",tag="product_bulk",operation_id="product_bulk_mappings_get",params(("workspace"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn mappings_get(
    State(state): State<AppState>,
    access: WorkspaceAccess,
) -> Result<Data<Value>, AppError> {
    service::read(&access)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m)-'workspace_id' FROM product_bulk_mappings m WHERE workspace_id=$1 ORDER BY name,id LIMIT 100").bind(access.workspace_id).fetch_all(&state.db).await?;
    Ok(Data(json!({"mappings":rows})))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-bulk-runs/mappings",tag="product_bulk",operation_id="product_bulk_mappings_save",params(("workspace"=Uuid,Path)),request_body=SavedMappingInput,responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn mappings_save(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Json(input): Json<SavedMappingInput>,
) -> Result<Data<Value>, AppError> {
    service::read(&access)?;
    if !access.allows("product.update") {
        return Err(AppError::Forbidden(
            "Product update permission is required to save shared mapping rules.",
        ));
    }
    if input.name.trim().is_empty()
        || input.name.len() > 255
        || !input.mapping.is_object()
        || input.mapping.to_string().len() > 65536
    {
        return Err(AppError::field(
            "mapping",
            "Use a named mapping object up to 64 KiB.",
        ));
    }
    let row:Value=sqlx::query_scalar("INSERT INTO product_bulk_mappings(workspace_id,name,mapping,created_by) VALUES($1,$2,$3,$4) ON CONFLICT(workspace_id,name) DO UPDATE SET mapping=$3 RETURNING to_jsonb(product_bulk_mappings)-'workspace_id'").bind(access.workspace_id).bind(input.name).bind(input.mapping).bind(access.user_id).fetch_one(&state.db).await?;
    Ok(Data(row))
}
#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/template",tag="product_bulk",operation_id="product_bulk_template",params(("workspace"=Uuid,Path)),responses((status=200,body=Envelope<Value>,description="Product bulk operation result"),(status=400,body=MessageBody,description="Invalid request"),(status=401,body=MessageBody,description="Authentication required"),(status=403,body=MessageBody,description="Missing permission"),(status=404,body=MessageBody,description="Unknown workspace or resource"),(status=409,body=MessageBody,description="Revision or state conflict"),(status=413,body=MessageBody,description="Request too large"),(status=422,body=UnprocessableBody,description="Invalid product bulk input")),security(("bearer"=[])))]
pub async fn template(
    State(state): State<AppState>,
    access: WorkspaceAccess,
) -> Result<Data<Value>, AppError> {
    service::read(&access)?;
    let schema = serde_json::to_value(
        <crate::modules::products::dto::ProductRequestDoc as utoipa::PartialSchema>::schema(),
    )
    .map_err(|_| AppError::Internal("Invalid product schema".into()))?;
    let mut fields:Vec<Value>=schema["properties"].as_object().map(|p|p.iter().filter(|(k,_)|super::extraction::is_native_target(k)).map(|(k,v)|json!({"key":k,"label":k,"type":v.get("type").unwrap_or(&Value::Null),"schema":v})).collect()).unwrap_or_default();
    for (key, label) in [
        ("_operation", "Operation"),
        ("_key", "Exact product key"),
        ("_product_id", "Product UUID"),
        ("_clear", "Explicitly clear fields"),
    ] {
        fields.push(json!({"key":key,"label":label,"type":"string"}));
    }
    let mut conn = state.db.acquire().await?;
    let definitions =
        crate::modules::product_fields::repo::definitions(&mut conn, access.workspace_id, false)
            .await?;
    Ok(Data(
        json!({"schema":schema,"fields":fields,"custom_fields":definitions,"match_keys":["product_nr_formatted","reference","ean13","barcode","name","id"],"limits":{"rows":50000,"sources":10,"source_bytes":MAX_SOURCE_BYTES,"cells":MAX_CELLS,"cell_bytes":MAX_CELL_BYTES}}),
    ))
}

#[utoipa::path(get,path="/workspace/{workspace}/product-bulk-runs/{run}/report",tag="product_bulk",operation_id="product_bulk_report",params(("workspace"=Uuid,Path),("run"=Uuid,Path)),responses((status=200,description="Streaming UTF-8 CSV row receipts")),security(("bearer"=[])))]
pub async fn report(
    State(state): State<AppState>,
    access: WorkspaceAccess,
    Path((_, run)): RunPath,
) -> Result<Response, AppError> {
    service::owned(&state.db, &access, run).await?;
    let workspace = access.workspace_id;
    let stream = futures_util::stream::try_unfold(
        (state.db.clone(), 0i64, false),
        move |(db, after, header)| async move {
            let rows=sqlx::query("SELECT ordinal,operation,source_id,sheet,page,line,target,status,issues,receipt FROM product_bulk_rows WHERE workspace_id=$1 AND run_id=$2 AND ordinal>$3 ORDER BY ordinal LIMIT 500").bind(workspace).bind(run).bind(after).fetch_all(&db).await.map_err(std::io::Error::other)?;
            if rows.is_empty() && header {
                return Ok::<_, std::io::Error>(None);
            }
            let mut out = if header {
                String::new()
            } else {
                "ordinal,operation,source_id,sheet,page,line,target,status,issues,receipt\r\n"
                    .into()
            };
            let mut next = after;
            for r in rows {
                next = r.get("ordinal");
                let values = [
                    next.to_string(),
                    r.get("operation"),
                    r.get::<Option<Uuid>, _>("source_id")
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    r.get::<Option<String>, _>("sheet").unwrap_or_default(),
                    r.get::<Option<i32>, _>("page")
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    r.get::<Option<i32>, _>("line")
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    r.get::<Value, _>("target").to_string(),
                    r.get("status"),
                    r.get::<Value, _>("issues").to_string(),
                    r.get::<Option<Value>, _>("receipt")
                        .unwrap_or(Value::Null)
                        .to_string(),
                ];
                out.push_str(
                    &values
                        .iter()
                        .map(|s| csv_cell(s))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                out.push_str("\r\n");
            }
            Ok(Some((out, (db, next, true))))
        },
    );
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=product-bulk-report.csv",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}
fn csv_cell(value: &str) -> String {
    let guarded = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.into()
    };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delete_projection_displays_all_affected_variants_and_stock() {
        let impact = json!({"variant_count":3,"stock_rows":2,"nonzero_stock_rows":1});
        let out = public_projection(
            &json!({"name":"Tea"}),
            None,
            &json!({}),
            &json!({"inheriting_variants":[{}],"delete_impact":impact}),
        );
        assert_eq!(out["affected_variants"], 3);
        assert_eq!(out["delete_impact"], impact);
        assert!(out.get("stocks").is_none());
    }
    #[test]
    fn source_types_are_content_checked() {
        assert!(sniff(b"not a PDF", "x.pdf", "application/pdf").is_err());
        assert_eq!(sniff(b"a,b\n1,2", "x.csv", "text/csv").unwrap(), "text/csv");
    }
    #[test]
    fn csv_export_neutralizes_formulas() {
        assert_eq!(csv_cell("=1+1"), "\"'=1+1\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
