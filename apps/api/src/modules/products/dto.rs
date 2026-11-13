//! Product request bodies. The resource lives in `resource.rs`.
// Laravel Store/UpdateProductRequest and SearchProductRequest
// (vimdesk-web product.store.ts, ProductPrices.vue).

use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use serde_json::{Map, Value};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    http::json::{Rules, Validate, filled, human, is_url},
    money,
};

pub const TYPES: [&str; 7] = [
    "digital",
    "multi_product_group",
    "multi_product_set",
    "physical",
    "physical_second_chance",
    "service",
    "giftcard",
];
pub const STATUSES: [&str; 3] = ["active", "disabled", "nla"];
const UNITS: [&str; 10] = [
    "box", "cm", "gram", "kg", "liter", "m", "m2", "m3", "pc", "single",
];
const WARRANTY_MODES: [&str; 3] = ["receipt", "sale", "manual"];
const WARRANTY_UNITS: [&str; 6] = ["day", "days", "month", "months", "year", "years"];
/// Bundle types: need `multi_set_product_ids`.
pub const SET_TYPES: [&str; 2] = ["multi_product_set", "multi_product_group"];
/// Types that may carry dimensions, `has_stock` and `is_serialized` (Laravel exclude_unless).
pub const PHYSICAL: [&str; 2] = ["physical", "physical_second_chance"];

/// Every column a request may set. `None` = not sent (update keeps), `Some(None)` = clear.
#[derive(Default, Clone)]
pub struct Fields {
    /// UUID-keyed typed field patch; absent keeps existing values.
    pub custom_fields: Option<Map<String, Value>>,
    pub name: Option<String>,
    pub brand: Option<Option<String>>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub has_stock: Option<bool>,
    pub is_serialized: Option<bool>,
    pub warranty_default_mode: Option<Option<String>>,
    pub warranty_duration_value: Option<Option<i32>>,
    pub warranty_duration_unit: Option<Option<String>>,
    pub reference: Option<Option<String>>,
    pub reference_vendor: Option<Option<String>>,
    pub reference_vendor2: Option<Option<String>>,
    pub reference_vendor3: Option<Option<String>>,
    pub ean13: Option<Option<String>>,
    pub hs_code: Option<Option<String>>,
    pub barcode: Option<Option<String>>,
    pub barcode_vendor: Option<Option<String>>,
    pub supplier_discount_code: Option<Option<String>>,
    pub short_description: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub stock_level_supplier: Option<Option<i32>>,
    pub supplier_relation_id: Option<Option<Uuid>>,
    pub currency_id: Option<Uuid>,
    pub vat_group_id: Option<Option<Uuid>>,
    pub sales_group_id: Option<Option<Uuid>>,
    pub category_id: Option<Option<Uuid>>,
    pub default_warehouse_id: Option<Option<Uuid>>,
    pub price: Option<Decimal>,
    pub gross_price: Option<Decimal>,
    pub wholesale_price: Option<Option<Decimal>>,
    pub supplier_wholesale_price: Option<Option<Decimal>>,
    pub impact_on_price: Option<Option<Decimal>>,
    pub impact_on_wholesale_price: Option<Option<Decimal>>,
    pub order_unit: Option<Decimal>,
    pub sale_unit: Option<Decimal>,
    pub weight: Option<Option<Decimal>>,
    pub unit: Option<Option<String>>,
    pub dimensions: Option<Option<Value>>,
    pub country_of_origin: Option<Option<String>>,
    pub url_vendor: Option<Option<String>>,
    pub url_vendor_override: Option<Option<String>>,
    pub exclude_price_corrections: Option<bool>,
    pub import_id: Option<Option<String>>,
    pub import_source: Option<Option<String>>,
    pub export_enabled: Option<bool>,
    pub store_only: Option<bool>,
    /// Absent keeps them; `[]` or null clears them.
    pub labels: Option<Vec<String>>,
    pub export_price_sync: Option<bool>,
    pub export_product_data: Option<bool>,
    pub export_data: Option<Option<Value>>,
    pub meta: Option<Option<Value>>,
    /// Bundle children: (product id, quantity).
    pub set_children: Option<Vec<(Uuid, Decimal)>>,
}

