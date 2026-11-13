//! Product JSON for detail and list. One builder for both: list rows carry the detail-only
//! keys too.
// Laravel ProductResource shape (detail) and ProductIndexResource shape (list).
// shortcut: list rows carry detail-only keys; split once offers/images add per-row queries.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    modules::{
        currencies::dto::CurrencyResource,
        lots::public::Link,
        products::{
            dto::PHYSICAL,
            pricing::{
                Band, PriceInputs, column, computed, discounted, incl, prices, purchase_price,
            },
            repo::ProductRow,
        },
        special_offers::{self, dto::OfferResource, repo::OfferRow},
        stock::{self, repo::StockLine},
        vat_groups::{dto::VatGroupResource, repo::VatGroupRow},
        warehouses::repo::StockRef,
    },
    money::php_number,
};

/// Everything a page of products embeds, loaded once per page.
#[derive(Default)]
pub struct Embeds {
    pub custom_fields: HashMap<Uuid, Value>,
    pub vat_groups: HashMap<Uuid, VatGroupRow>,
    pub currencies: HashMap<Uuid, Value>,
    pub sales_groups: HashMap<Uuid, Value>,
    pub suppliers: HashMap<Uuid, Value>,
    /// `default_warehouse`: full WarehouseResource on detail, compact on the list.
    pub warehouses: HashMap<Uuid, Value>,
    /// Fallback stock warehouses and the first store.
    pub stock_refs: HashMap<Uuid, StockRef>,
    pub store: Option<Uuid>,
    /// Bundle children per parent, already rendered, with their quantity.
    pub set_children: HashMap<Uuid, Vec<(Value, Decimal)>>,
    /// Live stock rows per product.
    pub stocks: HashMap<Uuid, Vec<StockLine>>,
    /// `?warehouse_id=` for the `pos_*` pick (Laravel read it from the request).
    pub pos_warehouse: Option<Uuid>,
    /// Best live internal supplier discount per product (purchase price).
    pub purchase_discounts: HashMap<Uuid, Decimal>,
    /// Live price correction bands of the workspace, in pick order.
    pub bands: Vec<Band>,
    /// Setting `product.disable_double_discount_for_business`.
    pub business_from_gross: bool,
    /// Live special offers per product, best first.
    pub offers: HashMap<Uuid, Vec<OfferRow>>,
    /// The UTC date the offers were loaded for (one per request).
    pub today: NaiveDate,
    /// Each product's first own image (variant images skipped), signed.
    pub thumbnails: HashMap<Uuid, Link>,
}

