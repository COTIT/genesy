//! Product rules: numbering, Laravel defaults, price normalisation, price history, default
//! variant, bundle children, embeds for the resource. Writes lock the row and write it whole.

use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    http::list::Page,
    modules::{
        currencies::{dto::CurrencyResource, repo as currencies},
        discounts, invoices, price_corrections,
        product_images::service as product_images,
        products::{
            dto::{BAD_CURRENCY, Fields, PHYSICAL, SET_TYPES, bad_id},
            pricing::{Priority, normalise, out_of_range},
            repo::{self, ListFilter, PriceChange, ProductRow},
            resource::{self, Embeds},
        },
        relations::service as relations,
        sales_groups::{dto::SalesGroupResource, repo as sales_groups},
        settings, special_offers,
        stock::repo as stock,
        vat_groups::repo as vat_groups,
        warehouses::{dto::WarehouseCompacted, repo as warehouses, service as warehouse_service},
        webhooks::events,
    },
};

/// Who changed prices, for price history (Laravel Context `price_change_*`).
#[derive(Clone, Copy)]
pub struct Actor {
    pub user_id: Uuid,
    /// `manual` from the product form, `system` for derived changes (duplicate).
    pub flow: &'static str,
}

/// One rendered product (GET one); `pos_warehouse` shapes the `pos_*` fields.
pub async fn get(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
    pos_warehouse: Option<Uuid>,
) -> Result<Value, AppError> {
    let row = repo::find(&state.db, workspace_id, parse(id)?)
        .await?
        .ok_or(AppError::NotFound)?;
    render_one(state, workspace_id, row, pos_warehouse).await
}

/// Ok when `id` is a live product of the workspace, else 404.
pub async fn exists(state: &AppState, workspace_id: Uuid, id: &str) -> Result<(), AppError> {
    repo::find(&state.db, workspace_id, parse(id)?)
        .await?
        .map(|_| ())
        .ok_or(AppError::NotFound)
}

/// Exact barcode/EAN/id lookup (POS scan), else 404.
pub async fn search(state: &AppState, workspace_id: Uuid, code: &str) -> Result<Value, AppError> {
    let row = repo::by_code(&state.db, workspace_id, code)
        .await?
        .ok_or(AppError::NotFound)?;
    render_one(state, workspace_id, row, None).await
}

async fn render_one(
    state: &AppState,
    workspace_id: Uuid,
    row: ProductRow,
    pos_warehouse: Option<Uuid>,
) -> Result<Value, AppError> {
    let mut e = embeds(state, workspace_id, std::slice::from_ref(&row), true).await?;
    e.pos_warehouse = pos_warehouse;
    Ok(resource::render(&row, &e, true))
}