/// POST /products and POST /products/{id}: the raw object, read key by key so absent and null
/// stay apart (Laravel `sometimes` vs `nullable`).
#[derive(Deserialize)]
pub struct ProductBody(Map<String, Value>);

/// Create: `name` + `price` required.
#[derive(Deserialize)]
pub struct CreateProductRequest(ProductBody);

impl CreateProductRequest {
    /// Farm supply raw material named `name`: physical, stock-tracked, kg, net price 0, status
    /// `disabled` (storefront and till catalogue sell only `active`; stock and lots ignore status).
    /// Validated like any create; VAT group and currency default.
    pub fn raw_material(name: Option<Value>) -> Self {
        let mut m = Map::new();
        if let Some(name) = name {
            m.insert("name".into(), name);
        }
        for (k, v) in [
            ("type", Value::from("physical")),
            ("status", Value::from("disabled")),
            ("has_stock", Value::from(true)),
            ("unit", Value::from("kg")),
            ("price", Value::from(0)),
        ] {
            m.insert(k.into(), v);
        }
        Self(ProductBody(m))
    }
}

/// Update: everything optional; `name` stays required when sent.
#[derive(Deserialize)]
pub struct UpdateProductRequest(ProductBody);

impl Validate for CreateProductRequest {
    type Valid = Fields;

    fn validate(self, r: &mut Rules) -> Option<Fields> {
        let m = &self.0.0;
        let mut f = fields(r, m, true);
        // "required" only for a missing or blank key; a bad value already has its own error.
        for k in ["name", "price"] {
            if filled(m.get(k).cloned()).is_none() {
                r.missing(k);
            }
        }
        if f.kind.as_deref().is_some_and(|t| SET_TYPES.contains(&t))
            && f.set_children.as_ref().is_none_or(Vec::is_empty)
        {
            r.missing("multi_set_product_ids");
        }
        // Laravel CreateProduct defaults.
        f.kind.get_or_insert_with(|| "physical".into());
        f.status.get_or_insert_with(|| "active".into());
        f.wholesale_price.get_or_insert(Some(Decimal::ZERO));
        f.supplier_wholesale_price
            .get_or_insert(Some(Decimal::ZERO));
        Some(f)
    }
}

impl Validate for UpdateProductRequest {
    type Valid = Fields;

    fn validate(self, r: &mut Rules) -> Option<Fields> {
        let m = &self.0.0;
        if m.contains_key("name") && filled(m.get("name").cloned()).is_none() {
            r.missing("name");
        }
        Some(fields(r, m, false))
    }
}

/// One key: absent = None, null/blank = Some(None), else Some(Some(v)).
fn key(m: &Map<String, Value>, k: &str) -> Option<Option<Value>> {
    m.get(k).map(|v| filled(Some(v.clone())))
}

fn text(r: &mut Rules, m: &Map<String, Value>, k: &str, max: usize) -> Option<Option<String>> {
    key(m, k).map(|v| {
        let v = v?;
        let s = match v {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            _ => {
                r.add(k, format!("The {} must be a string.", human(k)));
                return None;
            }
        };
        r.max_chars(k, &s, max);
        Some(s)
    })
}

fn decimal(
    r: &mut Rules,
    m: &Map<String, Value>,
    k: &str,
    min: Decimal,
    max: Decimal,
) -> Option<Option<Decimal>> {
    key(m, k).map(|v| v.and_then(|v| r.decimal(k, &v, min, max)))
}

/// Most labels per product, and their longest length.
pub const LABELS_MAX: usize = 8;
const LABEL_CHARS: usize = 32;