/// One product. `detail`: GET one (Laravel loaded more there).
pub fn render(p: &ProductRow, e: &Embeds, detail: bool) -> Value {
    let vat = p.vat_group_id.and_then(|id| e.vat_groups.get(&id));
    let currency = e.currencies.get(&p.currency_id);
    let vat_pct = vat.map_or(Decimal::ZERO, |v| v.vat_amount);
    let mutates = PHYSICAL.contains(&p.kind.as_str()) && p.has_stock;
    let lines = e.stocks.get(&p.id).map_or(&[][..], Vec::as_slice);
    let pos = if mutates { pos_pick(p, lines, e) } else { None };
    let offers = e.offers.get(&p.id).map_or(&[][..], Vec::as_slice);
    let active = special_offers::service::active(offers, e.today);
    let net = prices(&PriceInputs {
        net: p.price,
        gross: p.gross_price,
        impact_on_price: p.impact_on_price,
        excluded: p.exclude_price_corrections,
        business_from_gross: e.business_from_gross,
        vat_pct,
        bands: &e.bands,
        offer: active.map(special_offers::service::pricing),
    });
    let fmt = |v: Decimal| (computed(v, 4), computed(incl(v, vat_pct), 2));
    let (price, price_incl) = fmt(net.consumer);
    let (original, original_incl) = fmt(net.original);
    let (business, business_incl) = fmt(net.business);
    let (uncorrected, uncorrected_incl) = fmt(net.uncorrected);
    let discount = e.purchase_discounts.get(&p.id).copied();
    let purchase = purchase_price(
        p.impact_on_wholesale_price,
        p.wholesale_price,
        p.supplier_wholesale_price,
        discount,
        p.gross_price,
    );
    let children = e.set_children.get(&p.id).map_or_else(Vec::new, |c| {
        c.iter()
            .map(|(child, qty)| {
                let mut child = child.clone();
                // Laravel printed the raw pivot column: scale 8.
                child["quantity"] = Value::String(raw(*qty, 8));
                child
            })
            .collect()
    });
    json!({
        "id": p.id,
        "purchase_price": column(purchase, 4),
        "name": p.name,
        "brand": p.brand,
        "product_nr": p.product_nr_formatted,
        "reference": p.reference,
        "hs_group": null,
        "reference_vendor": p.reference_vendor,
        "reference_vendor2": p.reference_vendor2,
        "reference_vendor3": p.reference_vendor3,
        "url_vendor": p.url_vendor,
        "url_vendor_override": p.url_vendor_override,
        "ean13": p.ean13,
        "hs_code": p.hs_code,
        "barcode": p.barcode,
        "barcode_vendor": p.barcode_vendor,
        "type": p.kind,
        // Laravel left tinyints uncast here.
        "has_stock": i32::from(p.has_stock),
        "is_serialized": p.is_serialized,
        "warranty_default_mode": p.warranty_default_mode,
        "warranty_duration_value": p.warranty_duration_value,
        "warranty_duration_unit": p.warranty_duration_unit,
        "should_mutate_stock": mutates,
        "vat_group": null,
        "status": p.status,
        "business_price": business,
        "business_price_incl": business_incl,
        "impact_on_price": column(p.impact_on_price, 4),
        "impact_on_wholesale_price": column(p.impact_on_wholesale_price, 4),
        "price": price,
        "price_incl": price_incl,
        // Top-level for money cards; same currency as `relationships.currency`.
        "currency": currency.and_then(|c| c["asset_type"].as_str()).map(str::to_uppercase),
        "decimals": currency.map(|c| c["decimals"].clone()),
        "price_without_correction": uncorrected,
        "price_without_correction_incl": uncorrected_incl,
        "import_id": p.import_id,
        "import_source": p.import_source,
        "import_links": [],
        "original_price": original,
        "original_price_incl": original_incl,
        "wholesale_price": column(p.wholesale_price, 4),
        "gross_price": column(Some(p.gross_price), 4),
        "gross_price_incl": computed(incl(p.gross_price, vat_pct), 2),
        "supplier_wholesale_price": column(p.supplier_wholesale_price, 4),
        "purchase_discount_percentage": discount.map(|d| raw(d, 8)),
        "supplier_purchase_price_from_discount": match discount.filter(|d| !d.is_zero()) {
            Some(d) => computed(discounted(p.gross_price, d), 4),
            None => column(p.supplier_wholesale_price, 4),
        },
        "archived": i32::from(p.archived),
        "country_of_origin": p.country_of_origin,
        "weight": p.weight.map_or_else(|| "0.1".to_owned(), |w| column(Some(w), 2)),
        "images": [],
        "thumbnail": e.thumbnails.get(&p.id),
        "stock_level_supplier": p.stock_level_supplier,
        "supplier_discount_code": p.supplier_discount_code,
        "sales_group": null,
        "supplier_discount_group": null,
        "unit": p.unit,
        "order_unit": raw(p.order_unit, 4),
        "sale_unit": raw(p.sale_unit, 4),
        "dimensions": p.dimensions,
        "short_description": p.short_description,
        "description": p.description,
        "second_chance_reason": null,
        "exclude_price_corrections": p.exclude_price_corrections,
        "total_available_stock": php_number(lines.iter().map(|l| l.stock_lvl).sum()),
        "total_reserved_stock": php_number(lines.iter().map(|l| l.reserved).sum()),
        "pos_available_stock": php_number(pos.map_or(Decimal::ZERO, |l| l.stock_lvl)),
        "pos_stock_status": match pos {
            _ if !mutates => "not_applicable",
            Some(l) if l.stock_lvl > Decimal::ZERO => "available",
            _ => "unavailable",
        },
        "pos_stock_warehouse_id": pos.map(|l| l.warehouse_id),
        "pos_stock_id": pos.map(|l| l.id),
        "default_warehouse": p.default_warehouse_id.and_then(|id| e.warehouses.get(&id)),
        "stocks": stocks(p, lines, e),
        "supplier_relation": p.supplier_relation_id.and_then(|id| e.suppliers.get(&id)),
        "attributes": null,
        "meta": p.meta,
        "custom_fields": e.custom_fields.get(&p.id).cloned().unwrap_or_else(||json!({})),
        "association_data": [],
        "created_at": p.created_at,
        "last_updated_at": p.updated_at,
        "relationships": {
            "currency": currency,
            "active_special_offer": active.map(OfferResource::from),
            // Laravel loaded them on the detail only; list rows load today's offers.
            "special_offers": if detail {
                offers.iter().map(OfferResource::from).collect::<Vec<_>>()
            } else {
                Vec::new()
            },
            "sales_group": p.sales_group_id.and_then(|id| e.sales_groups.get(&id)),
            "vat_group": vat.map(|v| VatGroupResource::from(v.clone())),
            "custom_sections": [],
            "child_products": [],
            "parent_product": null,
            "return_item": null,
            "child_products_from_set": if detail { Value::from(children) } else { json!([]) },
            "storage_locations": [],
        },
        "vat_group_id": p.vat_group_id,
        "export_enabled": p.export_enabled,
        "store_only": p.store_only,
        "labels": p.labels,
        "category_id": p.category_id,
        "export_price_sync": p.export_price_sync,
        "export_product_data": p.export_product_data,
        "export_data": p.export_data.clone().unwrap_or_else(|| json!({
            "title": null, "description_short": null, "description": null,
            "meta_title": null, "meta_description": null, "keywords": null,
        })),
    })
}

