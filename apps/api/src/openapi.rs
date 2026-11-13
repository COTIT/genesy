//! OpenAPI document + the documented routers. `build_app` mounts [`public`] and [`private`]
//! and [`spec`] reads the same ones, so a route can't be served without being documented.
//! Written to `openapi.json` by `UPDATE_OPENAPI=1 cargo test --test openapi`.

use serde_json::{Value, json};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_axum::router::OpenApiRouter;

use crate::{
    AppState,
    error::{UnprocessableBody, ValidationErrorBody},
    http::response::MessageBody,
    modules::{
        analytics, app_keys, assistant, attention, auth, cash, contacts, currencies,
        currency_rates, custom_statuses, custom_types, deal_pipelines, deals, discount_groups,
        discounts, email_profiles, email_templates, email_verification, farm_bug_reports,
        farm_catalogue, farm_crops, farm_documents, farm_logins, farm_market, farm_memberships,
        farm_passbook, farm_payouts, farm_pue, farm_records, farm_supply, farm_supply_corrections,
        farm_surveys, farm_tower, farm_villages, farm_vouchers, files, flow_scenes, hourly_rates,
        invites, invoice_payments, invoices, lots, messages, orders, pdf_templates, permissions,
        price_corrections, pricing, product_brands, product_categories, product_images,
        product_variants, products, relations, sales_groups, scheduled_reports, search, settings,
        shipment_methods, special_offers, stock, storage_locations, storefront, storefront_design,
        supply_chain, supply_links, teams, till, till_catalog, till_sales,
        users::{self, dto::UserEmailResource},
        vat_groups, warehouse_bins, warehouse_locations, warehouse_rows, warehouse_sections,
        warehouse_shelves, warehouses, webhooks, workspaces,
    },
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Genesy API",
        description = "One backend for the Genesy shop, office, field and till apps. \
            Every private route needs `Authorization: Bearer <token>`.",
        license(name = "Proprietary")
    ),
    servers((url = "/v1")),
    // UserEmailResource: referenced by name from the hand-written team `relationships` schema.
    components(schemas(MessageBody, ValidationErrorBody, UnprocessableBody, UserEmailResource)),
    modifiers(&BearerAuth)
)]
struct ApiDoc;

/// A string enum schema from a Rust list, so the spec never holds a second copy of it.
pub fn string_enum<'a>(
    values: impl IntoIterator<Item = &'a str>,
    description: &str,
) -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
    use utoipa::openapi::schema::{ObjectBuilder, Type};
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(values))
        .description(Some(description))
        .into()
}

/// Registers the `bearer` scheme (user tokens), the `app_key` scheme (`gsk_…` app keys) and
/// the `till_session` scheme (till tokens), all the same header, that operations list under
/// `security`.
struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "assistant_bridge",
                SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Header(
                    utoipa::openapi::security::ApiKeyValue::new("Assistant-Bridge-Signature"),
                )),
            );
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
            );
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "app_key",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("gsk_ app key")
                        .description(Some("App key issued under /workspace/{workspace}/app-keys"))
                        .build(),
                ),
            );
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "till_session",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(
                            "Till session token from /workspace/{workspace}/till/login",
                        ))
                        .build(),
                ),
            );
    }
}

/// Documented routes that need no token; `build_app` puts them behind the IP rate limit.
pub(crate) fn public() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(assistant::routes::public())
        .merge(crate::modules::flow_conversations::routes::public())
        .merge(auth::routes::public())
        .merge(invites::routes::public())
        .merge(email_verification::routes::public())
        .merge(invoices::routes::public())
        .merge(crate::modules::invoices::gateway::public())
        .merge(crate::modules::gateway::handlers::public())
        .merge(lots::routes::public())
        .merge(farm_logins::routes::public())
        .merge(till::routes::public())
        .merge(orders::routes::public())
        .merge(scheduled_reports::routes::public())
}

