//! Reserve worst-case AI cost before sending bytes. Unknown calls keep their reservation.
use crate::AppError;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn reserve_ai_cost(
    db: &PgPool,
    workspace: Uuid,
    run: Uuid,
    source: Option<Uuid>,
    request_key: &str,
    upper_bound: Decimal,
) -> Result<Uuid, AppError> {
    if upper_bound <= Decimal::ZERO || request_key.is_empty() || request_key.len() > 200 {
        return Err(AppError::field(
            "budget",
            "Use a positive cost bound and a stable request key.",
        ));
    }
    let mut tx = db.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('product_bulk_budget:'||$1::uuid::text,0))",
    )
    .bind(workspace)
    .execute(&mut *tx)
    .await?;
    let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_bulk_runs WHERE workspace_id=$1 AND id=$2 AND cancelled_at IS NULL)").bind(workspace).bind(run).fetch_one(&mut *tx).await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    let old: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM product_bulk_ai_costs WHERE workspace_id=$1 AND request_key=$2",
    )
    .bind(workspace)
    .bind(request_key)
    .fetch_optional(&mut *tx)
    .await?;
    if old.is_some() {
        return Err(AppError::Conflict(
            "This AI request already has a reservation. Check its result before retrying.",
        ));
    }
    let cap:Decimal=sqlx::query_scalar("SELECT coalesce((SELECT per_run_cap_usd FROM product_bulk_settings WHERE workspace_id=$1),0::numeric)").bind(workspace).fetch_one(&mut *tx).await?;
    let spent:Decimal=sqlx::query_scalar("SELECT coalesce(sum(coalesce(actual_usd,upper_bound_usd)),0::numeric) FROM product_bulk_ai_costs WHERE workspace_id=$1 AND run_id=$2").bind(workspace).bind(run).fetch_one(&mut *tx).await?;
    if spent.checked_add(upper_bound).is_none_or(|n| n > cap) {
        return Err(AppError::Conflict(
            "AI spending limit reached. Ask a workspace administrator to set a sufficient per-run USD cap.",
        ));
    }
    let id=sqlx::query_scalar("INSERT INTO product_bulk_ai_costs(workspace_id,run_id,source_id,request_key,upper_bound_usd) VALUES($1,$2,$3,$4,$5) RETURNING id").bind(workspace).bind(run).bind(source).bind(request_key).bind(upper_bound).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn settle_ai_cost(
    db: &PgPool,
    workspace: Uuid,
    reservation: Uuid,
    actual: Decimal,
    usage: &Value,
) -> Result<(), AppError> {
    if actual < Decimal::ZERO {
        return Err(AppError::field("budget", "Cost cannot be negative."));
    }
    let changed=sqlx::query("UPDATE product_bulk_ai_costs SET actual_usd=$3,usage=$4,state='settled',settled_at=now() WHERE workspace_id=$1 AND id=$2 AND state='reserved'").bind(workspace).bind(reservation).bind(actual).bind(usage).execute(db).await?.rows_affected();
    if changed != 1 {
        return Err(AppError::Conflict(
            "This AI reservation was already settled or is unavailable.",
        ));
    }
    Ok(())
}