/// An uncast Laravel decimal column as MySQL printed it: always `dp` places (`"1.0000"`).
fn raw(v: Decimal, dp: u32) -> String {
    let mut v = crate::money::round(v, dp);
    v.rescale(dp);
    v.to_string()
}

/// Laravel `stocksWithFallbackWarehouses`: the real rows (StockWithoutRelationsResource), then
/// one zero row for the default warehouse and the first store when they have none.
fn stocks(p: &ProductRow, lines: &[StockLine], e: &Embeds) -> Vec<Value> {
    let mut rows: Vec<Value> = lines.iter().map(stock_row).collect();
    let mut have: Vec<Uuid> = lines.iter().filter_map(|l| l.w_id).collect();
    for id in p.default_warehouse_id.into_iter().chain(e.store) {
        if have.contains(&id) {
            continue;
        }
        have.push(id);
        if let Some(w) = e.stock_refs.get(&id) {
            rows.push(fallback_row(w));
        }
    }
    rows
}

/// One real stock row. No `product` key: Laravel didn't load it in product resources.
fn stock_row(l: &StockLine) -> Value {
    json!({
        "id": l.id,
        "stock_lvl": raw(l.stock_lvl, 2),
        "reserved": raw(l.reserved, 2),
        "minimal_stock_level": l.minimal_stock_level,
        "maximum_stock_level": l.maximum_stock_level,
        "warehouse": l.w_id.map(|id| json!({
            "id": id, "name": l.w_name, "alias": l.w_alias, "color": null, "icon": null,
            "is_store": l.w_is_store, "pickup_enabled": l.w_pickup, "location": l.w_location,
        })),
        "shelf": l.shelf_id.map(|id| json!({
            "id": id, "shelf_code": l.shelf_code, "shelf_name": l.shelf_name,
        })),
        "bin": l.bin_id.map(|id| json!({ "id": id, "bin_code": l.bin_code, "bin_name": l.bin_name })),
        "lot": stock::resource::lot(l.lot_id, l.lot_code.as_deref(), l.lot_expires_on),
        "updated_at": l.updated_at,
        "meta": l.meta,
    })
}