/// `labels`: a list of 1 to 32-character strings, at most 8, no repeats in any case; null =
/// none. Trimmed.
fn labels(r: &mut Rules, v: Option<Value>) -> Vec<String> {
    let Some(v) = v else {
        return Vec::new();
    };
    let Some(list) = v.as_array() else {
        r.add("labels", "The labels must be a list of texts.");
        return Vec::new();
    };
    if list.len() > LABELS_MAX {
        r.add(
            "labels",
            format!("The labels must not have more than {LABELS_MAX} items."),
        );
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for (n, item) in list.iter().enumerate() {
        let field = format!("labels.{n}");
        let Some(s) = item.as_str().map(str::trim) else {
            r.add(&field, "Each label must be a text.");
            continue;
        };
        let len = s.chars().count();
        if len == 0 || len > LABEL_CHARS {
            r.add(
                &field,
                format!("Each label must be 1 to {LABEL_CHARS} characters."),
            );
        } else if out.iter().any(|o| o.to_lowercase() == s.to_lowercase()) {
            r.add(&field, "The label is listed twice.");
        } else {
            out.push(s.to_owned());
        }
    }
    out
}

fn flag(r: &mut Rules, m: &Map<String, Value>, k: &str) -> Option<bool> {
    key(m, k).map(|v| v.and_then(|v| r.boolean(k, &v)).unwrap_or_default())
}

fn int(r: &mut Rules, m: &Map<String, Value>, k: &str, min: i64) -> Option<Option<i32>> {
    key(m, k).map(|v| {
        v.and_then(|v| r.integer(k, &v, min, i32::MAX.into()))
            .and_then(|n| i32::try_from(n).ok())
    })
}

/// Laravel EnumExist: lowercased input, "The selected {attr}: {value} is invalid."
fn enum_of(r: &mut Rules, m: &Map<String, Value>, k: &str, allowed: &[&str]) -> Option<String> {
    let v = key(m, k)??;
    let s = v
        .as_str()
        .map(str::to_lowercase)
        .unwrap_or_else(|| v.to_string());
    if allowed.contains(&s.as_str()) {
        Some(s)
    } else {
        r.add(k, format!("The selected {}: {s} is invalid.", human(k)));
        None
    }
}

/// `nullable|in:` on a plain list.
fn choice(
    r: &mut Rules,
    m: &Map<String, Value>,
    k: &str,
    allowed: &[&str],
) -> Option<Option<String>> {
    text(r, m, k, usize::MAX).map(|v| v.and_then(|s| r.one_of(k, Some(s), allowed, None)))
}

/// A uuid the service checks later; malformed = the ValidateId text right away.
fn id(r: &mut Rules, m: &Map<String, Value>, k: &str, bad: &str) -> Option<Option<Uuid>> {
    key(m, k).map(|v| {
        let v = v?;
        let id = v.as_str().and_then(|s| Uuid::parse_str(s.trim()).ok());
        if id.is_none() {
            r.add(k, bad);
        }
        id
    })
}

fn url(r: &mut Rules, m: &Map<String, Value>, k: &str, check: bool) -> Option<Option<String>> {
    text(r, m, k, 2048).map(|v| {
        let v = v?;
        if check && !is_url(&v) {
            r.add(k, format!("The {} must be a valid URL.", human(k)));
        }
        Some(v)
    })
}

/// Laravel ValidateId message for `field` in `domain`.
pub fn bad_id(field: &str, domain: &str) -> String {
    format!(
        "The {} is an invalid id for the {domain} domain.",
        human(field)
    )
}

/// The CurrencyExist text.
pub const BAD_CURRENCY: &str = "This currency id does not exist";

fn fields(r: &mut Rules, m: &Map<String, Value>, create: bool) -> Fields {
    let zero = Decimal::ZERO;
    let max = money::MAX;
    let any = -money::MAX;
    r.custom_sections("custom_sections", m.get("custom_sections"));
    r.custom_sections("custom_section_forms", m.get("custom_section_forms"));
    let kind = enum_of(r, m, "type", &TYPES);
    let status = enum_of(r, m, "status", &STATUSES);
    // NOT NULL column: on update a null price keeps the stored one (Laravel then failed).
    let price = decimal(r, m, "price", zero, max).flatten();
    let set_children = m
        .get("multi_set_product_ids")
        .and_then(|v| set_children(r, v));
    let currency_id = id(r, m, "currency_id", BAD_CURRENCY).flatten();
    let custom_fields = m.get("custom_fields").and_then(|v| match v.as_object() {
        Some(object) => Some(object.clone()),
        _ => {
            r.add("custom_fields", "Use an object keyed by field UUIDs.");
            None
        }
    });
    Fields {
        custom_fields,
        name: text(r, m, "name", 255).flatten(),
        brand: text(r, m, "brand", 255),
        kind,
        status,
        has_stock: flag(r, m, "has_stock"),
        is_serialized: flag(r, m, "is_serialized"),
        warranty_default_mode: choice(r, m, "warranty_default_mode", &WARRANTY_MODES),
        warranty_duration_value: int(r, m, "warranty_duration_value", 0),
        warranty_duration_unit: choice(r, m, "warranty_duration_unit", &WARRANTY_UNITS),
        reference: text(r, m, "reference", 100),
        reference_vendor: text(r, m, "reference_vendor", 100),
        reference_vendor2: text(r, m, "reference_vendor2", 100),
        reference_vendor3: text(r, m, "reference_vendor3", 100),
        ean13: text(r, m, "ean13", 100),
        hs_code: text(r, m, "hs_code", 55),
        barcode: text(r, m, "barcode", 55),
        barcode_vendor: text(r, m, "barcode_vendor", 255),
        supplier_discount_code: text(r, m, "supplier_discount_code", 55),
        short_description: text(r, m, "short_description", 255),
        description: text(r, m, "description", 1500),
        // Laravel `numeric` into an int column: MySQL rounded.
        stock_level_supplier: decimal(r, m, "stock_level_supplier", any, max).map(|v| {
            v.and_then(|d| {
                let n = money::round(d, 0).to_i32();
                if n.is_none() {
                    let msg = if d.is_sign_negative() {
                        "The stock level supplier must be at least -2147483648."
                    } else {
                        "The stock level supplier must not be greater than 2147483647."
                    };
                    r.add("stock_level_supplier", msg);
                }
                n
            })
        }),
        supplier_relation_id: id(
            r,
            m,
            "supplier_relation_id",
            &bad_id("supplier_relation_id", "relation"),
        ),
        currency_id,
        vat_group_id: id(r, m, "vat_group_id", &bad_id("vat_group_id", "vat_group")),
        sales_group_id: id(
            r,
            m,
            "sales_group_id",
            &bad_id("sales_group_id", "sales_group"),
        ),
        category_id: id(
            r,
            m,
            "category_id",
            &bad_id("category_id", "product_category"),
        ),
        default_warehouse_id: id(
            r,
            m,
            "default_warehouse_id",
            &bad_id("default_warehouse_id", "warehouse"),
        ),
        price,
        gross_price: decimal(r, m, "gross_price", zero, max).map(Option::unwrap_or_default),
        wholesale_price: decimal(r, m, "wholesale_price", zero, max),
        supplier_wholesale_price: decimal(r, m, "supplier_wholesale_price", zero, max),
        impact_on_price: decimal(r, m, "impact_on_price", any, max),
        impact_on_wholesale_price: decimal(
            r,
            m,
            "impact_on_wholesale_price",
            any,
            Decimal::ONE_HUNDRED,
        ),
        order_unit: decimal(r, m, "order_unit", zero, max).flatten(),
        sale_unit: decimal(r, m, "sale_unit", zero, max).flatten(),
        weight: decimal(r, m, "weight", zero, max),
        unit: choice(r, m, "unit", &UNITS),
        dimensions: key(m, "dimensions").map(|v| v.and_then(|v| dimensions(r, &v))),
        country_of_origin: text(r, m, "country_of_origin", 100),
        // Laravel's update rules took any string here; create checked URLs.
        url_vendor: url(r, m, "url_vendor", create),
        url_vendor_override: url(r, m, "url_vendor_override", create),
        exclude_price_corrections: flag(r, m, "exclude_price_corrections"),
        import_id: text(r, m, "import_id", 255),
        import_source: text(r, m, "import_source", 255),
        export_enabled: flag(r, m, "export_enabled"),
        store_only: flag(r, m, "store_only"),
        labels: key(m, "labels").map(|v| labels(r, v)),
        export_price_sync: flag(r, m, "export_price_sync"),
        export_product_data: flag(r, m, "export_product_data"),
        export_data: key(m, "export_data").map(|v| v.and_then(|v| object(r, "export_data", v))),
        meta: m.get("meta").and_then(|v| r.meta(Some(v.clone()))),
        set_children,
    }
}

/// `dimensions` {length, width, height}: numbers >= 0, kept as JSON numbers.
fn dimensions(r: &mut Rules, v: &Value) -> Option<Value> {
    let Value::Object(m) = v else {
        r.add("dimensions", "The dimensions must be an array.");
        return None;
    };
    let mut out = Map::new();
    for k in ["length", "width", "height"] {
        let n = filled(m.get(k).cloned())
            .and_then(|n| r.number(&format!("dimensions.{k}"), &n, Decimal::ZERO));
        out.insert(k.into(), n.map_or(Value::Null, Value::from));
    }
    Some(Value::Object(out))
}

fn object(r: &mut Rules, k: &str, v: Value) -> Option<Value> {
    if v.is_object() {
        Some(v)
    } else {
        r.add(k, format!("The {} must be an array.", human(k)));
        None
    }
}

/// `multi_set_product_ids`: [{id: uuid, quantity: numeric >= 1}], at most 100.
fn set_children(r: &mut Rules, v: &Value) -> Option<Vec<(Uuid, Decimal)>> {
    let Value::Array(items) = v else {
        r.add(
            "multi_set_product_ids",
            "The multi set product ids must be an array.",
        );
        return None;
    };
    if items.len() > 100 {
        r.add(
            "multi_set_product_ids",
            "The multi set product ids must not have more than 100 items.",
        );
        return None;
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let k = format!("multi_set_product_ids.{i}");
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());
        if id.is_none() {
            r.add(
                &format!("{k}.id"),
                format!("The {k}.id must be a valid UUID."),
            );
        }
        let qty = filled(item.get("quantity").cloned())
            .and_then(|q| r.decimal(&format!("{k}.quantity"), &q, Decimal::ONE, money::MAX));
        if let Some(id) = id
            && out.iter().any(|(seen, _)| *seen == id)
        {
            r.add(
                &format!("{k}.id"),
                format!("The {k}.id field has a duplicate value."),
            );
        } else if let (Some(id), Some(q)) = (id, qty) {
            out.push((id, q));
        }
    }
    Some(out)
}

