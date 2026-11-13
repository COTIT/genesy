use super::{
    dto::{self, DefinitionRequest, FieldDefinition, FieldFilter, TypedValue, ValidatedFilter},
    repo,
};
use crate::{AppError, modules::products::service::lock_identity};
use chrono::Utc;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, PgPool};
use std::collections::HashSet;
use uuid::Uuid;

pub async fn list(
    db: &PgPool,
    ws: Uuid,
    status: Option<&str>,
) -> Result<Vec<FieldDefinition>, AppError> {
    if status.is_some_and(|s| !["draft", "active", "archived"].contains(&s)) {
        return Err(AppError::field(
            "status",
            "Choose draft, active or archived.",
        ));
    }
    let mut conn = db.acquire().await?;
    Ok(repo::definitions(&mut conn, ws, false)
        .await?
        .into_iter()
        .filter(|d| status.is_none_or(|s| s == d.status))
        .collect())
}
pub async fn get(db: &PgPool, ws: Uuid, id: Uuid) -> Result<FieldDefinition, AppError> {
    list(db, ws, None)
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or(AppError::NotFound)
}
pub async fn save(
    db: &PgPool,
    ws: Uuid,
    id: Option<Uuid>,
    r: DefinitionRequest,
) -> Result<FieldDefinition, AppError> {
    let mut tx = db.begin().await?;
    lock_identity(&mut tx, ws).await?;
    let defs = repo::definitions(&mut tx, ws, true).await?;
    let prior = id
        .map(|id| {
            defs.iter()
                .find(|d| d.id == id)
                .cloned()
                .ok_or(AppError::NotFound)
        })
        .transpose()?;
    let now = Utc::now();
    let mut d = prior.clone().unwrap_or(FieldDefinition {
        id: Uuid::now_v7(),
        key: String::new(),
        label: String::new(),
        kind: String::new(),
        status: "draft".into(),
        required: false,
        nullable: true,
        max_length: 16384,
        min: None,
        max: None,
        options: vec![],
        used: false,
        revision: 1,
        created_at: now,
        updated_at: now,
    });
    if let Some(value) = r.key {
        d.key = value;
    }
    if let Some(value) = r.label {
        d.label = value;
    }
    if let Some(value) = r.kind {
        d.kind = value;
    }
    if let Some(value) = r.status {
        d.status = value;
    }
    if let Some(value) = r.required {
        d.required = value;
    }
    if let Some(value) = r.nullable {
        d.nullable = value;
    }
    if let Some(value) = r.max_length {
        d.max_length = value;
    }
    if let Some(value) = r.min {
        d.min = if value.is_null() {
            None
        } else {
            Some(
                dto::decimal(&value)
                    .map_err(|e| AppError::field("min", e.to_string()))?
                    .normalize()
                    .to_string(),
            )
        };
    }
    if let Some(value) = r.max {
        d.max = if value.is_null() {
            None
        } else {
            Some(
                dto::decimal(&value)
                    .map_err(|e| AppError::field("max", e.to_string()))?
                    .normalize()
                    .to_string(),
            )
        };
    }
    if let Some(value) = r.options {
        d.options = value;
    }
    validate_definition(&d)?;
    if defs
        .iter()
        .any(|other| other.id != d.id && other.key == d.key)
    {
        return Err(AppError::field("key", "The key has already been taken."));
    }
    if let Some(old) = &prior {
        if old.status == "archived" {
            return Err(AppError::field(
                "status",
                "Archived definitions cannot be changed.",
            ));
        }
        if old.used && old.kind != d.kind {
            return Err(AppError::field(
                "type",
                "The type cannot change after values have been used.",
            ));
        }
        if old.key != d.key {
            return Err(AppError::field("key", "The field key is immutable."));
        }
        // A definition edit must not invalidate values already stored, including archived products.
        let allowed: Vec<Uuid> = d.options.iter().map(|o| o.id).collect();
        let invalid:i64=sqlx::query_scalar("SELECT count(*) FROM product_field_values WHERE workspace_id=$1 AND field_id=$2 AND ((type='text' AND char_length(value_text)>$3) OR (type='number' AND (($4::numeric IS NOT NULL AND value_number<$4) OR ($5::numeric IS NOT NULL AND value_number>$5))) OR (type='select' AND NOT(value_option=ANY($6))) OR (type='multiselect' AND NOT(value_options<@$6::uuid[])))")
            .bind(ws).bind(d.id).bind(d.max_length).bind(d.min.as_deref().and_then(|v|v.parse::<rust_decimal::Decimal>().ok())).bind(d.max.as_deref().and_then(|v|v.parse::<rust_decimal::Decimal>().ok())).bind(allowed).fetch_one(&mut *tx).await?;
        if invalid > 0 {
            return Err(AppError::field(
                "options",
                "These constraints would invalidate existing values.",
            ));
        }
    }
    if d.required && d.status == "active" {
        let count = repo::missing(&mut tx, ws, d.id).await?;
        if count > 0 {
            return Err(AppError::field(
                "required",
                format!("Populate this field on {count} live products before activating required."),
            ));
        }
    }
    repo::save(&mut tx, ws, &d).await?;
    let saved = repo::definitions(&mut tx, ws, false)
        .await?
        .into_iter()
        .find(|f| f.id == d.id)
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(saved)
}
fn validate_definition(d: &FieldDefinition) -> Result<(), AppError> {
    if d.key.is_empty()
        || d.key.len() > 100
        || !d
            .key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AppError::field(
            "key",
            "Use 1 to 100 ASCII letters, digits, underscores or hyphens.",
        ));
    }
    if d.label.trim().is_empty() || d.label.chars().count() > 255 {
        return Err(AppError::field(
            "label",
            "Use a label of 1 to 255 characters.",
        ));
    }
    if !dto::TYPES.contains(&d.kind.as_str()) {
        return Err(AppError::field(
            "type",
            "Choose text, number, boolean, date, select or multiselect.",
        ));
    }
    if !["draft", "active", "archived"].contains(&d.status.as_str()) {
        return Err(AppError::field(
            "status",
            "Choose draft, active or archived.",
        ));
    }
    if !(1..=16384).contains(&d.max_length) {
        return Err(AppError::field(
            "max_length",
            "Choose 1 through 16384 characters.",
        ));
    }
    if let (Some(min), Some(max)) = (&d.min, &d.max)
        && dto::decimal(&json!(min))? > dto::decimal(&json!(max))?
    {
        return Err(AppError::field(
            "max",
            "The maximum must not be less than the minimum.",
        ));
    }
    if d.kind != "number" && (d.min.is_some() || d.max.is_some()) {
        return Err(AppError::field(
            "min",
            "Number limits require a number field.",
        ));
    }
    let choices = ["select", "multiselect"].contains(&d.kind.as_str());
    if (choices && d.options.is_empty())
        || (!choices && !d.options.is_empty())
        || d.options.len() > 200
    {
        return Err(AppError::field(
            "options",
            "Select fields need 1 to 200 options; other fields have none.",
        ));
    }
    let mut ids = HashSet::new();
    let mut labels = HashSet::new();
    for o in &d.options {
        if o.id.is_nil()
            || !ids.insert(o.id)
            || o.label.trim().is_empty()
            || o.label.chars().count() > 255
            || !labels.insert(o.label.clone())
        {
            return Err(AppError::field(
                "options",
                "Options need unique UUIDs and unique nonempty labels up to 255 characters.",
            ));
        }
    }
    Ok(())
}
/// Validate a patch and every active required field before writing any values.
async fn validate_patch(
    conn: &mut PgConnection,
    ws: Uuid,
    product: Option<Uuid>,
    patch: Option<&Map<String, Value>>,
) -> Result<(Value, Vec<(FieldDefinition, Option<TypedValue>)>), AppError> {
    let defs = repo::definitions(conn, ws, true).await?;
    let mut current = if let Some(product) = product {
        repo::values(&mut *conn, ws, &[product])
            .await?
            .remove(&product)
            .unwrap_or(json!({}))
    } else {
        json!({})
    };
    let mut seen = HashSet::new();
    let mut writes = Vec::new();
    for (key, value) in patch.into_iter().flatten() {
        let id = Uuid::parse_str(key)
            .map_err(|_| AppError::field(&format!("custom_fields.{key}"), "Use a field UUID."))?;
        if !seen.insert(id) {
            return Err(AppError::field(
                "custom_fields",
                "A field UUID is listed twice.",
            ));
        }
        let def = defs.iter().find(|d| d.id == id).ok_or_else(|| {
            AppError::field(
                &format!("custom_fields.{key}"),
                "The field does not belong to this workspace.",
            )
        })?;
        if def.status == "archived" {
            return Err(AppError::field(
                &format!("custom_fields.{key}"),
                "Archived fields cannot receive assignments.",
            ));
        }
        let value = if value.is_null() {
            if !def.nullable || (def.required && def.status == "active") {
                return Err(AppError::field(
                    &format!("custom_fields.{key}"),
                    "This field cannot be cleared.",
                ));
            }
            current.as_object_mut().unwrap().remove(&id.to_string());
            None
        } else {
            let typed = dto::typed(def, value)
                .map_err(|e| AppError::field(&format!("custom_fields.{key}"), e.to_string()))?;
            current[&id.to_string()] = value.clone();
            Some(typed)
        };
        writes.push((def.clone(), value));
    }
    for d in defs.iter().filter(|d| d.required && d.status == "active") {
        if current
            .get(d.id.to_string())
            .is_none_or(|v| v.is_null() || v.as_array().is_some_and(Vec::is_empty))
        {
            return Err(AppError::field(
                &format!("custom_fields.{}", d.id),
                "This field is required.",
            ));
        }
    }
    Ok((current, writes))
}
pub async fn project(
    conn: &mut PgConnection,
    ws: Uuid,
    product: Option<Uuid>,
    patch: Option<&Map<String, Value>>,
) -> Result<Value, AppError> {
    Ok(validate_patch(conn, ws, product, patch).await?.0)
}
pub async fn apply(
    conn: &mut PgConnection,
    ws: Uuid,
    product: Uuid,
    patch: Option<&Map<String, Value>>,
) -> Result<(), AppError> {
    let (_, writes) = validate_patch(conn, ws, Some(product), patch).await?;
    for (def, value) in writes {
        repo::write(conn, ws, product, &def, value).await?;
    }
    Ok(())
}
/// Native duplication preserves archived values without reopening public assignment.
pub async fn copy(
    conn: &mut PgConnection,
    ws: Uuid,
    source: Uuid,
    target: Uuid,
) -> Result<(), AppError> {
    sqlx::query("INSERT INTO product_field_values SELECT workspace_id,$3,field_id,type,value_text,value_number,value_boolean,value_date,value_option,value_options FROM product_field_values WHERE workspace_id=$1 AND product_id=$2 ON CONFLICT DO NOTHING").bind(ws).bind(source).bind(target).execute(conn).await?;
    Ok(())
}
pub async fn validate_filters(
    conn: &mut PgConnection,
    ws: Uuid,
    filters: &[FieldFilter],
) -> Result<Vec<ValidatedFilter>, AppError> {
    if filters.len() > 20 {
        return Err(AppError::field("custom_filters", "Use at most 20 filters."));
    }
    let defs = repo::definitions(conn, ws, false).await?;
    let mut out = Vec::new();
    for f in filters {
        let d = defs
            .iter()
            .find(|d| d.id == f.field_id)
            .ok_or_else(|| AppError::field("custom_filters", "Unknown field of this workspace."))?;
        let op = f.operator.as_str();
        let scalar = op == "eq" && d.kind != "multiselect";
        let range = ["lt", "lte", "gt", "gte"].contains(&op)
            && ["number", "date"].contains(&d.kind.as_str());
        let contains = op == "contains" && ["text", "multiselect"].contains(&d.kind.as_str());
        if op != "is_set"
            && !scalar
            && !range
            && !contains
            && !(op == "any" && d.kind == "multiselect")
        {
            return Err(AppError::field(
                "custom_filters",
                "The operator is not supported for this field type.",
            ));
        }
        let mut value = TypedValue {
            text: None,
            number: None,
            boolean: None,
            date: None,
            option: None,
            options: None,
        };
        let mut is_set = true;
        if op == "is_set" {
            is_set = f
                .value
                .as_ref()
                .map(|v| {
                    v.as_bool().ok_or_else(|| {
                        AppError::field("custom_filters", "is_set needs true or false.")
                    })
                })
                .transpose()?
                .unwrap_or(true);
        } else {
            let input = f
                .value
                .as_ref()
                .ok_or_else(|| AppError::field("custom_filters", "A filter value is required."))?;
            let input = if d.kind == "multiselect" && op == "contains" {
                json!([input])
            } else {
                input.clone()
            };
            let mut filter_definition = d.clone();
            filter_definition.min = None;
            filter_definition.max = None;
            filter_definition.max_length = 16384;
            value = dto::typed(&filter_definition, &input)
                .map_err(|e| AppError::field("custom_filters", e.to_string()))?;
            if value.options.as_ref().is_some_and(Vec::is_empty) {
                return Err(AppError::field(
                    "custom_filters",
                    "Choose at least one option.",
                ));
            }
        }
        out.push(ValidatedFilter {
            field_id: d.id,
            kind: d.kind.clone(),
            operator: f.operator.clone(),
            value,
            is_set,
        });
    }
    Ok(out)
}