/// Laravel `makeFallbackStockRow`.
fn fallback_row(w: &StockRef) -> Value {
    json!({
        "id": format!("fallback-{}", w.id),
        "stock_lvl": "0.00",
        "reserved": "0.00",
        "minimal_stock_level": 0,
        "maximum_stock_level": 0,
        "warehouse": {
            "id": w.id, "name": w.name, "alias": w.alias, "color": null, "icon": null,
            "is_store": w.is_store, "pickup_enabled": w.pickup_enabled, "location": w.location,
        },
        "shelf": null,
        "bin": null,
        "lot": null,
        "product": null,
        "updated_at": null,
        "meta": null,
    })
}

/// Laravel `posStockPayload`: the highest pickup-enabled row in the asked warehouse; without
/// one asked, in the default warehouse, else anywhere. Ties keep the oldest row.
fn pos_pick<'a>(p: &ProductRow, lines: &'a [StockLine], e: &Embeds) -> Option<&'a StockLine> {
    let best = |wh: Option<Uuid>| {
        lines
            .iter()
            .filter(|l| l.w_pickup == Some(true) && wh.is_none_or(|w| l.warehouse_id == w))
            .fold(None, |best: Option<&StockLine>, l| match best {
                Some(b) if b.stock_lvl >= l.stock_lvl => Some(b),
                _ => Some(l),
            })
    };
    match e.pos_warehouse {
        Some(w) => best(Some(w)),
        None => p
            .default_warehouse_id
            .and_then(|w| best(Some(w)))
            .or_else(|| best(None)),
    }
}

/// Price-history row (Laravel ProductPriceHistoryResource).
pub fn history_row(h: &crate::modules::products::repo::HistoryRow) -> Value {
    let mut row = json!({
        "id": h.id,
        "field": h.field,
        "old_value": h.old_value.map(|v| column(Some(v), 4)),
        "new_value": h.new_value.map(|v| column(Some(v), 4)),
        "changed_by_flow": h.changed_by_flow,
        "created_at": h.created_at,
    });
    if let (Some(first), Some(last)) = (&h.first_name, &h.last_name) {
        row["changed_by"] = Value::String(crate::modules::users::dto::full_name(
            first,
            h.middle_name.as_deref(),
            last,
        ));
    }
    if let Some(id) = h.variant_id {
        row["variant"] = json!({ "id": id, "sku": h.variant_sku });
    }
    row
}

