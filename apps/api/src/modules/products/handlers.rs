//! HTTP edge for /v1/workspace/{workspace}/products: `product.*` permission (via `Can`), call
//! service, wrap. Barcode search asks for `product.list`.
// Laravel's barcode search had no check.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    error::{AppError, ValidationErrorBody},
    http::{
        json::ValidatedJson,
        list::{Page, Paginated, search as pattern_of},
        response::{Data, Envelope, MessageBody},
        workspace::{Can, permissions},
    },
    modules::products::{
        dto::{
            CreateProductRequest, GetQuery, HistoryQuery, LinkedProducts, ListQuery,
            ProductCreated, ProductRequestDoc, SaleStatics, SearchBody, UpdateProductRequest,
            pos_warehouse,
        },
        repo::ListFilter,
        resource::{HistoryDoc, ProductDoc},
        service::{self, Actor},
    },
};

permissions! {
    ProductCreate => "product.create",
    ProductList => "product.list",
    ProductView => "product.view",
    ProductUpdate => "product.update",
    ProductDelete => "product.delete",
    ProductDuplicate => "product.duplicate",
}

type ProductPath = Path<(String, String)>;

/// Created / duplicated: `{data:{product_id}}`.
fn created(id: Uuid) -> (StatusCode, Data<Value>) {
    (StatusCode::CREATED, Data(json!({ "product_id": id })))
}