/// Documented routes that need a token. Modules move here from `build_app`'s plain list
/// as they get `#[utoipa::path]` annotations.
pub(crate) fn private() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(assistant::routes::private())
        .merge(crate::modules::flow_conversations::routes::private())
        .merge(auth::routes::private())
        .merge(invites::routes::private())
        .merge(email_verification::routes::private())
        .merge(currencies::routes::private())
        .merge(currency_rates::routes::private())
        .merge(users::routes::private())
        .merge(permissions::routes::private())
        .merge(workspaces::routes::private())
        .merge(teams::routes::private())
        .merge(search::routes::private())
        .merge(attention::routes::private())
        .merge(scheduled_reports::routes::private())
        .merge(flow_scenes::routes::private())
        .merge(relations::routes::private())
        .merge(contacts::routes::private())
        .merge(custom_statuses::routes::private())
        .merge(custom_types::routes::private())
        .merge(shipment_methods::routes::private())
        .merge(sales_groups::routes::private())
        .merge(hourly_rates::routes::private())
        .merge(vat_groups::routes::private())
        .merge(storage_locations::routes::private())
        .merge(stock::routes::private())
        .merge(special_offers::routes::private())
        .merge(price_corrections::routes::private())
        .merge(discounts::routes::private())
        .merge(discount_groups::routes::private())
        .merge(product_variants::routes::private())
        .merge(products::routes::private())
        .merge(crate::modules::product_fields::routes::private())
        .merge(crate::modules::product_bulk_runs::routes::private())
        .merge(product_categories::routes::private())
        .merge(product_brands::routes::private())
        .merge(warehouse_bins::routes::private())
        .merge(warehouse_shelves::routes::private())
        .merge(warehouse_rows::routes::private())
        .merge(warehouse_sections::routes::private())
        .merge(warehouse_locations::routes::private())
        .merge(warehouses::routes::private())
        .merge(deal_pipelines::routes::private())
        .merge(deals::routes::private())
        .merge(invoice_payments::routes::private())
        .merge(crate::modules::invoices::gateway::private())
        .merge(crate::modules::gateway::handlers::private())
        .merge(pdf_templates::routes::private())
        .merge(email_templates::routes::private())
        .merge(email_profiles::routes::private())
        .merge(messages::routes::private())
        .merge(settings::routes::private())
        .merge(app_keys::routes::private())
        .merge(storefront::routes::private())
        .merge(storefront::connections::private())
        .merge(storefront::connections::public())
        .merge(storefront_design::routes::private())
        .merge(product_images::routes::private())
        .merge(webhooks::routes::private())
        .merge(lots::routes::private())
        .merge(farm_villages::routes::private())
        .merge(farm_memberships::routes::private())
        .merge(farm_crops::routes::private())
        .merge(farm_catalogue::routes::private())
        .merge(farm_pue::routes::private())
        .merge(farm_market::routes::private())
        .merge(farm_tower::routes::private())
        .merge(farm_records::routes::private())
        .merge(farm_logins::routes::private())
        .merge(farm_surveys::routes::private())
        .merge(farm_vouchers::routes::private())
        .merge(farm_supply::routes::private())
        .merge(farm_supply_corrections::routes::private())
        .merge(farm_passbook::routes::private())
        .merge(farm_payouts::routes::private())
        .merge(supply_links::routes::private())
        .merge(supply_chain::routes::private())
        .merge(orders::routes::private())
        .merge(analytics::routes::private())
        .merge(orders::routes::storefront())
        .merge(till::routes::private())
        .merge(till_catalog::routes::private())
        .merge(cash::routes::private())
        .merge(till_sales::routes::private())
}

/// Documented routes that need a token and take large bodies (item lines, invoices, uploads);
/// `build_app` gives them twice the body deadline.
pub(crate) fn documents() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(pricing::routes::private())
        .merge(invoices::routes::private())
        .merge(files::routes::private())
        .merge(farm_documents::routes::private())
        .merge(farm_bug_reports::routes::private())
}

/// Storefront preview-token resolve: app key, own rate limit in `build_app`.
pub(crate) fn previews() -> OpenApiRouter<AppState> {
    storefront_design::routes::previews()
}