/// OpenAPI shape of [`render`]'s product JSON. Docs only: `render` builds a `json!` literal,
/// and the contract check validates real replies against this.
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
#[schema(
    as = ProductResource,
    description = "A product with prices, stock and embeds. Decimal columns are strings \
        (`\"12.5000\"`); list rows leave `special_offers` and `child_products_from_set` empty."
)]
pub struct ProductDoc {
    #[schema(value_type=Object)]
    custom_fields: Value,
    id: Uuid,
    /// 4 decimals.
    purchase_price: String,
    name: String,
    #[schema(required = true)]
    brand: Option<String>,
    product_nr: String,
    #[schema(required = true)]
    reference: Option<String>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    hs_group: Option<Value>,
    #[schema(required = true)]
    reference_vendor: Option<String>,
    #[schema(required = true)]
    reference_vendor2: Option<String>,
    #[schema(required = true)]
    reference_vendor3: Option<String>,
    #[schema(required = true)]
    url_vendor: Option<String>,
    #[schema(required = true)]
    url_vendor_override: Option<String>,
    #[schema(required = true)]
    ean13: Option<String>,
    #[schema(required = true)]
    hs_code: Option<String>,
    #[schema(required = true)]
    barcode: Option<String>,
    #[schema(required = true)]
    barcode_vendor: Option<String>,
    #[schema(rename = "type")]
    kind: String,
    /// `0` or `1`.
    has_stock: i32,
    is_serialized: bool,
    #[schema(required = true)]
    warranty_default_mode: Option<String>,
    #[schema(required = true)]
    warranty_duration_value: Option<i32>,
    #[schema(required = true)]
    warranty_duration_unit: Option<String>,
    should_mutate_stock: bool,
    /// Always null; see `relationships.vat_group`.
    #[schema(value_type = Option<Object>, required = true)]
    vat_group: Option<Value>,
    status: String,
    business_price: String,
    business_price_incl: String,
    impact_on_price: String,
    impact_on_wholesale_price: String,
    price: String,
    price_incl: String,
    /// Currency of the prices: upper-case ISO 4217 code; null if that currency was deleted.
    #[schema(required = true, example = "EUR")]
    currency: Option<String>,
    /// The currency's minor-unit exponent, for display. Price strings stay major units.
    #[schema(required = true)]
    decimals: Option<i16>,
    price_without_correction: String,
    price_without_correction_incl: String,
    #[schema(required = true)]
    import_id: Option<String>,
    #[schema(required = true)]
    import_source: Option<String>,
    /// Always empty.
    import_links: Vec<Value>,
    original_price: String,
    original_price_incl: String,
    wholesale_price: String,
    gross_price: String,
    gross_price_incl: String,
    supplier_wholesale_price: String,
    /// 8 decimals; null without a supplier discount.
    #[schema(required = true)]
    purchase_discount_percentage: Option<String>,
    supplier_purchase_price_from_discount: String,
    /// `0` or `1`.
    archived: i32,
    #[schema(required = true)]
    country_of_origin: Option<String>,
    /// 2 decimals; `"0.1"` when unset.
    weight: String,
    /// Always empty; see `thumbnail`.
    images: Vec<Value>,
    /// The product's first own image (variant images skipped) as a signed link; null without
    /// one, or on a server without file storage.
    #[schema(required = true)]
    thumbnail: Option<Link>,
    #[schema(required = true)]
    stock_level_supplier: Option<i32>,
    #[schema(required = true)]
    supplier_discount_code: Option<String>,
    /// Always null; see `relationships.sales_group`.
    #[schema(value_type = Option<Object>, required = true)]
    sales_group: Option<Value>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    supplier_discount_group: Option<Value>,
    #[schema(required = true)]
    unit: Option<String>,
    /// 4 decimals.
    order_unit: String,
    /// 4 decimals.
    sale_unit: String,
    #[schema(value_type = Option<Object>, required = true)]
    dimensions: Option<Value>,
    #[schema(required = true)]
    short_description: Option<String>,
    #[schema(required = true)]
    description: Option<String>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    second_chance_reason: Option<Value>,
    exclude_price_corrections: bool,
    total_available_stock: f64,
    /// Units open orders hold, over every stock row of the product.
    total_reserved_stock: f64,
    pos_available_stock: f64,
    /// `available`, `unavailable` or `not_applicable`.
    pos_stock_status: String,
    #[schema(required = true)]
    pos_stock_warehouse_id: Option<Uuid>,
    #[schema(required = true)]
    pos_stock_id: Option<Uuid>,
    /// Short warehouse on lists, full warehouse on the single GET.
    #[schema(value_type = Option<Object>, required = true)]
    default_warehouse: Option<Value>,
    /// Real stock rows, then a zero row (id `fallback-<warehouse>`) for the default warehouse
    /// and the first store when they have none.
    stocks: Vec<ProductStockDoc>,
    #[schema(value_type = Option<Object>, required = true)]
    supplier_relation: Option<Value>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    attributes: Option<Value>,
    #[schema(value_type = Option<Object>, required = true)]
    meta: Option<Value>,
    /// Always empty.
    association_data: Vec<Value>,
    created_at: DateTime<Utc>,
    last_updated_at: DateTime<Utc>,
    #[schema(inline)]
    relationships: ProductRelationsDoc,
    #[schema(required = true)]
    vat_group_id: Option<Uuid>,
    export_enabled: bool,
    /// Sold at the till only; the web shop never lists it (Genesy addition).
    store_only: bool,
    /// Short public labels ("Raw", "Unheated"), in order.
    labels: Vec<String>,
    /// Main category (`/erp/product-categories`), same workspace; null = none (Genesy addition).
    #[schema(required = true)]
    category_id: Option<Uuid>,
    export_price_sync: bool,
    export_product_data: bool,
    /// `title`, `description_short`, `description`, `meta_title`, `meta_description`,
    /// `keywords`.
    #[schema(value_type = Object)]
    export_data: Value,
}

