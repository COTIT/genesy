use crate::AppError;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

pub const TYPES: [&str; 6] = ["text", "number", "boolean", "date", "select", "multiselect"];
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FieldOption {
    pub id: Uuid,
    pub label: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct FieldDefinition {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub required: bool,
    pub nullable: bool,
    pub max_length: i32,
    pub min: Option<String>,
    pub max: Option<String>,
    pub options: Vec<FieldOption>,
    pub used: bool,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DefinitionRequest {
    pub key: Option<String>,
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub status: Option<String>,
    pub required: Option<bool>,
    pub nullable: Option<bool>,
    pub max_length: Option<i32>,
    #[serde(default, deserialize_with = "present_value")]
    #[schema(value_type=Option<String>)]
    pub min: Option<Value>,
    #[serde(default, deserialize_with = "present_value")]
    #[schema(value_type=Option<String>)]
    pub max: Option<Value>,
    pub options: Option<Vec<FieldOption>>,
}
#[derive(Clone, Debug)]
pub struct TypedValue {
    pub text: Option<String>,
    pub number: Option<Decimal>,
    pub boolean: Option<bool>,
    pub date: Option<NaiveDate>,
    pub option: Option<Uuid>,
    pub options: Option<Vec<Uuid>>,
}
impl TypedValue {
    fn empty() -> Self {
        Self {
            text: None,
            number: None,
            boolean: None,
            date: None,
            option: None,
            options: None,
        }
    }
}
pub fn decimal(value: &Value) -> Result<Decimal, AppError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid("Use an exact decimal string."))?;
    let body = text.strip_prefix('-').unwrap_or(text);
    let parts: Vec<_> = body.split('.').collect();
    if parts.len() > 2
        || parts[0].is_empty()
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("Use an exact decimal string."));
    }
    let number: Decimal = text
        .parse()
        .map_err(|_| invalid("The number is out of range."))?;
    if number.scale() > 8 || number.abs() > crate::money::MAX {
        return Err(invalid("The number is out of range."));
    }
    Ok(number)
}
fn invalid(message: &str) -> AppError {
    AppError::field("custom_fields", message)
}
pub fn typed(def: &FieldDefinition, value: &Value) -> Result<TypedValue, AppError> {
    let mut out = TypedValue::empty();
    match def.kind.as_str() {
        "text" => {
            let value = value
                .as_str()
                .filter(|v| !v.is_empty() && v.chars().count() <= def.max_length as usize)
                .ok_or_else(|| invalid("Use nonempty text within the field length limit."))?;
            out.text = Some(value.to_owned());
        }
        "number" => {
            let value = decimal(value)?;
            let min = def.min.as_deref().and_then(|v| v.parse::<Decimal>().ok());
            let max = def.max.as_deref().and_then(|v| v.parse::<Decimal>().ok());
            if min.is_some_and(|n| value < n) || max.is_some_and(|n| value > n) {
                return Err(invalid("The number is outside the field limits."));
            }
            out.number = Some(value);
        }
        "boolean" => {
            out.boolean = Some(
                value
                    .as_bool()
                    .ok_or_else(|| invalid("Use true or false."))?,
            )
        }
        "date" => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid("Use an ISO date (YYYY-MM-DD)."))?;
            let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| invalid("Use an ISO date (YYYY-MM-DD)."))?;
            if text.len() != 10 || date.format("%Y-%m-%d").to_string() != text {
                return Err(invalid("Use an ISO date (YYYY-MM-DD)."));
            }
            out.date = Some(date);
        }
        "select" | "multiselect" => {
            let values = if def.kind == "select" {
                vec![value]
            } else {
                value
                    .as_array()
                    .ok_or_else(|| invalid("Use an array of option UUIDs."))?
                    .iter()
                    .collect()
            };
            if values.len() > 200 {
                return Err(invalid("Choose at most 200 options."));
            }
            let mut ids = Vec::new();
            for value in values {
                let id = value
                    .as_str()
                    .and_then(|v| Uuid::parse_str(v).ok())
                    .filter(|id| def.options.iter().any(|o| o.id == *id))
                    .ok_or_else(|| invalid("Choose an option of this field."))?;
                if ids.contains(&id) {
                    return Err(invalid("An option is listed twice."));
                }
                ids.push(id);
            }
            if def.kind == "select" {
                out.option = ids.first().copied();
            } else {
                ids.sort();
                out.options = Some(ids);
            }
        }
        _ => return Err(invalid("Unknown field type.")),
    }
    Ok(out)
}
#[derive(Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldFilter {
    pub field_id: Uuid,
    pub operator: String,
    pub value: Option<Value>,
}
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in=Query)]
pub struct ListQuery {
    pub status: Option<String>,
}

fn present_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Clone, Debug)]
pub struct ValidatedFilter {
    pub field_id: Uuid,
    pub kind: String,
    pub operator: String,
    pub value: TypedValue,
    pub is_set: bool,
}

impl crate::http::json::Validate for DefinitionRequest {
    type Valid = Self;
    fn validate(self, _: &mut crate::http::json::Rules) -> Option<Self> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn field(kind: &str) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            key: "test".into(),
            label: "Test".into(),
            kind: kind.into(),
            status: "active".into(),
            required: false,
            nullable: true,
            max_length: 4,
            min: Some("1".into()),
            max: Some("10".into()),
            options: vec![],
            used: false,
            revision: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
    #[test]
    fn exact_numbers_reject_precision_and_non_decimal_input() {
        for v in [
            json!(1),
            json!("1e2"),
            json!("NaN"),
            json!("1,5"),
            json!("0.000000001"),
            json!("+1"),
        ] {
            assert!(decimal(&v).is_err(), "{v}");
        }
        assert_eq!(
            decimal(&json!("-1.2500")).unwrap().normalize().to_string(),
            "-1.25"
        );
        assert!(typed(&field("number"), &json!("0")).is_err());
        assert!(typed(&field("number"), &json!("10.00000001")).is_err());
    }
    #[test]
    fn typed_values_keep_false_and_reject_invalid_dates_and_choices() {
        assert_eq!(
            typed(&field("boolean"), &json!(false)).unwrap().boolean,
            Some(false)
        );
        assert!(typed(&field("boolean"), &json!("false")).is_err());
        for value in ["2026-2-01", "2026-02-30", "2026-01-01T00:00:00Z"] {
            assert!(typed(&field("date"), &json!(value)).is_err());
        }
        assert!(typed(&field("date"), &json!("2026-02-01")).is_ok());
        assert!(typed(&field("text"), &json!("12345")).is_err());
        let mut f = field("multiselect");
        let id = Uuid::now_v7();
        f.options = vec![FieldOption {
            id,
            label: "Red".into(),
        }];
        assert!(typed(&f, &json!([id, id])).is_err());
        assert!(typed(&f, &json!([Uuid::now_v7()])).is_err());
        assert_eq!(typed(&f, &json!([id])).unwrap().options, Some(vec![id]));
    }
    #[test]
    fn request_distinguishes_omitted_and_null_limits() {
        let absent: DefinitionRequest = serde_json::from_value(json!({})).unwrap();
        let clear: DefinitionRequest = serde_json::from_value(json!({"min":null})).unwrap();
        assert!(absent.min.is_none());
        assert_eq!(clear.min, Some(Value::Null));
    }
}