/// Full spec, built from [`public`], [`private`] and [`documents`], as JSON.
pub fn spec() -> Result<Value, serde_json::Error> {
    let doc = ApiDoc::openapi()
        .merge_from(public().split_for_parts().1)
        .merge_from(private().split_for_parts().1)
        .merge_from(documents().split_for_parts().1)
        .merge_from(previews().split_for_parts().1);
    let mut doc = serde_json::to_value(doc)?;
    add_shared_responses(&mut doc);
    reference_named_schemas(&mut doc);
    Ok(doc)
}

/// Responses the middleware gives on any route (rate limits, pool/timeouts, body limits,
/// auth), added once here instead of on every handler. A handler's own entry wins.
fn add_shared_responses(doc: &mut Value) {
    let message =
        json!({ "application/json": { "schema": { "$ref": "#/components/schemas/MessageBody" } } });
    let retry = json!({ "Retry-After": {
        "description": "Seconds to wait before retrying",
        "schema": { "type": "integer" },
    }});
    let reply = |description: &str, headers: Option<&Value>| {
        let mut r = json!({ "description": description, "content": message });
        if let Some(h) = headers {
            r["headers"] = h.clone();
        }
        r
    };
    let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for op in paths
        .values_mut()
        .filter_map(Value::as_object_mut)
        .flat_map(|item| item.values_mut())
    {
        let has_body = op.get("requestBody").is_some();
        let secured = op.get("security").is_some();
        let assistant = op["tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|t| t == "assistant"));
        let bearer = op["security"]
            .as_array()
            .is_some_and(|ss| ss.iter().any(|s| s.get("bearer").is_some()));
        let Some(responses) = op.get_mut("responses").and_then(Value::as_object_mut) else {
            continue;
        };
        let mut add = |code: &str, r: Value| {
            responses.entry(code).or_insert(r);
        };
        if assistant {
            add(
                "403",
                reply("Workspace access or trusted bridge proof refused", None),
            );
            add(
                "404",
                reply("Unknown resource or another login/workspace", None),
            );
        }
        if bearer && !assistant {
            add(
                "400",
                reply("Malformed optional assistant action headers", None),
            );
            add(
                "409",
                reply(
                    "Optional assistant action approval unavailable, changed, expired or consumed",
                    None,
                ),
            );
        }
        add("429", reply("Too many requests", Some(&retry)));
        add("500", reply("Server error", None));
        add("503", reply("Busy or timed out; retry later", Some(&retry)));
        if has_body {
            add("408", reply("Request body arrived too slowly", None));
            add("413", reply("Request body too large", None));
        }
        if secured {
            add("401", reply("Missing, invalid or expired token", None));
        }
    }
}

/// utoipa pastes generic args inline (`Envelope<CurrencyResource>` holds a full copy of
/// `CurrencyResource`); codegen then emits a second, anonymous type. Swap every exact copy
/// of a named object schema for a `$ref`.
fn reference_named_schemas(doc: &mut Value) {
    let named: Vec<(String, Value)> = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .map(|schemas| {
            schemas
                .iter()
                .filter(|(_, s)| s.get("properties").is_some())
                .map(|(name, s)| (name.clone(), s.clone()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(paths) = doc.get_mut("paths") {
        replace_copies(paths, &named);
    }
    if let Some(schemas) = doc
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    {
        // A definition keeps its own body; only what it contains gets swapped.
        schemas
            .values_mut()
            .for_each(|s| replace_children(s, &named));
    }
}

fn replace_copies(v: &mut Value, named: &[(String, Value)]) {
    if let Some((name, _)) = named.iter().find(|(_, s)| s == v) {
        *v = json!({ "$ref": format!("#/components/schemas/{name}") });
        return;
    }
    replace_children(v, named);
}

fn replace_children(v: &mut Value, named: &[(String, Value)]) {
    match v {
        Value::Object(m) => m.values_mut().for_each(|c| replace_copies(c, named)),
        Value::Array(a) => a.iter_mut().for_each(|c| replace_copies(c, named)),
        _ => {}
    }
}
