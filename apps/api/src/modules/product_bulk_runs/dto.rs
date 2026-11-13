//! Bounded import wire contracts; raw sources never authorize mutations.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

pub const MAX_ROWS: i64 = 50_000;
pub const MAX_CELLS: usize = 2_000_000;
pub const MAX_CELL_BYTES: usize = 16 * 1024;
pub const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_INFLATED_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[schema(as = ProductBulkSource)]
pub struct Source {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub run_id: Uuid,
    pub object_key: String,
    pub media_type: String,
    pub name: String,
    pub cursor: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[schema(as = ProductBulkRawRow)]
pub struct RawRow {
    pub source_id: Option<Uuid>,
    pub sheet: Option<String>,
    pub page: Option<u32>,
    pub line: u32,
    pub cells: Vec<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    pub formula_columns: Vec<usize>,
    #[serde(default)]
    pub header_candidate: bool,
    #[serde(default)]
    pub needs_input: Option<String>,
}

#[derive(Debug)]
pub struct ExtractionStep {
    pub rows: Vec<RawRow>,
    pub columns: Vec<String>,
    pub cursor: Value,
    pub done: bool,
    pub total: Option<usize>,
    pub selection: Option<SelectionPlan>,
    pub suggested_mapping: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkSelection)]
pub struct Selection {
    pub ids: Option<Vec<Uuid>>,
    pub category_id: Option<Uuid>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub custom_filters: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkChange)]
pub struct Change {
    pub kind: String,
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkSelectionPlan)]
pub struct SelectionPlan {
    pub selection: Selection,
    pub operation: String,
    #[serde(default)]
    pub changes: std::collections::BTreeMap<String, Change>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkCreateInput)]
pub struct CreateInput {
    pub name: Option<String>,
    pub instruction: Option<String>,
    pub mode: Option<String>,
    pub match_key: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkStageInput)]
pub struct StageInput {
    pub revision: i64,
    pub text: Option<String>,
    #[serde(default)]
    pub rows: Vec<StagedRow>,
    pub selection: Option<Selection>,
    pub operation: Option<String>,
    pub changes: Option<std::collections::BTreeMap<String, Change>>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkStagedRow)]
pub struct StagedRow {
    pub operation: String,
    #[serde(default = "object")]
    pub target: Value,
    #[serde(default = "object")]
    pub values: Value,
    pub source_id: Option<Uuid>,
    pub sheet: Option<String>,
    pub page: Option<u32>,
    pub line: Option<u32>,
    #[serde(default = "yes")]
    pub included: bool,
}
fn object() -> Value {
    serde_json::json!({})
}
fn yes() -> bool {
    true
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkMappingInput)]
pub struct MappingInput {
    pub revision: i64,
    pub source_id: Uuid,
    pub sheet: Option<String>,
    pub header_row: Option<u32>,
    pub columns: std::collections::BTreeMap<String, Value>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkRowInput)]
pub struct RowInput {
    pub revision: i64,
    pub included: Option<bool>,
    pub operation: Option<String>,
    pub target: Option<Value>,
    pub values: Option<Value>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkReviewInput)]
pub struct ReviewInput {
    pub revision: i64,
    pub scope: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkApproveInput)]
pub struct ApproveInput {
    pub revision: i64,
    pub review_id: Uuid,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkSettingsInput)]
pub struct SettingsInput {
    pub per_run_cap_usd: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkSavedMappingInput)]
pub struct SavedMappingInput {
    pub name: String,
    pub mapping: Value,
}

#[derive(Default, Deserialize, utoipa::IntoParams)]
#[serde(default)]
#[into_params(parameter_in = Query)]
pub struct RowsQuery {
    pub after: Option<i64>,
    pub page_size: Option<i64>,
    pub status: Option<String>,
    pub q: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ProductBulkResumeInput)]
pub struct ResumeInput {
    pub revision: i64,
}

// Domain checks need the current run lock; the shared extractor still normalizes malformed JSON.
macro_rules! validated_body {
    ($($body:ty),+ $(,)?) => {$(
        impl crate::http::json::Validate for $body {
            type Valid = Self;
            fn validate(self, _: &mut crate::http::json::Rules) -> Option<Self> { Some(self) }
        }
    )+};
}
validated_body!(
    CreateInput,
    StageInput,
    MappingInput,
    RowInput,
    ReviewInput,
    ApproveInput,
    SettingsInput,
    SavedMappingInput,
    ResumeInput
);
