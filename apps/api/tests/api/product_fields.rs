//! Typed fields share native product transactions, permission gates and indexed filters.
use crate::common::Api;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn field(api: &Api, key: &str, kind: &str, extra: Value) -> String {
    let mut body = json!({"key":key,"label":key,"type":kind,"status":"active"});
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    api.with_base("/product-fields").create(body, "id").await
}
async fn patch_field(api: &Api, id: &str, body: Value) -> (StatusCode, Value) {
    api.with_base("/product-fields")
        .call(Method::PATCH, &format!("/{id}"), Some(body), &api.owner)
        .await
}
async fn product(api: &Api, values: Value) -> String {
    api.create(
        json!({"name":"Product","price":"1.25","custom_fields":values}),
        "product_id",
    )
    .await
}
fn query_filter(filters: Value) -> String {
    let text = filters.to_string();
    let encoded: String = text
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("?custom_filters={encoded}")
}
#[sqlx::test]
async fn six_types_round_trip_patch_and_native_duplicate(pool: PgPool) {
    let api = Api::new(pool, "/products").await;
    let text = field(&api, "Color", "text", json!({"max_length":10})).await;
    let number = field(&api, "weight", "number", json!({"min":"1","max":"20"})).await;
    let boolean = field(&api, "organic", "boolean", json!({})).await;
    let date = field(&api, "expires", "date", json!({})).await;
    let option = Uuid::now_v7();
    let select = field(
        &api,
        "grade",
        "select",
        json!({"options":[{"id":option,"label":"A"}]}),
    )
    .await;
    let multi = field(
        &api,
        "tags",
        "multiselect",
        json!({"options":[{"id":option,"label":"A"}]}),
    )
    .await;
    let values = json!({&text:"Red",&number:"12.5000",&boolean:false,&date:"2026-12-31",&select:option,&multi:[option]});
    let id = product(&api, values).await;
    let got = api.get(&id).await;
    assert_eq!(got["custom_fields"][&number], "12.5");
    assert_eq!(got["custom_fields"][&boolean], false);
    assert_eq!(got["custom_fields"][&date], "2026-12-31");
    assert_eq!(got["custom_fields"][&select], option.to_string());
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}"),
            Some(json!({"name":"Renamed","custom_fields":{&text:null}})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let got = api.get(&id).await;
    assert!(got["custom_fields"].get(&text).is_none());
    assert_eq!(got["custom_fields"][&number], "12.5");
    let (status, res) = api
        .with_base("/product-fields")
        .call(Method::DELETE, &format!("/{number}"), None, &api.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}/duplicate"),
            Some(json!({})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{res}");
    let copied = api.get(res["data"]["product_id"].as_str().unwrap()).await;
    assert_eq!(copied["custom_fields"], got["custom_fields"]);
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}"),
            Some(json!({"custom_fields":{&number:"10"}})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{res}");
}
#[sqlx::test]
async fn required_activation_counts_gaps_and_native_create_rolls_back(pool: PgPool) {
    let api = Api::new(pool.clone(), "/products").await;
    let id = product(&api, json!({})).await;
    let f = field(
        &api,
        "origin",
        "text",
        json!({"status":"draft","required":true}),
    )
    .await;
    let (status, res) = patch_field(&api, &f, json!({"status":"active"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{res}");
    assert!(
        res["errors"]["required"][0]
            .as_str()
            .unwrap()
            .contains("1 live products")
    );
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}"),
            Some(json!({"custom_fields":{&f:"TZ"}})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        patch_field(&api, &f, json!({"status":"active"})).await.0,
        StatusCode::OK
    );
    let (status, res) = api
        .call(
            Method::POST,
            "",
            Some(json!({"name":"Missing","price":"1"})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{res}");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM products WHERE workspace_id=$1::uuid")
            .bind(&api.ws)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "failed create cannot leave product or variant rows"
    );
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}"),
            Some(json!({"custom_fields":{&f:null}})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{res}");
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}/duplicate"),
            Some(json!({})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{res}");
}
#[sqlx::test]
async fn invalid_values_foreign_fields_and_definition_edits_are_rejected(pool: PgPool) {
    let api = Api::new(pool, "/products").await;
    let number = field(&api, "amount", "number", json!({"min":"1","max":"5"})).await;
    let text = field(
        &api,
        "short",
        "text",
        json!({"max_length":4,"nullable":false}),
    )
    .await;
    let other = api
        .as_new_owner("field-foreign@example.test", "other")
        .await;
    let foreign = field(&other, "foreign", "text", json!({})).await;
    for values in [
        json!({&number:2}),
        json!({&number:"5.00000001"}),
        json!({&number:"0.000000001"}),
        json!({&text:"12345"}),
        json!({&text:null}),
        json!({&foreign:"x"}),
    ] {
        let (status, res) = api
            .call(
                Method::POST,
                "",
                Some(json!({"name":"Bad","price":"1","custom_fields":values})),
                &api.owner,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{res}");
    }
    let id = product(&api, json!({&number:"5"})).await;
    assert_eq!(
        patch_field(&api, &number, json!({"max":"4"})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        patch_field(&api, &number, json!({"min":null,"max":null}))
            .await
            .0,
        StatusCode::OK
    );
    let (status, res) = api
        .call(
            Method::POST,
            &format!("/{id}"),
            Some(json!({"custom_fields":{&number:null}})),
            &api.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(
        patch_field(&api, &number, json!({"type":"text"})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "used types stay immutable after clear"
    );
}
#[sqlx::test]
async fn typed_filters_apply_to_native_list_and_empty_page_count(pool: PgPool) {
    let api = Api::new(pool, "/products").await;
    let text = field(&api, "label", "text", json!({})).await;
    let number = field(&api, "count", "number", json!({})).await;
    let date = field(&api, "date", "date", json!({})).await;
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    let multi = field(
        &api,
        "tags",
        "multiselect",
        json!({"options":[{"id":a,"label":"A"},{"id":b,"label":"B"}]}),
    )
    .await;
    let first = product(
        &api,
        json!({&text:"50%_Tea",&number:"10",&date:"2026-12-31",&multi:[a]}),
    )
    .await;
    product(
        &api,
        json!({&text:"50XXTea",&number:"1",&date:"2026-01-01",&multi:[b]}),
    )
    .await;
    for filters in [
        json!([{"field_id":text,"operator":"contains","value":"%_tea"}]),
        json!([{"field_id":number,"operator":"gte","value":"5"}]),
        json!([{"field_id":date,"operator":"gt","value":"2026-06-01"}]),
        json!([{"field_id":multi,"operator":"any","value":[a]}]),
        json!([{"field_id":multi,"operator":"contains","value":a}]),
    ] {
        let path = query_filter(filters);
        let rows = api.list(&path).await;
        assert_eq!(rows["data"].as_array().unwrap().len(), 1, "{rows}");
        assert_eq!(rows["data"][0]["id"], first);
        let past = api.list(&format!("{path}&page=2&per_page=1")).await;
        assert!(past["data"].as_array().unwrap().is_empty());
        assert_eq!(past["meta"]["total"], 1, "{past}");
    }
}
#[sqlx::test]
async fn definitions_admin_only_and_exact_keys_are_distinct(pool: PgPool) {
    let api = Api::new(pool, "/products").await;
    let fields = api.with_base("/product-fields");
    field(&api, "Color", "text", json!({})).await;
    field(&api, "color", "text", json!({})).await;
    let member = crate::common::member_with(
        &api.app,
        &api.pool,
        &api.ws,
        "field-member@example.test",
        &["product.view"],
    )
    .await;
    let (status, res) = fields
        .call(
            Method::POST,
            "",
            Some(json!({"key":"another","label":"Another","type":"text"})),
            &member,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{res}");
}