#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
struct ProductRelationsDoc {
    #[schema(required = true)]
    currency: Option<CurrencyResource>,
    #[schema(required = true)]
    active_special_offer: Option<OfferResource>,
    special_offers: Vec<OfferResource>,
    #[schema(value_type = Option<Object>, required = true)]
    sales_group: Option<Value>,
    #[schema(value_type = Option<Object>, required = true)]
    vat_group: Option<Value>,
    /// Always empty.
    custom_sections: Vec<Value>,
    /// Always empty.
    child_products: Vec<Value>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    parent_product: Option<Value>,
    /// Always null.
    #[schema(value_type = Option<Object>, required = true)]
    return_item: Option<Value>,
    /// Bundle parts (product shape plus `quantity`, 8 decimals), single GET only.
    #[schema(value_type = Vec<Object>)]
    child_products_from_set: Vec<Value>,
    /// Always empty.
    storage_locations: Vec<Value>,
}

/// One row of a product's `stocks`.
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
#[schema(as = ProductStock)]
struct ProductStockDoc {
    /// Stock id, or `fallback-<warehouse id>` for a zero row.
    id: String,
    /// 2 decimals.
    stock_lvl: String,
    /// 2 decimals.
    reserved: String,
    minimal_stock_level: i32,
    maximum_stock_level: i32,
    /// `id`, `name`, `alias`, `color`, `icon`, `is_store`, `pickup_enabled`, `location`.
    #[schema(value_type = Option<Object>, required = true)]
    warehouse: Option<Value>,
    /// `id`, `shelf_code`, `shelf_name`.
    #[schema(value_type = Option<Object>, required = true)]
    shelf: Option<Value>,
    /// `id`, `bin_code`, `bin_name`.
    #[schema(value_type = Option<Object>, required = true)]
    bin: Option<Value>,
    /// `id`, `code`, `expires_on`; null for no-lot stock.
    #[schema(value_type = Option<Object>, required = true)]
    lot: Option<Value>,
    /// Zero rows only; always null.
    #[schema(value_type = Option<Object>)]
    product: Option<Value>,
    #[schema(required = true)]
    updated_at: Option<DateTime<Utc>>,
    #[schema(value_type = Option<Object>, required = true)]
    meta: Option<Value>,
}

/// OpenAPI shape of [`history_row`]: one price change.
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
#[schema(as = ProductPriceHistoryRow)]
pub struct HistoryDoc {
    id: Uuid,
    /// Price column that changed.
    field: String,
    /// 4 decimals.
    #[schema(required = true)]
    old_value: Option<String>,
    /// 4 decimals.
    #[schema(required = true)]
    new_value: Option<String>,
    changed_by_flow: String,
    created_at: DateTime<Utc>,
    /// Full name; left out when no user made the change.
    changed_by: Option<String>,
    /// `id`, `sku`; left out for product-level changes.
    #[schema(value_type = Option<Object>)]
    variant: Option<Value>,
}