/// A product in list shape (embedded in variants), else 404.
pub async fn render_index(
    state: &AppState,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(ProductRow, Value), AppError> {
    let row = repo::find(&state.db, workspace_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let e = embeds(state, workspace_id, std::slice::from_ref(&row), false).await?;
    let v = resource::render(&row, &e, false);
    Ok((row, v))
}

/// Live products among `ids` in list shape, by id (stock lists embed them).
pub async fn render_many(
    state: &AppState,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = repo::find_many(&state.db, workspace_id, ids).await?;
    let e = embeds(state, workspace_id, &rows, false).await?;
    Ok(rows
        .iter()
        .map(|p| (p.id, resource::render(p, &e, false)))
        .collect())
}

/// One list page (rendered) plus total; `pos_warehouse` shapes the `pos_*` fields.
pub async fn list(
    state: &AppState,
    workspace_id: Uuid,
    f: &ListFilter<'_>,
    pos_warehouse: Option<Uuid>,
) -> Result<(Vec<Value>, i64), AppError> {
    let (rows, mut total) = repo::list(&state.db, workspace_id, f).await?;
    if rows.is_empty() && f.offset > 0 {
        total = repo::count(&state.db, workspace_id, f).await?;
    }
    let mut e = embeds(state, workspace_id, &rows, false).await?;
    e.pos_warehouse = pos_warehouse;
    Ok((
        rows.iter()
            .map(|p| resource::render(p, &e, false))
            .collect(),
        total,
    ))
}

/// Create (Laravel CreateProduct + ProductCreated) in one transaction; returns the new id.
pub async fn create(
    state: &AppState,
    workspace_id: Uuid,
    f: Fields,
    actor: Actor,
) -> Result<Uuid, AppError> {
    let mut tx = state.db.begin().await?;
    let id = create_in(&mut tx, workspace_id, f, actor).await?;
    tx.commit().await?;
    Ok(id)
}

pub(crate) async fn create_in(
    conn: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    f: Fields,
    actor: Actor,
) -> Result<Uuid, AppError> {
    lock_identity(conn, workspace_id).await?;
    // Lock first, then read the series, so the snapshot is the latest settings.
    repo::lock_numbers(conn, workspace_id).await?;
    let mut row = prepare_create(conn, workspace_id, &f).await?;
    super::approval::check_product(conn, workspace_id, &row).await?;
    let (appendix, start) =
        settings::service::numbering(&mut **conn, workspace_id, "product").await?;
    row.product_number = repo::next_number(conn, workspace_id, start).await?;
    row.product_nr_formatted = appendix.format(row.product_number);
    row.id = repo::insert(conn, workspace_id, &appendix.to_json(), &row).await?;
    repo::insert_history(
        conn,
        workspace_id,
        row.id,
        None,
        &created_prices(&row, f.gross_price.is_some()),
        Some(actor.user_id),
        actor.flow,
    )
    .await?;
    // Default variant (Laravel ProductCreated): its saving hook normalises the product price
    // against itself, so the price rounds to cents and `original_price` keeps the raw one.
    let (variant_price, variant_impact) = normalise(
        Some(row.price),
        Some(row.price),
        row.impact_on_price,
        Priority::Price,
    )
    .map_err(out_of_range)?;
    let variant_row = ProductRow {
        price: variant_price.unwrap_or(row.price),
        ..row.clone()
    };
    let variant =
        repo::insert_default_variant(conn, workspace_id, &variant_row, row.price, variant_impact)
            .await?;
    repo::insert_history(
        conn,
        workspace_id,
        row.id,
        Some(variant),
        &created_prices(&variant_row, true),
        Some(actor.user_id),
        actor.flow,
    )
    .await?;
    if let Some(children) = bundle_children(&row, &f) {
        repo::set_children(conn, workspace_id, row.id, children).await?;
    }
    if row.has_stock {
        let main = stock::seed(conn, workspace_id, row.id).await?;
        // Laravel overwrote a chosen default with the main warehouse; only fill a missing one.
        if let Some(main) = main
            && row.default_warehouse_id.is_none()
        {
            repo::set_default_warehouse(conn, workspace_id, row.id, main).await?;
        }
    }
    crate::modules::product_fields::service::apply(
        conn,
        workspace_id,
        row.id,
        f.custom_fields.as_ref(),
    )
    .await?;
    events::product_changed(conn, workspace_id, row.id, "created").await?;
    Ok(row.id)
}

/// Patch (Laravel UpdateProduct): lock, merge, normalise, write, record price changes.
pub async fn update(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
    f: Fields,
    actor: Actor,
) -> Result<(), AppError> {
    let id = parse(id)?;
    let mut tx = state.db.begin().await?;
    update_in(&mut tx, workspace_id, id, f, actor).await?;
    tx.commit().await?;
    Ok(())
}

/// Native mutation inside the caller's fenced transaction.
pub(crate) async fn update_in(
    conn: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
    f: Fields,
    actor: Actor,
) -> Result<(), AppError> {
    lock_identity(conn, workspace_id).await?;
    let before = repo::find_locked(conn, workspace_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let row = prepare_update(conn, workspace_id, &before, &f).await?;
    super::approval::check_product(conn, workspace_id, &row).await?;
    repo::update(conn, workspace_id, &row).await?;
    if row.price != before.price {
        // Inheriting variants (the default size made with the product) follow, priced as at
        // create.
        let (price, impact) = normalise(
            Some(row.price),
            Some(row.price),
            row.impact_on_price,
            Priority::Price,
        )
        .map_err(out_of_range)?;
        let price = price.unwrap_or(row.price);
        repo::sync_inheriting_variants(conn, workspace_id, id, price, row.price, impact).await?;
    }
    let changes = changed_prices(&before, &row);
    repo::insert_history(
        conn,
        workspace_id,
        id,
        None,
        &changes,
        Some(actor.user_id),
        actor.flow,
    )
    .await?;
    if let Some(children) = bundle_children(&row, &f) {
        repo::set_children(conn, workspace_id, id, children).await?;
    }
    crate::modules::product_fields::service::apply(
        conn,
        workspace_id,
        id,
        f.custom_fields.as_ref(),
    )
    .await?;
    events::product_changed(conn, workspace_id, id, "updated").await?;
    Ok(())
}

/// Laravel DuplicateProduct: create again from the stored columns, with a new number.
pub async fn duplicate(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
    user_id: Uuid,
) -> Result<Uuid, AppError> {
    let mut tx = state.db.begin().await?;
    lock_identity(&mut tx, workspace_id).await?;
    repo::lock_numbers(&mut tx, workspace_id).await?;
    let src = repo::find_locked(&mut tx, workspace_id, parse(id)?)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut fields = duplicate_fields(&src);
    let values = crate::modules::product_fields::repo::values(&mut *tx, workspace_id, &[src.id])
        .await?
        .remove(&src.id)
        .unwrap_or_else(|| serde_json::json!({}));
    let defs =
        crate::modules::product_fields::repo::definitions(&mut tx, workspace_id, true).await?;
    fields.custom_fields = Some(
        values
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| {
                defs.iter()
                    .any(|d| d.id.to_string() == **key && d.status != "archived")
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    let id = create_in(
        &mut tx,
        workspace_id,
        fields,
        Actor {
            user_id,
            flow: "system",
        },
    )
    .await?;
    crate::modules::product_fields::service::copy(&mut tx, workspace_id, src.id, id).await?;
    tx.commit().await?;
    Ok(id)
}

/// Native copy fields, shared with approval quotes before any insert.
pub(super) fn duplicate_fields(src: &ProductRow) -> Fields {
    Fields {
        custom_fields: None,
        name: Some(src.name.clone()),
        brand: Some(src.brand.clone()),
        kind: Some(src.kind.clone()),
        status: Some(src.status.clone()),
        has_stock: Some(src.has_stock),
        is_serialized: Some(src.is_serialized),
        warranty_default_mode: Some(src.warranty_default_mode.clone()),
        warranty_duration_value: Some(src.warranty_duration_value),
        warranty_duration_unit: Some(src.warranty_duration_unit.clone()),
        reference: Some(src.reference.clone()),
        reference_vendor: Some(src.reference_vendor.clone()),
        reference_vendor2: Some(src.reference_vendor2.clone()),
        reference_vendor3: Some(src.reference_vendor3.clone()),
        ean13: Some(src.ean13.clone()),
        hs_code: Some(src.hs_code.clone()),
        barcode: Some(src.barcode.clone()),
        barcode_vendor: Some(src.barcode_vendor.clone()),
        supplier_discount_code: Some(src.supplier_discount_code.clone()),
        short_description: Some(src.short_description.clone()),
        description: Some(src.description.clone()),
        stock_level_supplier: Some(src.stock_level_supplier),
        supplier_relation_id: Some(src.supplier_relation_id),
        currency_id: Some(src.currency_id),
        vat_group_id: Some(src.vat_group_id),
        sales_group_id: Some(src.sales_group_id),
        category_id: Some(src.category_id),
        default_warehouse_id: Some(src.default_warehouse_id),
        price: Some(src.price),
        gross_price: Some(src.gross_price),
        wholesale_price: Some(src.wholesale_price),
        supplier_wholesale_price: Some(src.supplier_wholesale_price),
        impact_on_price: Some(src.impact_on_price),
        impact_on_wholesale_price: Some(src.impact_on_wholesale_price),
        order_unit: Some(src.order_unit),
        sale_unit: Some(src.sale_unit),
        weight: Some(src.weight),
        unit: Some(src.unit.clone()),
        dimensions: Some(src.dimensions.clone()),
        country_of_origin: Some(src.country_of_origin.clone()),
        url_vendor: Some(src.url_vendor.clone()),
        url_vendor_override: Some(src.url_vendor_override.clone()),
        exclude_price_corrections: Some(src.exclude_price_corrections),
        import_id: Some(src.import_id.clone()),
        import_source: Some(src.import_source.clone()),
        export_enabled: Some(src.export_enabled),
        store_only: Some(src.store_only),
        labels: Some(src.labels.clone()),
        export_price_sync: Some(src.export_price_sync),
        export_product_data: Some(src.export_product_data),
        export_data: Some(src.export_data.clone()),
        meta: Some(src.meta.clone()),
        set_children: None,
    }
}

/// Soft delete, else 404; 422 while an item line uses it.
pub async fn delete(state: &AppState, workspace_id: Uuid, id: &str) -> Result<(), AppError> {
    let id = parse(id)?;
    let mut tx = state.db.begin().await?;
    delete_in(&mut tx, workspace_id, id).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn delete_in(
    conn: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(), AppError> {
    lock_identity(conn, workspace_id).await?;
    if !repo::lock_for_delete(&mut **conn, workspace_id, id).await? {
        return Err(AppError::NotFound);
    }
    invoices::service::product_deletable(conn, workspace_id, id).await?;
    crate::modules::assistant::impact::check(conn, workspace_id, "product", id).await?;
    repo::soft_delete(&mut **conn, workspace_id, id).await?;
    events::product_changed(conn, workspace_id, id, "deleted").await?;
    Ok(())
}

/// One page of a live product's price history, else 404.
pub async fn history(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
    page: Page,
) -> Result<(Vec<Value>, i64), AppError> {
    let id = parse(id)?;
    if repo::find(&state.db, workspace_id, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    let (rows, mut total) =
        repo::history(&state.db, workspace_id, id, page.size, page.offset()).await?;
    if rows.is_empty() && page.offset() > 0 {
        total = repo::history_count(&state.db, workspace_id, id).await?;
    }
    Ok((rows.iter().map(resource::history_row).collect(), total))
}

/// Resolve create defaults and native price normalization without writing.
pub(crate) async fn prepare_create(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    f: &Fields,
) -> Result<ProductRow, AppError> {
    let mut row = blank(conn, workspace_id, f).await?;
    apply(&mut row, f);
    check(conn, workspace_id, &row, f).await?;
    normalise_row(&mut row, Priority::Price)?;
    Ok(row)
}

/// Merge a patch with the same native priority and reference checks as the write.
pub(crate) async fn prepare_update(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    before: &ProductRow,
    f: &Fields,
) -> Result<ProductRow, AppError> {
    let mut row = before.clone();
    apply(&mut row, f);
    check(conn, workspace_id, &row, f).await?;
    if row.gross_price != before.gross_price
        || row.price != before.price
        || row.impact_on_price != before.impact_on_price
    {
        let priority = if f.price.is_some_and(|p| p != before.price) {
            Priority::Price
        } else {
            Priority::Impact
        };
        normalise_row(&mut row, priority)?;
    }
    Ok(row)
}

/// A new row with Laravel's create defaults; main currency and VAT group filled in.
async fn blank(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    f: &Fields,
) -> Result<ProductRow, AppError> {
    let currency_id = match f.currency_id {
        Some(id) => id,
        None => repo::main_currency(&mut *conn, workspace_id)
            .await?
            .ok_or_else(|| AppError::field("currency_id", BAD_CURRENCY))?,
    };
    let vat_group_id = match f.vat_group_id {
        Some(v) => v,
        None => repo::fallback_vat_group(&mut *conn, workspace_id).await?,
    };
    let now = Utc::now();
    Ok(ProductRow {
        id: Uuid::nil(),
        product_number: 0,
        product_nr_formatted: String::new(),
        name: String::new(),
        brand: None,
        kind: "physical".into(),
        status: "active".into(),
        has_stock: true,
        is_serialized: false,
        warranty_default_mode: None,
        warranty_duration_value: None,
        warranty_duration_unit: None,
        reference: None,
        reference_vendor: None,
        reference_vendor2: None,
        reference_vendor3: None,
        ean13: None,
        hs_code: None,
        barcode: None,
        barcode_vendor: None,
        supplier_discount_code: None,
        short_description: None,
        description: None,
        stock_level_supplier: None,
        supplier_relation_id: None,
        currency_id,
        vat_group_id,
        sales_group_id: None,
        default_warehouse_id: None,
        price: Decimal::ZERO,
        gross_price: Decimal::ZERO,
        wholesale_price: None,
        supplier_wholesale_price: None,
        impact_on_price: None,
        impact_on_wholesale_price: None,
        order_unit: Decimal::ONE,
        sale_unit: Decimal::ONE,
        weight: None,
        unit: None,
        dimensions: None,
        country_of_origin: None,
        url_vendor: None,
        url_vendor_override: None,
        exclude_price_corrections: false,
        archived: false,
        import_id: None,
        import_source: None,
        export_enabled: false,
        store_only: false,
        labels: Vec::new(),
        category_id: None,
        export_price_sync: false,
        export_product_data: false,
        export_data: None,
        meta: None,
        created_at: now,
        updated_at: now,
    })
}

/// Merge sent fields. Dimensions, `has_stock` and `is_serialized` only stick on physical types,
/// judged by the type after this request (Laravel needed `type` in the same request).
fn apply(p: &mut ProductRow, f: &Fields) {
    macro_rules! set {
        ($($field:ident),* $(,)?) => {
            $( if let Some(v) = &f.$field { p.$field = v.clone(); } )*
        };
    }
    set!(
        brand,
        warranty_default_mode,
        warranty_duration_value,
        warranty_duration_unit,
        reference,
        reference_vendor,
        reference_vendor2,
        reference_vendor3,
        ean13,
        hs_code,
        barcode,
        barcode_vendor,
        supplier_discount_code,
        short_description,
        description,
        stock_level_supplier,
        supplier_relation_id,
        vat_group_id,
        sales_group_id,
        category_id,
        default_warehouse_id,
        wholesale_price,
        supplier_wholesale_price,
        impact_on_price,
        impact_on_wholesale_price,
        weight,
        unit,
        country_of_origin,
        url_vendor,
        url_vendor_override,
        import_id,
        import_source,
        export_data,
        meta,
    );
    if let Some(v) = &f.name {
        p.name = v.clone();
    }
    if let Some(v) = &f.kind {
        p.kind = v.clone();
    }
    if let Some(v) = &f.status {
        p.status = v.clone();
    }
    if let Some(v) = f.currency_id {
        p.currency_id = v;
    }
    for (src, dst) in [
        (f.price, &mut p.price),
        (f.gross_price, &mut p.gross_price),
        (f.order_unit, &mut p.order_unit),
        (f.sale_unit, &mut p.sale_unit),
    ] {
        if let Some(v) = src {
            *dst = v;
        }
    }
    for (src, dst) in [
        (
            f.exclude_price_corrections,
            &mut p.exclude_price_corrections,
        ),
        (f.export_enabled, &mut p.export_enabled),
        (f.store_only, &mut p.store_only),
        (f.export_price_sync, &mut p.export_price_sync),
        (f.export_product_data, &mut p.export_product_data),
    ] {
        if let Some(v) = src {
            *dst = v;
        }
    }
    if let Some(v) = &f.labels {
        p.labels = v.clone();
    }
    if PHYSICAL.contains(&p.kind.as_str()) {
        if let Some(v) = &f.dimensions {
            p.dimensions = v.clone();
        }
        if let Some(v) = f.has_stock {
            p.has_stock = v;
        }
        if let Some(v) = f.is_serialized {
            p.is_serialized = v;
        }
    }
    // Laravel saving hook: a falsy order unit is 1.
    if p.order_unit.is_zero() {
        p.order_unit = Decimal::ONE;
    }
}

/// Children to sync: only for a bundle type and a non-empty list (Laravel ProductCreated and
/// ProductUpdated); otherwise the stored children stay.
fn bundle_children<'a>(p: &ProductRow, f: &'a Fields) -> Option<&'a [(Uuid, Decimal)]> {
    f.set_children
        .as_deref()
        .filter(|c| !c.is_empty() && SET_TYPES.contains(&p.kind.as_str()))
}

/// Laravel's saving hook: with a gross price, price and markup follow each other.
fn normalise_row(p: &mut ProductRow, priority: Priority) -> Result<(), AppError> {
    let (price, impact) = normalise(
        Some(p.gross_price),
        Some(p.price),
        p.impact_on_price,
        priority,
    )
    .map_err(out_of_range)?;
    if let Some(price) = price {
        p.price = price;
    }
    p.impact_on_price = impact;
    Ok(())
}

/// References must be live rows of this workspace; bundle children live products other than
/// this one.
async fn check(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    p: &ProductRow,
    f: &Fields,
) -> Result<(), AppError> {
    let [supplier, vat, sales, warehouse, currency, category] = repo::refs_ok(
        &mut *conn,
        workspace_id,
        [
            f.supplier_relation_id.flatten(),
            f.vat_group_id.flatten(),
            f.sales_group_id.flatten(),
            f.default_warehouse_id.flatten(),
            f.currency_id,
            f.category_id.flatten(),
        ],
    )
    .await?;
    let mut errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (ok, field, domain) in [
        (supplier, "supplier_relation_id", "relation"),
        (vat, "vat_group_id", "vat_group"),
        (sales, "sales_group_id", "sales_group"),
        (warehouse, "default_warehouse_id", "warehouse"),
        (category, "category_id", "product_category"),
    ] {
        if !ok {
            errors.insert(field.into(), vec![bad_id(field, domain)]);
        }
    }
    if !currency {
        errors.insert("currency_id".into(), vec![BAD_CURRENCY.into()]);
    }
    if let Some(children) = &f.set_children {
        let ids: Vec<Uuid> = children.iter().map(|c| c.0).collect();
        let live = repo::live_ids(&mut *conn, workspace_id, &ids).await?;
        for (i, (child, _)) in children.iter().enumerate() {
            if *child == p.id || !live.contains(child) {
                let key = format!("multi_set_product_ids.{i}.id");
                errors.insert(key.clone(), vec![format!("The selected {key} is invalid.")]);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Validation(errors))
    }
}

/// Price fields recorded on create: every non-null one (Laravel RecordsPriceChanges). An unsent
/// gross price was null in Laravel (stored 0 here), so it gets no row.
fn created_prices(p: &ProductRow, gross_sent: bool) -> Vec<PriceChange> {
    [
        ("gross_price", gross_sent.then_some(p.gross_price)),
        ("price", Some(p.price)),
        ("supplier_wholesale_price", p.supplier_wholesale_price),
        ("wholesale_price", p.wholesale_price),
    ]
    .into_iter()
    .filter(|(_, v)| v.is_some())
    .map(|(f, v)| (f, None, v))
    .collect()
}

/// Price fields whose value changed.
fn changed_prices(a: &ProductRow, b: &ProductRow) -> Vec<PriceChange> {
    [
        ("gross_price", Some(a.gross_price), Some(b.gross_price)),
        ("price", Some(a.price), Some(b.price)),
        (
            "supplier_wholesale_price",
            a.supplier_wholesale_price,
            b.supplier_wholesale_price,
        ),
        ("wholesale_price", a.wholesale_price, b.wholesale_price),
    ]
    .into_iter()
    .filter(|(_, old, new)| old != new)
    .collect()
}

/// Load what `rows` embed, one query (or a few for the handful of distinct ids) per kind.
async fn embeds(
    state: &AppState,
    workspace_id: Uuid,
    rows: &[ProductRow],
    detail: bool,
) -> Result<Embeds, AppError> {
    let mut e = Embeds::default();
    if rows.is_empty() {
        return Ok(e);
    }
    let mut all: Vec<ProductRow> = rows.to_vec();
    let mut children: Vec<(Uuid, Uuid, Decimal)> = Vec::new();
    if detail {
        let ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
        children = repo::set_children_of(&state.db, workspace_id, &ids).await?;
        let child_ids: Vec<Uuid> = children.iter().map(|c| c.1).collect();
        if !child_ids.is_empty() {
            all.extend(repo::find_many(&state.db, workspace_id, &child_ids).await?);
        }
    }
    let ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
    for ((product, variant), links) in
        product_images::storefront_links(state, workspace_id, &ids).await?
    {
        if let (None, Some(first)) = (variant, links.into_iter().next()) {
            e.thumbnails.insert(product, first);
        }
    }
    e.vat_groups = vat_groups::all(&state.db, workspace_id)
        .await?
        .into_iter()
        .map(|g| (g.id, g))
        .collect();
    let currency_ids = distinct(all.iter().map(|p| Some(p.currency_id)));
    for c in currencies::find_many(&state.db, workspace_id, &currency_ids).await? {
        e.currencies
            .insert(c.id, to_value(CurrencyResource::from(c))?);
    }
    let group_ids = distinct(all.iter().map(|p| p.sales_group_id));
    for g in sales_groups::find_many(&state.db, workspace_id, &group_ids).await? {
        e.sales_groups
            .insert(g.id, to_value(SalesGroupResource::from(g))?);
    }
    let suppliers = distinct(all.iter().map(|p| p.supplier_relation_id));
    e.suppliers = relations::index_values(state, workspace_id, &suppliers).await?;
    let defaults = distinct(all.iter().map(|p| p.default_warehouse_id));
    for w in warehouses::compact_many(&state.db, workspace_id, &defaults).await? {
        e.warehouses
            .insert(w.id, to_value(WarehouseCompacted::from(&w))?);
    }
    let (refs, store) = warehouses::stock_refs(&state.db, workspace_id, &defaults).await?;
    e.stock_refs = refs.into_iter().map(|w| (w.id, w)).collect();
    e.store = store;
    let ids: Vec<Uuid> = all.iter().map(|p| p.id).collect();
    e.custom_fields =
        crate::modules::product_fields::repo::values(&state.db, workspace_id, &ids).await?;
    e.purchase_discounts = discounts::repo::purchase_discounts(&state.db, workspace_id, &ids)
        .await?
        .into_iter()
        .collect();
    e.bands = price_corrections::repo::bands(&state.db, workspace_id).await?;
    // Lists price from today's offers; only the detail prints every offer.
    e.today = chrono::Utc::now().date_naive();
    let active_on = (!detail).then_some(e.today);
    for o in special_offers::repo::for_products(&state.db, workspace_id, &ids, active_on).await? {
        if let Some(product) = o.product_id {
            e.offers.entry(product).or_default().push(o);
        }
    }
    e.business_from_gross = settings::repo::section(&state.db, workspace_id, "product")
        .await?
        .and_then(|s| s.get("disable_double_discount_for_business")?.as_bool())
        == Some(true);
    for line in stock::lines(&state.db, workspace_id, &ids).await? {
        e.stocks.entry(line.product_id).or_default().push(line);
    }
    if !detail {
        return Ok(e);
    }
    // Bundle children render in list shape (compact warehouse) before the detail rows' own
    // default warehouses switch to the full resource.
    let rendered: HashMap<Uuid, Value> = all[rows.len()..]
        .iter()
        .map(|c| (c.id, resource::render(c, &e, false)))
        .collect();
    for (parent, child, qty) in children {
        if let Some(v) = rendered.get(&child) {
            e.set_children
                .entry(parent)
                .or_default()
                .push((v.clone(), qty));
        }
    }
    let full = distinct(rows.iter().map(|p| p.default_warehouse_id));
    e.warehouses
        .extend(warehouse_service::resources(state, workspace_id, &full).await?);
    Ok(e)
}

fn distinct(ids: impl Iterator<Item = Option<Uuid>>) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = ids.flatten().collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn to_value(v: impl serde::Serialize) -> Result<Value, AppError> {
    serde_json::to_value(v).map_err(|e| AppError::Internal(e.to_string()))
}

/// Malformed id = unknown id = 404.
fn parse(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound)
}

/// Serialize identifier matching with every native catalog mutation and definition edit.
pub(crate) async fn lock_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ws: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SET LOCAL lock_timeout='5s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('product_identity:' || $1::text,0))",
    )
    .bind(ws)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Private import review data; never expose raw snapshots as public product resources.
pub(crate) async fn snapshot_in(
    conn: &mut PgConnection,
    ws: Uuid,
    id: Uuid,
) -> Result<Value, AppError> {
    let row = repo::find_locked(&mut *conn, ws, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let product = to_value(row.clone())?;
    let custom_fields = crate::modules::product_fields::repo::values(&mut *conn, ws, &[id])
        .await?
        .remove(&id)
        .unwrap_or_else(|| serde_json::json!({}));
    let variants:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(v)-'workspace_id' FROM product_variants v WHERE workspace_id=$1 AND product_id=$2 AND inherits_price AND deleted_at IS NULL ORDER BY id FOR SHARE").bind(ws).bind(id).fetch_all(&mut *conn).await?;
    let children:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(s) FROM product_sets s WHERE workspace_id=$1 AND product_id=$2 ORDER BY child_product_id").bind(ws).bind(id).fetch_all(&mut *conn).await?;
    Ok(
        serde_json::json!({"product":product,"custom_fields":custom_fields,"inheriting_variants":variants,"bundle_children":children,"dependencies":material_dependencies(conn,ws,&row).await?}),
    )
}
async fn material_dependencies(
    conn: &mut PgConnection,
    ws: Uuid,
    row: &ProductRow,
) -> Result<Value, AppError> {
    let currency = super::approval_repo::currency(conn, ws, row.currency_id).await?;
    let vat = match row.vat_group_id {
        Some(id) => super::approval_repo::vat(conn, ws, id).await?,
        None => None,
    };
    // Keep reference meanings steady through the caller's comparison and mutation.
    for (query, id) in [
        (
            "SELECT id FROM relations WHERE workspace_id=$1 AND id=$2 FOR SHARE",
            row.supplier_relation_id,
        ),
        (
            "SELECT id FROM product_categories WHERE workspace_id=$1 AND id=$2 FOR SHARE",
            row.category_id,
        ),
        (
            "SELECT id FROM sales_groups WHERE workspace_id=$1 AND id=$2 FOR SHARE",
            row.sales_group_id,
        ),
        (
            "SELECT id FROM warehouses WHERE workspace_id=$1 AND id=$2 FOR SHARE",
            row.default_warehouse_id,
        ),
    ] {
        if let Some(id) = id {
            sqlx::query(query)
                .bind(ws)
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
    }
    let mut fields = serde_json::to_value(
        crate::modules::product_fields::repo::definitions(conn, ws, true).await?,
    )
    .map_err(|e| AppError::Internal(e.to_string()))?;
    for field in fields.as_array_mut().unwrap() {
        field.as_object_mut().unwrap().remove("used");
    }
    let references:Value=sqlx::query_scalar("SELECT jsonb_build_object('supplier',(SELECT jsonb_build_object('id',id,'updated_at',updated_at,'deleted_at',deleted_at) FROM relations WHERE workspace_id=$1 AND id=$2),'category',(SELECT to_jsonb(c) FROM product_categories c WHERE workspace_id=$1 AND id=$3),'sales_group',(SELECT to_jsonb(s) FROM sales_groups s WHERE workspace_id=$1 AND id=$4),'warehouse',(SELECT to_jsonb(w) FROM warehouses w WHERE workspace_id=$1 AND id=$5))")
        .bind(ws).bind(row.supplier_relation_id).bind(row.category_id).bind(row.sales_group_id).bind(row.default_warehouse_id).fetch_one(&mut *conn).await?;
    sqlx::query("SELECT workspace_id FROM settings WHERE workspace_id=$1 FOR SHARE")
        .bind(ws)
        .execute(&mut *conn)
        .await?;
    let numbering =
        crate::modules::settings::service::section_or_default(&mut *conn, ws, "product").await?;
    let stock_warehouses:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(w) FROM warehouses w WHERE workspace_id=$1 AND deleted_at IS NULL AND (default_warehouse OR is_store) ORDER BY id FOR SHARE").bind(ws).fetch_all(&mut *conn).await?;
    Ok(
        serde_json::json!({"currency":currency,"vat":vat,"field_definitions":fields,"references":references,"numbering":numbering,"stock_warehouses":stock_warehouses}),
    )
}

/// Native projection plus complete before/dependency material for immutable import reviews.
pub(crate) async fn material_in(
    conn: &mut PgConnection,
    ws: Uuid,
    operation: &str,
    id: Option<Uuid>,
    values: &Value,
) -> Result<Value, AppError> {
    let mut before = if let Some(id) = id {
        Some(snapshot_in(conn, ws, id).await?)
    } else {
        None
    };
    if operation == "delete" {
        let id = id.ok_or(AppError::NotFound)?;
        let variants: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(v)-'workspace_id' FROM product_variants v WHERE workspace_id=$1 AND product_id=$2 AND deleted_at IS NULL ORDER BY id FOR SHARE")
            .bind(ws).bind(id).fetch_all(&mut *conn).await?;
        // A stock writer can hold one slot while inserting another (taking product KEY SHARE).
        // Refuse a busy snapshot rather than waiting while holding the product UPDATE lock.
        let stocks: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'warehouse_id',warehouse_id,'shelf_id',shelf_id,'bin_id',bin_id,'lot_id',lot_id,'stock_lvl',stock_lvl::text,'reserved',reserved::text,'minimal_stock_level',minimal_stock_level,'maximum_stock_level',maximum_stock_level) FROM stocks WHERE workspace_id=$1 AND product_id=$2 AND deleted_at IS NULL ORDER BY warehouse_id,bin_id NULLS FIRST,lot_id NULLS FIRST FOR SHARE NOWAIT")
            .bind(ws).bind(id).fetch_all(&mut *conn).await.map_err(|error| {
                if error.as_database_error().is_some_and(|e| e.code().as_deref()==Some("55P03")) {
                    AppError::Conflict("Stock is being changed. Prepare a fresh review after that write finishes.")
                } else { error.into() }
            })?;
        let nonzero_stock_rows = stocks
            .iter()
            .filter(|s| {
                ["stock_lvl", "reserved"].iter().any(|k| {
                    s[k].as_str()
                        .and_then(|s| s.parse::<Decimal>().ok())
                        .is_some_and(|n| !n.is_zero())
                })
            })
            .count();
        let before = before.as_mut().ok_or(AppError::NotFound)?;
        before["delete_impact"] = serde_json::json!({"variant_count":variants.len(),"stock_rows":stocks.len(),"nonzero_stock_rows":nonzero_stock_rows});
        before["all_variants"] = serde_json::json!(variants);
        before["stocks"] = serde_json::json!(stocks);
        return Ok(serde_json::json!({"before":before}));
    }
    let fields = super::dto::decode_values(values.clone(), operation == "create")?;
    let row = if operation == "create" {
        prepare_create(conn, ws, &fields).await?
    } else if operation == "update" {
        let current = repo::find_locked(&mut *conn, ws, id.ok_or(AppError::NotFound)?)
            .await?
            .ok_or(AppError::NotFound)?;
        prepare_update(conn, ws, &current, &fields).await?
    } else {
        return Err(AppError::field(
            "operation",
            "Choose create, update or delete.",
        ));
    };
    let custom_fields = crate::modules::product_fields::service::project(
        conn,
        ws,
        id,
        fields.custom_fields.as_ref(),
    )
    .await?;
    let mut projected = to_value(row.clone())?;
    projected.as_object_mut().unwrap().remove("created_at");
    projected.as_object_mut().unwrap().remove("updated_at");
    let bundle_children = if let Some(children) = &fields.set_children {
        let ids: Vec<Uuid> = children.iter().map(|c| c.0).collect();
        sqlx::query_scalar::<_,Value>("SELECT to_jsonb(p) FROM products p WHERE workspace_id=$1 AND id=ANY($2) ORDER BY id FOR SHARE").bind(ws).bind(ids).fetch_all(&mut *conn).await?
    } else {
        Vec::new()
    };
    Ok(
        serde_json::json!({"before":before,"projected":projected,"custom_fields":custom_fields,"dependencies":material_dependencies(conn,ws,&row).await?,"bundle_children":bundle_children}),
    )
}