/// POST /products/search.
#[derive(Deserialize, ToSchema)]
#[schema(as = ProductSearchRequest)]
pub struct SearchBody {
    /// Barcode, EAN or product id; a literal `Shift` (some scanners type it) is dropped.
    #[schema(value_type = String, max_length = 155)]
    query: Option<String>,
}

impl Validate for SearchBody {
    type Valid = String;

    fn validate(self, r: &mut Rules) -> Option<String> {
        // Laravel strips the literal "Shift" some barcode scanners type.
        let q = self.query.map(|q| q.replace("Shift", ""));
        Some(r.text("query", q, 155))
    }
}

/// GET list query. Unknown params ignored.
// Laravel ListProductRequest plus the RN/web params Laravel ignored.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// JSON array of typed custom-field filters (AND), at most 20.
    pub custom_filters: Option<String>,
    /// Search on name, number, references and codes.
    pub q: Option<String>,
    /// Same as `q` (used when `q` is absent).
    pub search: Option<String>,
    /// `active`, `disabled` or `nla`. Default `active`.
    pub status: Option<String>,
    /// 1-based; junk falls back to 1.
    #[param(value_type = Option<i64>)]
    pub page: Option<String>,
    /// Page size (default 50, max 100); `limit` and `per_page` win over it.
    #[param(value_type = Option<i64>)]
    pub page_size: Option<String>,
    /// Page size; `limit` wins over it.
    #[param(value_type = Option<i64>)]
    pub per_page: Option<String>,
    /// Page size.
    #[param(value_type = Option<i64>)]
    pub limit: Option<String>,
    /// Only products of this supplier relation.
    #[param(value_type = Option<Uuid>)]
    pub supplier_id: Option<String>,
    /// Same as `supplier_id`.
    #[param(value_type = Option<Uuid>)]
    pub supplier_relation_id: Option<String>,
    /// Only products of this category (`/erp/product-categories`).
    #[param(value_type = Option<Uuid>)]
    pub category_id: Option<String>,
    /// Exact EAN-13.
    pub ean13: Option<String>,
    /// Comma-separated product ids to leave out, at most 100.
    pub exclude_ids: Option<String>,
    /// Warehouse for the `pos_*` fields; does not filter.
    #[param(value_type = Option<Uuid>)]
    pub warehouse_id: Option<String>,
    /// Only stock-tracked products whose sellable stock is at most this. Sellable = Σ (level −
    /// reserved) over all warehouses and lots, held, recalled or expired lots left out (the
    /// shop's "sold out"). Out of stock: `available_lte=0`. Untracked products never match.
    #[param(value_type = Option<String>, example = "0")]
    pub available_lte: Option<String>,
}