/// GET /products -> `{data:[...], meta}`, newest first, status `active` unless asked.
#[utoipa::path(
    get, path = "/workspace/{workspace}/products", tag = "products",
    operation_id = "products_list", summary = "List products",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ListQuery),
    responses(
        (status = 200, body = Paginated<ProductDoc>, description = "One page of products, newest first"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.list`"),
        (status = 404, body = MessageBody, description = "Unknown workspace, or caller not a member"),
        (status = 422, body = ValidationErrorBody, description = "Search text too long, malformed `supplier_id`, `category_id` or `available_lte`, or too many `exclude_ids`"),
    ),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<AppState>,
    access: Can<ProductList>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Paginated<Value>>, AppError> {
    let pattern = pattern_of(q.q.as_deref().or(q.search.as_deref()))?;
    // Laravel: `limit`, else `page_size`; vimdesk-web and the RN app send `per_page`.
    let size = q
        .limit
        .as_deref()
        .or(q.per_page.as_deref())
        .or(q.page_size.as_deref());
    let page = Page::parse(q.page.as_deref(), size);
    let supplier = match q
        .supplier_id
        .as_deref()
        .or(q.supplier_relation_id.as_deref())
    {
        Some(s) if !s.trim().is_empty() => Some(Uuid::parse_str(s.trim()).map_err(|_| {
            AppError::field("supplier_id", "The supplier id must be a valid UUID.")
        })?),
        _ => None,
    };
    let category = match q.category_id.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(Uuid::parse_str(s).map_err(|_| {
            AppError::field("category_id", "The category id must be a valid UUID.")
        })?),
        _ => None,
    };
    let exclude: Vec<Uuid> = q
        .exclude_ids
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| Uuid::parse_str(s.trim()).ok())
        .collect();
    if exclude.len() > 100 {
        return Err(AppError::field(
            "exclude_ids",
            "The exclude ids must not have more than 100 items.",
        ));
    }
    let status = q
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("active");
    let ean13 = q.ean13.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let available_lte = match q.available_lte.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(s.parse::<rust_decimal::Decimal>().map_err(|_| {
            AppError::field("available_lte", "The available lte must be a number.")
        })?),
        _ => None,
    };
    let raw_filters: Vec<crate::modules::product_fields::dto::FieldFilter> =
        match q.custom_filters.as_deref() {
            Some(text) if text.len() <= 65536 => serde_json::from_str(text).map_err(|_| {
                AppError::field("custom_filters", "Use an array of custom-field filters.")
            })?,
            Some(_) => {
                return Err(AppError::field(
                    "custom_filters",
                    "The filters are too large.",
                ));
            }
            None => Vec::new(),
        };
    let custom_filters = {
        let mut conn = state.db.acquire().await?;
        crate::modules::product_fields::service::validate_filters(
            &mut conn,
            access.workspace_id,
            &raw_filters,
        )
        .await?
    };
    let filter = ListFilter {
        custom_filters: &custom_filters,
        status,
        pattern: pattern.as_deref(),
        supplier,
        category,
        ean13,
        exclude: &exclude,
        available_lte,
        limit: page.size,
        offset: page.offset(),
    };
    let pos = pos_warehouse(q.warehouse_id.as_deref());
    let (data, total) = service::list(&state, access.workspace_id, &filter, pos).await?;
    Ok(Json(Paginated::new(data, total, page)))
}

/// GET /products/{product}.
#[utoipa::path(
    get, path = "/workspace/{workspace}/products/{product}", tag = "products",
    operation_id = "products_get", summary = "Get a product",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id"), GetQuery),
    responses(
        (status = 200, body = Envelope<ProductDoc>, description = "The product"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.view`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn get(
    State(state): State<AppState>,
    access: Can<ProductView>,
    Path((_, id)): ProductPath,
    Query(q): Query<GetQuery>,
) -> Result<Data<Value>, AppError> {
    let pos = pos_warehouse(q.warehouse_id.as_deref());
    Ok(Data(
        service::get(&state, access.workspace_id, &id, pos).await?,
    ))
}

/// POST /products/search `{query}` -> one product by barcode, EAN or id, else 404.
#[utoipa::path(
    post, path = "/workspace/{workspace}/products/search", tag = "products",
    operation_id = "products_search", summary = "Find one product by barcode, EAN or id",
    params(("workspace" = Uuid, Path, description = "Workspace id")),
    request_body = SearchBody,
    responses(
        (status = 200, body = Envelope<ProductDoc>, description = "The product"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.list`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
        (status = 422, body = ValidationErrorBody, description = "Invalid input"),
    ),
    security(("bearer" = []))
)]
pub async fn search(
    State(state): State<AppState>,
    access: Can<ProductList>,
    ValidatedJson(query): ValidatedJson<SearchBody>,
) -> Result<Data<Value>, AppError> {
    Ok(Data(
        service::search(&state, access.workspace_id, &query).await?,
    ))
}

/// POST /products -> 201 `{data:{product_id}}`.
#[utoipa::path(
    post, path = "/workspace/{workspace}/products", tag = "products",
    operation_id = "products_create", summary = "Create a product",
    params(("workspace" = Uuid, Path, description = "Workspace id")),
    request_body = ProductRequestDoc,
    responses(
        (status = 201, body = Envelope<ProductCreated>, description = "Created; returns the new id"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.create`"),
        (status = 404, body = MessageBody, description = "Unknown workspace, or caller not a member"),
        (status = 422, body = ValidationErrorBody, description = "Invalid input"),
    ),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<AppState>,
    access: Can<ProductCreate>,
    ValidatedJson(fields): ValidatedJson<CreateProductRequest>,
) -> Result<(StatusCode, Data<Value>), AppError> {
    let actor = Actor {
        user_id: access.user_id,
        flow: "manual",
    };
    let id = service::create(&state, access.workspace_id, fields, actor).await?;
    Ok(created(id))
}

/// POST /products/{product} -> `{"message":"ok"}`.
#[utoipa::path(
    post, path = "/workspace/{workspace}/products/{product}", tag = "products",
    operation_id = "products_update", summary = "Update a product (send only changed fields)",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    request_body = ProductRequestDoc,
    responses(
        (status = 200, body = MessageBody, description = "Done (`ok`)"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.update`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
        (status = 422, body = ValidationErrorBody, description = "Invalid input"),
    ),
    security(("bearer" = []))
)]
pub async fn update(
    State(state): State<AppState>,
    access: Can<ProductUpdate>,
    Path((_, id)): ProductPath,
    ValidatedJson(fields): ValidatedJson<UpdateProductRequest>,
) -> Result<crate::http::response::Message, AppError> {
    let actor = Actor {
        user_id: access.user_id,
        flow: "manual",
    };
    service::update(&state, access.workspace_id, &id, fields, actor).await?;
    Ok(crate::http::response::Message("ok"))
}

/// DELETE /products/{product} -> `{"data":{"message":"The Product has been deleted"}}`.
#[utoipa::path(
    delete, path = "/workspace/{workspace}/products/{product}", tag = "products",
    operation_id = "products_delete", summary = "Delete a product",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    responses(
        (status = 200, body = Envelope<MessageBody>, description = "Deleted; `data.message` confirms"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.delete`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
        (status = 422, body = MessageBody, description = "Product is used on invoice items"),
    ),
    security(("bearer" = []))
)]
pub async fn delete(
    State(state): State<AppState>,
    access: Can<ProductDelete>,
    Path((_, id)): ProductPath,
) -> Result<Data<Value>, AppError> {
    service::delete(&state, access.workspace_id, &id).await?;
    Ok(Data(json!({ "message": "The Product has been deleted" })))
}

/// POST /products/{product}/duplicate -> 201 `{data:{product_id}}`.
#[utoipa::path(
    post, path = "/workspace/{workspace}/products/{product}/duplicate", tag = "products",
    operation_id = "products_duplicate", summary = "Copy a product",
    description = "An exact copy; takes no changes. For another size, name or price, copy first, then update the copy.",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    responses(
        (status = 201, body = Envelope<ProductCreated>, description = "Copied; returns the new id"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.duplicate`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn duplicate(
    State(state): State<AppState>,
    access: Can<ProductDuplicate>,
    Path((_, id)): ProductPath,
) -> Result<(StatusCode, Data<Value>), AppError> {
    let id = service::duplicate(&state, access.workspace_id, &id, access.user_id).await?;
    Ok(created(id))
}

/// GET /products/{product}/price-history -> `{data:[...], meta}`; `per_page` default 25.
#[utoipa::path(
    get, path = "/workspace/{workspace}/products/{product}/price-history", tag = "products",
    operation_id = "products_price_history", summary = "List a product's price changes",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id"), HistoryQuery),
    responses(
        (status = 200, body = Paginated<HistoryDoc>, description = "One page of price changes, newest first"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.view`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn price_history(
    State(state): State<AppState>,
    access: Can<ProductView>,
    Path((_, id)): ProductPath,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Paginated<Value>>, AppError> {
    let mut page = Page::parse(q.page.as_deref(), q.per_page.as_deref());
    if q.per_page
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .is_none_or(|n| n < 1)
    {
        page.size = 25;
    }
    let (data, total) = service::history(&state, access.workspace_id, &id, page).await?;
    Ok(Json(Paginated::new(data, total, page)))
}

/// GET /products/{product}/sale-statics: sales figures from invoices. No invoices exist until
/// the final batch, so the answer is the empty-result shape.
#[utoipa::path(
    get, path = "/workspace/{workspace}/products/{product}/sale-statics", tag = "products",
    operation_id = "products_sale_statics", summary = "Get a product's sales figures",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    responses(
        (status = 200, body = Envelope<SaleStatics>, description = "Sales figures"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.view`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn sale_statics(
    State(state): State<AppState>,
    access: Can<ProductView>,
    Path((_, id)): ProductPath,
) -> Result<Data<Value>, AppError> {
    service::exists(&state, access.workspace_id, &id).await?;
    Ok(Data(json!({
        "units_sold": 0, "revenue": 0.0, "avg_monthly_sales": null, "last_sale": null,
        "customer_orders": 0, "last_order": null, "months_active": null,
    })))
}

/// GET /products/{product}/linked-products: links can't be created in genesy yet, so none.
#[utoipa::path(
    get, path = "/workspace/{workspace}/products/{product}/linked-products", tag = "products",
    operation_id = "products_linked_products", summary = "List linked products",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    responses(
        (status = 200, body = Envelope<LinkedProducts>, description = "Linked products"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.view`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn linked_products(
    State(state): State<AppState>,
    access: Can<ProductView>,
    Path((_, id)): ProductPath,
) -> Result<Data<Value>, AppError> {
    service::exists(&state, access.workspace_id, &id).await?;
    Ok(Data(json!({ "products": [] })))
}

/// POST /products/{product}/sync-product-stock-locations-from-external-service: no external
/// stock service is connected, so this answers `{"message":"ok"}`.
// Picqer isn't ported: Laravel's "credentials not configured" case.
#[utoipa::path(
    post, path = "/workspace/{workspace}/products/{product}/sync-product-stock-locations-from-external-service", tag = "products",
    operation_id = "products_sync_external", summary = "Sync stock locations from an external service (no-op)",
    params(("workspace" = Uuid, Path, description = "Workspace id"), ("product" = Uuid, Path, description = "Product id")),
    responses(
        (status = 200, body = MessageBody, description = "Done (`ok`)"),
        (status = 403, body = MessageBody, description = "Inactive membership or missing `product.view`"),
        (status = 404, body = MessageBody, description = "Unknown workspace or product, or caller not a member"),
    ),
    security(("bearer" = []))
)]
pub async fn sync_external(
    State(state): State<AppState>,
    access: Can<ProductView>,
    Path((_, id)): ProductPath,
) -> Result<crate::http::response::Message, AppError> {
    service::exists(&state, access.workspace_id, &id).await?;
    Ok(crate::http::response::Message("ok"))
}
