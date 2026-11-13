use super::dto::{FieldDefinition, TypedValue};
use crate::AppError;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgExecutor, Row};
use std::collections::HashMap;
use uuid::Uuid;

pub async fn definitions(
    conn: &mut PgConnection,
    ws: Uuid,
    lock: bool,
) -> Result<Vec<FieldDefinition>, AppError> {
    let mut query = sqlx::QueryBuilder::new(
        "SELECT to_jsonb(d)-'workspace_id'||jsonb_build_object('min',min::text,'max',max::text) AS definition FROM product_field_definitions d WHERE workspace_id=",
    );
    query.push_bind(ws).push(" ORDER BY id");
    if lock {
        query.push(" FOR SHARE");
    }
    let rows = query.build().fetch_all(conn).await?;
    rows.into_iter()
        .map(|r| {
            serde_json::from_value(r.get("definition"))
                .map_err(|e| AppError::Internal(e.to_string()))
        })
        .collect()
}
pub async fn save(conn: &mut PgConnection, ws: Uuid, d: &FieldDefinition) -> Result<(), AppError> {
    sqlx::query("INSERT INTO product_field_definitions(id,workspace_id,key,label,type,status,required,nullable,max_length,min,max,options) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::text::numeric,$11::text::numeric,$12) ON CONFLICT(id) DO UPDATE SET key=excluded.key,label=excluded.label,type=excluded.type,status=excluded.status,required=excluded.required,nullable=excluded.nullable,max_length=excluded.max_length,min=excluded.min,max=excluded.max,options=excluded.options,revision=product_field_definitions.revision+1,updated_at=now() WHERE product_field_definitions.workspace_id=excluded.workspace_id")
        .bind(d.id).bind(ws).bind(&d.key).bind(&d.label).bind(&d.kind).bind(&d.status).bind(d.required).bind(d.nullable).bind(d.max_length).bind(&d.min).bind(&d.max).bind(json!(d.options)).execute(conn).await?;
    Ok(())
}
pub async fn values(
    db: impl PgExecutor<'_>,
    ws: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, AppError> {
    let rows=sqlx::query("SELECT product_id,jsonb_object_agg(field_id::text,CASE type WHEN 'text' THEN to_jsonb(value_text) WHEN 'number' THEN to_jsonb(trim_scale(value_number)::text) WHEN 'boolean' THEN to_jsonb(value_boolean) WHEN 'date' THEN to_jsonb(value_date::text) WHEN 'select' THEN to_jsonb(value_option) WHEN 'multiselect' THEN to_jsonb(value_options) END) AS fields FROM product_field_values WHERE workspace_id=$1 AND product_id=ANY($2) GROUP BY product_id").bind(ws).bind(ids).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("product_id"), r.get("fields")))
        .collect())
}
pub async fn write(
    conn: &mut PgConnection,
    ws: Uuid,
    product: Uuid,
    def: &FieldDefinition,
    v: Option<TypedValue>,
) -> Result<(), AppError> {
    if let Some(v) = v {
        sqlx::query("INSERT INTO product_field_values(workspace_id,product_id,field_id,type,value_text,value_number,value_boolean,value_date,value_option,value_options) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(workspace_id,product_id,field_id) DO UPDATE SET type=excluded.type,value_text=excluded.value_text,value_number=excluded.value_number,value_boolean=excluded.value_boolean,value_date=excluded.value_date,value_option=excluded.value_option,value_options=excluded.value_options")
            .bind(ws).bind(product).bind(def.id).bind(&def.kind).bind(v.text).bind(v.number).bind(v.boolean).bind(v.date).bind(v.option).bind(v.options).execute(&mut *conn).await?;
        sqlx::query("UPDATE product_field_definitions SET used=true WHERE workspace_id=$1 AND id=$2 AND NOT used").bind(ws).bind(def.id).execute(conn).await?;
    } else {
        sqlx::query("DELETE FROM product_field_values WHERE workspace_id=$1 AND product_id=$2 AND field_id=$3").bind(ws).bind(product).bind(def.id).execute(conn).await?;
    }
    Ok(())
}
pub async fn missing(conn: &mut PgConnection, ws: Uuid, id: Uuid) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM products p WHERE p.workspace_id=$1 AND p.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM product_field_values v WHERE v.workspace_id=p.workspace_id AND v.product_id=p.id AND v.field_id=$2 AND (v.type<>'multiselect' OR cardinality(v.value_options)>0))").bind(ws).bind(id).fetch_one(conn).await?)
}

/// Append safe indexed predicates; callers use `products p`, and validate first.
pub fn push_filters(
    q: &mut sqlx::QueryBuilder<sqlx::Postgres>,
    filters: &[super::dto::ValidatedFilter],
) {
    for f in filters {
        q.push(if f.operator == "is_set" && !f.is_set {
            " AND NOT EXISTS ("
        } else {
            " AND EXISTS ("
        });
        q.push("SELECT 1 FROM product_field_values v WHERE v.workspace_id=p.workspace_id AND v.product_id=p.id AND v.field_id=").push_bind(f.field_id);
        q.push(" AND v.type=").push(match f.kind.as_str() {
            "text" => "'text'",
            "number" => "'number'",
            "boolean" => "'boolean'",
            "date" => "'date'",
            "select" => "'select'",
            "multiselect" => "'multiselect'",
            _ => unreachable!("validated field type"),
        });
        if f.operator != "is_set" {
            let operator = match f.operator.as_str() {
                "lt" => " < ",
                "lte" => " <= ",
                "gt" => " > ",
                "gte" => " >= ",
                _ => " = ",
            };
            match f.kind.as_str() {
                "text" if f.operator == "contains" => {
                    let text = f
                        .value
                        .text
                        .as_deref()
                        .unwrap_or_default()
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_");
                    q.push(" AND v.value_text ILIKE ")
                        .push_bind(format!("%{text}%"));
                }
                "text" => {
                    q.push(" AND md5(v.value_text)=md5(")
                        .push_bind(&f.value.text)
                        .push(") AND v.value_text=")
                        .push_bind(&f.value.text);
                }
                "number" => {
                    q.push(" AND v.value_number")
                        .push(operator)
                        .push_bind(f.value.number);
                }
                "date" => {
                    q.push(" AND v.value_date")
                        .push(operator)
                        .push_bind(f.value.date);
                }
                "boolean" => {
                    q.push(" AND v.value_boolean=").push_bind(f.value.boolean);
                }
                "select" => {
                    q.push(" AND v.value_option=").push_bind(f.value.option);
                }
                "multiselect" => {
                    q.push(" AND v.value_options ")
                        .push(if f.operator == "any" { " && " } else { " @> " })
                        .push_bind(&f.value.options);
                }
                _ => unreachable!("validated field type"),
            }
        }
        q.push(")");
    }
}