/// GET one query: `warehouse_id` shapes the `pos_*` fields.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GetQuery {
    /// Warehouse for the `pos_*` fields.
    #[param(value_type = Option<Uuid>)]
    pub warehouse_id: Option<String>,
}

/// A sent `warehouse_id`: a malformed one matches no warehouse (nil), blank is none.
pub fn pos_warehouse(v: Option<&str>) -> Option<Uuid> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Uuid::parse_str(s).unwrap_or(Uuid::nil()))
}

/// GET price-history query.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryQuery {
    /// 1-based; junk falls back to 1.
    #[param(value_type = Option<i64>)]
    pub page: Option<String>,
    /// Default 25, max 100.
    #[param(value_type = Option<i64>)]
    pub per_page: Option<String>,
}

/// OpenAPI shape of [`ProductBody`]. Docs only: the body is read key by key.
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
#[schema(
    as = ProductRequest,
    description = "Create needs `name` and `price` (and `multi_set_product_ids` for bundles); \
        update keeps keys left out, null clears them. Decimals as strings (`\"12.50\"`); \
        JSON numbers also work."
)]
pub struct ProductRequestDoc {
    /// Field UUID to typed value. Absent keeps; null clears only nullable fields.
    #[schema(value_type=Option<Object>)]
    custom_fields: Option<Value>,
    /// max 255; required on create
    name: Option<String>,
    /// max 255
    brand: Option<String>,
    /// Default `physical`.
    #[schema(rename = "type")]
    kind: Option<String>,
    /// `active` (on sale), `disabled` (hidden from shop and till, kept, e.g. until next season)
    /// or `nla` (no longer available). Default `active`.
    status: Option<String>,
    has_stock: Option<bool>,
    is_serialized: Option<bool>,
    /// `receipt`, `sale` or `manual`
    warranty_default_mode: Option<String>,
    warranty_duration_value: Option<i32>,
    /// `day(s)`, `month(s)` or `year(s)`
    warranty_duration_unit: Option<String>,
    reference: Option<String>,
    reference_vendor: Option<String>,
    reference_vendor2: Option<String>,
    reference_vendor3: Option<String>,
    ean13: Option<String>,
    hs_code: Option<String>,
    barcode: Option<String>,
    barcode_vendor: Option<String>,
    supplier_discount_code: Option<String>,
    short_description: Option<String>,
    description: Option<String>,
    stock_level_supplier: Option<i32>,
    supplier_relation_id: Option<Uuid>,
    currency_id: Option<Uuid>,
    vat_group_id: Option<Uuid>,
    sales_group_id: Option<Uuid>,
    /// Main category (`/erp/product-categories`), same workspace; null = none.
    category_id: Option<Uuid>,
    default_warehouse_id: Option<Uuid>,
    /// Net consumer price; required on create.
    price: Option<String>,
    gross_price: Option<String>,
    wholesale_price: Option<String>,
    supplier_wholesale_price: Option<String>,
    impact_on_price: Option<String>,
    impact_on_wholesale_price: Option<String>,
    order_unit: Option<String>,
    sale_unit: Option<String>,
    weight: Option<String>,
    unit: Option<String>,
    /// `length`, `width`, `height` (numbers >= 0).
    #[schema(value_type = Option<Object>)]
    dimensions: Option<Value>,
    country_of_origin: Option<String>,
    url_vendor: Option<String>,
    url_vendor_override: Option<String>,
    exclude_price_corrections: Option<bool>,
    import_id: Option<String>,
    import_source: Option<String>,
    export_enabled: Option<bool>,
    /// Sold at the till only; the web shop never lists it.
    store_only: Option<bool>,
    /// Short public labels the shop shows as chips, e.g. `["Raw", "Unheated"]`: at most 8,
    /// each 1 to 32 characters, no repeats (any case). `[]` or null clears them.
    #[schema(value_type = Option<Vec<String>>, example = json!(["Raw", "Unheated"]))]
    labels: Option<Value>,
    export_price_sync: Option<bool>,
    export_product_data: Option<bool>,
    #[schema(value_type = Option<Object>)]
    export_data: Option<Value>,
    #[schema(value_type = Option<Object>)]
    meta: Option<Value>,
    /// Bundle parts `[{id, quantity}]`, at most 100; required for bundle types.
    #[schema(value_type = Option<Vec<Object>>)]
    multi_set_product_ids: Option<Value>,
    /// Custom fields are not supported: an item naming a `section_id` is rejected.
    #[schema(value_type = Option<Vec<Object>>)]
    custom_sections: Option<Value>,
    /// Same rule as `custom_sections`.
    #[schema(value_type = Option<Vec<Object>>)]
    custom_section_forms: Option<Value>,
}

/// Create and duplicate response.
#[allow(dead_code)]
#[derive(ToSchema)]
pub struct ProductCreated {
    product_id: Uuid,
}

/// Sales figures; all zero or null until invoices exist.
#[allow(dead_code)]
#[derive(ToSchema)]
pub struct SaleStatics {
    units_sold: i64,
    revenue: f64,
    #[schema(required = true)]
    avg_monthly_sales: Option<f64>,
    #[schema(required = true)]
    last_sale: Option<String>,
    customer_orders: i64,
    #[schema(required = true)]
    last_order: Option<String>,
    #[schema(required = true)]
    months_active: Option<i64>,
}

/// Linked products; always empty for now.
#[allow(dead_code)]
#[derive(ToSchema)]
pub struct LinkedProducts {
    products: Vec<Uuid>,
}

/// Decode native product values for durable imports using exactly the HTTP validator.
pub(crate) fn decode_values(value: Value, create: bool) -> Result<Fields, crate::AppError> {
    let mut rules = Rules::default();
    let fields = if create {
        serde_json::from_value::<CreateProductRequest>(value)
            .map_err(|_| crate::AppError::field("values", "Use a product object."))?
            .validate(&mut rules)
    } else {
        serde_json::from_value::<UpdateProductRequest>(value)
            .map_err(|_| crate::AppError::field("values", "Use a product object."))?
            .validate(&mut rules)
    };
    rules.into_result()?;
    fields.ok_or_else(|| crate::AppError::field("values", "Use valid product values."))
}
