//! Durable product changes: staging is inert, approval scopes are exact, chunks are fenced.
use crate::common::{self, send, signup};
use axum::{
    Router,
    http::{Method, StatusCode},
};
use genesy_360_api::{
    AppState,
    jobs::{self, kinds::Ctx, runner::Step},
    product_bulk_runs,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

struct World {
    app: Router,
    state: AppState,
    token: String,
    workspace: Uuid,
    base: String,
}

#[sqlx::test]
async fn malformed_or_unknown_json_is_a_structured_refusal(db: PgPool) {
    let w = world(db).await;
    for body in [
        json!(false),
        json!({"unexpected":"not a product import field"}),
    ] {
        let (status, value) = send(&w.app, Method::POST, &w.base, Some(body), Some(&w.token)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{value}");
        assert!(value["errors"]["body"].is_array(), "{value}");
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_bulk_runs WHERE workspace_id=$1")
            .bind(w.workspace)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
    assert_eq!(count, 0);
}
async fn world(db: PgPool) -> World {
    let mut state = common::state(db, genesy_360_api::mail::Mailer::capture().0);
    state.product_bulk_enabled = true;
    let app = genesy_360_api::build_app(state.clone());
    let (token, workspace) = signup(&app, "bulk@example.test", "bulk").await;
    let workspace = Uuid::parse_str(&workspace).unwrap();
    World {
        app,
        state,
        token,
        workspace,
        base: format!("/v1/workspace/{workspace}/product-bulk-runs"),
    }
}
async fn request(w: &World, method: Method, suffix: &str, body: Option<Value>) -> Value {
    let (status, value) = send(
        &w.app,
        method,
        &format!("{}{suffix}", w.base),
        body,
        Some(&w.token),
    )
    .await;
    assert!(status.is_success(), "{status}: {value}");
    value["data"].clone()
}
async fn native(w: &World, name: &str, price: &str) -> Uuid {
    let (status, value) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/products", w.workspace),
        Some(json!({"name":name,"price":price})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    Uuid::parse_str(value["data"]["product_id"].as_str().unwrap()).unwrap()
}
fn ctx(w: &World) -> Ctx {
    Ctx {
        invoice_state: Some(w.state.clone()),
        assistant_api_key: None,
        db: w.state.db.clone(),
        files: w.state.files.clone(),
        http: genesy_360_api::outbound::Outbound::new(&[]).unwrap(),
        credentials: w.state.credentials.clone(),
        payments: w.state.payments.clone(),
    }
}
async fn drain(w: &World) {
    for _ in 0..100 {
        let Some(job) =
            jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
                .await
                .unwrap()
        else {
            return;
        };
        let outcome = product_bulk_runs::worker::run(&ctx(w), &job).await.unwrap();
        match outcome {
            Step::Done => {
                jobs::repo::finish(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
            Step::More => {
                jobs::repo::requeue(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
        }
    }
    panic!("import did not finish bounded test work");
}
async fn stage(w: &World, rows: Value) -> (Uuid, i64) {
    let run = request(w, Method::POST, "", Some(json!({"mode":"create"}))).await;
    let id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let run = request(
        w,
        Method::POST,
        &format!("/{id}/stage"),
        Some(json!({"revision":run["revision"],"rows":rows})),
    )
    .await;
    (id, run["revision"].as_i64().unwrap())
}
async fn approve(w: &World, id: Uuid, revision: i64, scope: &str) -> Value {
    let review = request(
        w,
        Method::POST,
        &format!("/{id}/reviews"),
        Some(json!({"revision":revision,"scope":scope})),
    )
    .await;
    request(
        w,
        Method::POST,
        &format!("/{id}/approve"),
        Some(json!({"revision":revision,"review_id":review["id"]})),
    )
    .await;
    review
}

async fn chunk_with_native_ascending_locks(w: &World, first: Uuid, second: Uuid) {
    use std::time::Duration;
    let job = jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
        .await
        .unwrap()
        .unwrap();
    let mut native = w.state.db.begin().await.unwrap();
    let native_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *native)
        .await
        .unwrap();
    // Schedule the native invoice product-lock query between its two ascending IDs.
    sqlx::query("SELECT id FROM products WHERE workspace_id=$1 AND id=$2 FOR KEY SHARE")
        .bind(w.workspace)
        .bind(first)
        .execute(&mut *native)
        .await
        .unwrap();
    let context = ctx(w);
    let running = job.clone();
    let worker =
        tokio::spawn(async move { product_bulk_runs::worker::run(&context, &running).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>$1 AND $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock' AND query LIKE '%FROM products%')")
                .bind(native_pid).fetch_one(&w.state.db).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("worker did not reach its product lock");
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *native)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM products WHERE workspace_id=$1 AND id=$2 FOR KEY SHARE")
        .bind(w.workspace)
        .bind(second)
        .execute(&mut *native)
        .await
        .expect(
            "reverse CSV order must not hold the larger product while waiting for the smaller one",
        );
    native.commit().await.unwrap();
    match worker.await.unwrap().unwrap() {
        Step::Done => {
            jobs::repo::finish(&w.state.db, job.id, job.token)
                .await
                .unwrap();
        }
        Step::More => {
            jobs::repo::requeue(&w.state.db, job.id, job.token)
                .await
                .unwrap();
        }
    }
}

#[sqlx::test]
async fn reverse_csv_order_locks_products_in_native_order_before_prepare_and_apply(db: PgPool) {
    let w = world(db).await;
    let mut products = [
        native(&w, "First target", "1").await,
        native(&w, "Second target", "2").await,
    ];
    products.sort();
    let [first, second] = products;
    let (run, revision) = stage(
        &w,
        json!([
            {"operation":"update","target":{"product_id":second},"values":{"price":"12"}},
            {"operation":"update","target":{"product_id":first},"values":{"price":"11"}}
        ]),
    )
    .await;
    chunk_with_native_ascending_locks(&w, first, second).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(rows["rows"][0]["resolved_id"], second.to_string());
    assert_eq!(rows["rows"][1]["resolved_id"], first.to_string());
    approve(&w, run, revision, "write").await;
    chunk_with_native_ascending_locks(&w, first, second).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(rows["rows"][0]["receipt"]["product_id"], second.to_string());
    assert_eq!(rows["rows"][1]["receipt"]["product_id"], first.to_string());
}

#[sqlx::test]
async fn delete_review_stales_for_noninheriting_variants_and_stock_changes(db: PgPool) {
    let w = world(db).await;
    let (status, body) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/warehouses", w.workspace),
        Some(json!({"name":"Delete impact stock","is_store":true})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for change in ["variant", "stock", "busy_stock"] {
        let product = native(&w, &format!("Delete impact {change}"), "3").await;
        let (run, revision) = stage(
            &w,
            json!([
                {"operation":"delete","target":{"product_id":product}}
            ]),
        )
        .await;
        drain(&w).await;
        let material: Value =
            sqlx::query_scalar("SELECT material FROM product_bulk_rows WHERE run_id=$1")
                .bind(run)
                .fetch_one(&w.state.db)
                .await
                .unwrap();
        assert!(
            material["native"]["before"]["delete_impact"]["variant_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(material["native"]["before"]["stocks"].is_array());
        approve(&w, run, revision, "delete").await;
        let mut busy = None;
        if change == "variant" {
            let (status, body) = send(
                &w.app,
                Method::POST,
                &format!("/v1/workspace/{}/products/{product}/variants", w.workspace),
                Some(json!({"sku":"Independent","price":"9"})),
                Some(&w.token),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            let independent: i64 = sqlx::query_scalar("SELECT count(*) FROM product_variants WHERE product_id=$1 AND NOT inherits_price AND deleted_at IS NULL")
                .bind(product).fetch_one(&w.state.db).await.unwrap();
            assert!(independent > 0);
        } else if change == "stock" {
            // Native creation seeds a warehouse slot; change exact stock material after approval.
            let changed = sqlx::query("UPDATE stocks SET stock_lvl=stock_lvl+1 WHERE product_id=$1 AND deleted_at IS NULL")
                .bind(product).execute(&w.state.db).await.unwrap();
            assert!(changed.rows_affected() > 0);
        } else {
            let mut tx = w.state.db.begin().await.unwrap();
            sqlx::query("SELECT id FROM stocks WHERE product_id=$1 AND deleted_at IS NULL FOR NO KEY UPDATE")
                .bind(product).fetch_all(&mut *tx).await.unwrap();
            busy = Some(tx);
        }
        drain(&w).await;
        if let Some(tx) = busy {
            tx.rollback().await.unwrap();
        }
        let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
        assert_eq!(rows["rows"][0]["status"], "stale", "{rows}");
        let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM products WHERE id=$1")
            .bind(product)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
        assert!(live);
    }
}

#[sqlx::test]
async fn duplicate_source_keys_across_chunks_mark_every_participant_and_exclusion_recovers(
    db: PgPool,
) {
    let w = world(db).await;
    let run = request(
        &w,
        Method::POST,
        "",
        Some(json!({"mode":"create","match_key":"barcode"})),
    )
    .await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let sources = [Uuid::now_v7(), Uuid::now_v7()];
    for source in sources {
        sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state) VALUES($1,$2,$3,$4,'duplicates.csv','text/csv',1,repeat('0',64),'done')")
            .bind(source).bind(w.workspace).bind(run).bind(format!("genesy/product-bulk/{}/{run}/{source}",w.workspace)).execute(&w.state.db).await.unwrap();
    }
    let mut staged = Vec::new();
    for i in 0..27 {
        staged.push(json!({"operation":"create","values":{"name":format!("Row {i}"),"price":"1"},"source_id":if i<25 {sources[0]} else {sources[1]}}));
    }
    let staged = request(
        &w,
        Method::POST,
        &format!("/{run}/stage"),
        Some(json!({"revision":1,"rows":staged})),
    )
    .await;
    let revision = staged["revision"].as_i64().unwrap();
    for (ordinal, source, cells, columns) in [
        (
            1,
            sources[0],
            json!(["unmapped", "DUP", "First", "8"]),
            json!(["Mystery", "barcode", "name", "price"]),
        ),
        (
            2,
            sources[0],
            json!(["BAD", "Broken price", "bad"]),
            json!(["barcode", "name", "price"]),
        ),
        (
            26,
            sources[1],
            json!(["DUP", "Second", "9"]),
            json!(["barcode", "name", "price"]),
        ),
        (
            27,
            sources[1],
            json!(["BAD", "Valid counterpart", "9"]),
            json!(["barcode", "name", "price"]),
        ),
    ] {
        let raw = json!({"source_id":source,"sheet":"Sheet1","line":ordinal,"cells":cells,"columns":columns,"formula_columns":[]});
        sqlx::query("UPDATE product_bulk_rows SET raw=$3 WHERE run_id=$1 AND ordinal=$2")
            .bind(run)
            .bind(ordinal as i64)
            .bind(raw)
            .execute(&w.state.db)
            .await
            .unwrap();
    }
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    for index in [0usize, 1, 25, 26] {
        let row = &rows["rows"][index];
        assert_eq!(row["status"], "error", "{row}");
        assert!(
            row["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "duplicate_target"),
            "{row}"
        );
    }
    assert_eq!(rows["rows"][0]["values"]["barcode"], "DUP");
    assert_eq!(rows["rows"][1]["values"]["price"], "bad");
    assert_eq!(rows["rows"][1]["values"]["name"], "Broken price");
    let (status, body) = send(
        &w.app,
        Method::POST,
        &format!("{}/{run}/reviews", w.base),
        Some(json!({"revision":revision,"scope":"write"})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let mut revision = revision;
    for index in [0usize, 1] {
        let changed = request(
            &w,
            Method::PATCH,
            &format!(
                "/{run}/rows/{}",
                rows["rows"][index]["id"].as_str().unwrap()
            ),
            Some(json!({"revision":revision,"included":false})),
        )
        .await;
        revision = changed["revision"].as_i64().unwrap();
    }
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    for index in [25usize, 26] {
        assert_eq!(rows["rows"][index]["status"], "ready");
    }
    let review = request(
        &w,
        Method::POST,
        &format!("/{run}/reviews"),
        Some(json!({"revision":revision,"scope":"write"})),
    )
    .await;
    assert_eq!(review["row_count"], 25);
}

#[sqlx::test]
async fn duplicate_identity_uses_exact_keys_blank_fallback_and_resolved_products(db: PgPool) {
    let w = world(db).await;
    let product = native(&w, "Matched identity", "1").await;
    let (status, body) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/products/{product}", w.workspace),
        Some(json!({"barcode":"Existing"})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let run = request(
        &w,
        Method::POST,
        "",
        Some(json!({"mode":"create","match_key":"barcode"})),
    )
    .await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    request(&w,Method::POST,&format!("/{run}/stage"),Some(json!({"revision":1,"rows":[
        {"operation":"create","values":{"name":"Zero A","price":"1","barcode":"Zero"}},
        {"operation":"create","values":{"name":"Zero B","price":"2","barcode":"Zero"}},
        {"operation":"create","values":{"name":"Lower","price":"1","barcode":"case"}},
        {"operation":"create","values":{"name":"Upper","price":"1","barcode":"CASE"}},
        {"operation":"create","target":{"key":""},"values":{"name":"Masked A","price":"1","barcode":"Masked"}},
        {"operation":"create","target":{"key":"  "},"values":{"name":"Masked B","price":"2","barcode":"Masked"}},
        {"operation":"create","values":{"name":"Blank A","price":"1","barcode":""}},
        {"operation":"create","values":{"name":"Blank B","price":"1","barcode":""}},
        {"operation":"update","target":{"product_id":product},"values":{"price":"8"}},
        {"operation":"update","target":{"key":"Existing"},"values":{"price":"9"}}
    ]}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    for index in [0usize, 1, 4, 5, 8, 9] {
        assert_eq!(rows["rows"][index]["status"], "error", "{rows}");
        assert!(
            rows["rows"][index]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "duplicate_target")
        );
    }
    for index in [2usize, 3, 6, 7] {
        assert_eq!(rows["rows"][index]["status"], "ready", "{rows}");
    }
}

#[sqlx::test]
async fn id_matching_canonicalizes_uuid_keys_and_refuses_malformed_ids(db: PgPool) {
    let w = world(db).await;
    let product = native(&w, "UUID duplicate", "1").await;
    let unique = native(&w, "UUID unique", "1").await;
    let absent = Uuid::parse_str("019aabbc-ddaa-7abc-8abc-0123456789ab").unwrap();
    let run = request(
        &w,
        Method::POST,
        "",
        Some(json!({"mode":"create","match_key":"id"})),
    )
    .await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    request(&w,Method::POST,&format!("/{run}/stage"),Some(json!({"revision":1,"rows":[
        {"operation":"update","target":{"key":product.to_string().to_uppercase()},"values":{"price":"8"}},
        {"operation":"update","target":{"product_id":product},"values":{"price":"9"}},
        {"operation":"update","target":{"key":"not-a-uuid"},"values":{"price":"1"}},
        {"operation":"create","target":{"key":absent.to_string().to_uppercase()},"values":{"name":"Missing A","price":"1"}},
        {"operation":"create","target":{"key":absent},"values":{"name":"Missing B","price":"1"}},
        {"operation":"update","target":{"key":unique.to_string().to_uppercase()},"values":{"price":"2"}}
    ]}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    for index in [0usize, 1, 3, 4] {
        assert_eq!(rows["rows"][index]["status"], "error", "{rows}");
        assert!(
            rows["rows"][index]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["code"] == "duplicate_target")
        );
    }
    assert_eq!(rows["rows"][0]["resolved_id"], product.to_string());
    assert_eq!(rows["rows"][2]["status"], "error");
    assert!(
        rows["rows"][2]["issues"][0]["fields"]["target"][0]
            .as_str()
            .unwrap()
            .contains("valid product UUID")
    );
    assert_eq!(rows["rows"][5]["status"], "ready");
    assert_eq!(rows["rows"][5]["resolved_id"], unique.to_string());
}

async fn stored_csv(w: &World, text: &str) -> (Uuid, Uuid) {
    let run = request(w, Method::POST, "", Some(json!({"mode":"create"}))).await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let mut tx = w.state.db.begin().await.unwrap();
    let source = product_bulk_runs::service::save_source(
        &w.state,
        &mut tx,
        w.workspace,
        run,
        "headers.csv",
        "text/csv",
        text.as_bytes(),
    )
    .await
    .unwrap();
    product_bulk_runs::service::invalidate(&mut tx, w.workspace, run)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    product_bulk_runs::service::finish_source(&w.state, source, text.as_bytes().to_vec())
        .await
        .unwrap();
    (run, source)
}

async fn mocked_extracted_source(
    w: &World,
    media: &str,
    columns: Value,
    mapping: Value,
    raw_rows: Vec<Value>,
) -> (Uuid, Uuid, i64) {
    let run = request(w, Method::POST, "", Some(json!({"mode":"create"}))).await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let source = Uuid::now_v7();
    sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state,columns,suggested_mapping,cursor) VALUES($1,$2,$3,$4,'fixture',$5,1,repeat('0',64),'done',$6,$7,'{\"page\":3,\"total_pages\":2}')")
        .bind(source).bind(w.workspace).bind(run).bind(format!("genesy/product-bulk/{}/{run}/{source}",w.workspace)).bind(media)
        .bind(json!({"":columns.clone()})).bind(json!({"":mapping})).execute(&w.state.db).await.unwrap();
    let staged: Vec<Value> = raw_rows
        .iter()
        .map(|_| json!({"operation":"create","source_id":source,"values":{}}))
        .collect();
    let run_data = request(
        w,
        Method::POST,
        &format!("/{run}/stage"),
        Some(json!({"revision":1,"rows":staged})),
    )
    .await;
    for (i, mut raw) in raw_rows.into_iter().enumerate() {
        raw["source_id"] = json!(source);
        raw["columns"] = columns.clone();
        if raw.get("sheet").is_none() {
            raw["sheet"] = Value::Null;
        }
        raw["formula_columns"] = json!([]);
        sqlx::query("UPDATE product_bulk_rows SET raw=$3 WHERE run_id=$1 AND ordinal=$2")
            .bind(run)
            .bind(i as i64 + 1)
            .bind(raw)
            .execute(&w.state.db)
            .await
            .unwrap();
    }
    (run, source, run_data["revision"].as_i64().unwrap())
}

#[sqlx::test]
async fn source_money_requires_currency_confirmation_and_explicit_eur_correction_preserves_amount(
    db: PgPool,
) {
    let w = world(db).await;
    sqlx::query("UPDATE currencies SET default_currency=false WHERE workspace_id=$1")
        .bind(w.workspace)
        .execute(&w.state.db)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE currencies SET default_currency=true WHERE workspace_id=$1 AND lower(asset_type)='usd'",
    )
    .bind(w.workspace)
    .execute(&w.state.db)
    .await
    .unwrap();
    let eur: Uuid = sqlx::query_scalar(
        "SELECT id FROM currencies WHERE workspace_id=$1 AND lower(asset_type)='eur'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    for media in [
        "application/pdf",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ] {
        let (run,source,revision)=mocked_extracted_source(&w,media,json!(["Name","Net price EUR"]),json!({"Name":"name","Net price EUR":"price"}),vec![json!({"page":if media=="application/pdf"{Some(1)}else{None},"line":1,"cells":["EUR literal","1.25"]})]).await;
        drain(&w).await;
        let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
        let row = &rows["rows"][0];
        assert_eq!(row["status"], "error", "{rows}");
        assert!(
            row["issues"][0]["fields"]["currency_id"].is_array(),
            "{rows}"
        );
        assert_eq!(row["values"]["price"], "1.25");
        assert_eq!(row["values"]["name"], "EUR literal");
        let mapped=request(&w,Method::PATCH,&format!("/{run}/mapping"),Some(json!({"revision":revision,"source_id":source,"columns":{"Name":"name","Net price EUR":"price"}}))).await;
        drain(&w).await;
        let still = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
        assert_eq!(
            still["rows"][0]["status"], "error",
            "mapping price alone cannot confirm currency"
        );
        let unrelated=request(&w,Method::PATCH,&format!("/{run}/rows/{}",row["id"].as_str().unwrap()),Some(json!({"revision":mapped["revision"],"values":{"name":"Unrelated name correction","price":"1.25"}}))).await;
        drain(&w).await;
        let still = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
        assert_eq!(
            still["rows"][0]["status"], "error",
            "manual_override must not bypass currency guard"
        );
        assert!(still["rows"][0]["issues"][0]["fields"]["currency_id"].is_array());
        let corrected=request(&w,Method::PATCH,&format!("/{run}/rows/{}",row["id"].as_str().unwrap()),Some(json!({"revision":unrelated["revision"],"values":{"name":"EUR literal","price":"1.25","currency_id":eur}}))).await;
        drain(&w).await;
        approve(&w, run, corrected["revision"].as_i64().unwrap(), "write").await;
        drain(&w).await;
        let product: Uuid = sqlx::query_scalar(
            "SELECT (receipt->>'product_id')::uuid FROM product_bulk_rows WHERE run_id=$1",
        )
        .bind(run)
        .fetch_one(&w.state.db)
        .await
        .unwrap();
        let actual = sqlx::query("SELECT currency_id,price FROM products WHERE id=$1")
            .bind(product)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
        assert_eq!(actual.get::<Uuid, _>("currency_id"), eur);
        assert_eq!(actual.get::<Decimal, _>("price"), Decimal::new(125, 2));
    }
    let (native, _, _) = mocked_extracted_source(
        &w,
        "text/csv",
        json!(["name", "price"]),
        json!({}),
        vec![json!({"page":null,"line":1,"cells":["Canonical template","1.25"]})],
    )
    .await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{native}/rows"), None).await;
    assert_eq!(
        rows["rows"][0]["status"], "ready",
        "canonical spreadsheet contract retains documented default"
    );
    let (ai, source, revision) = mocked_extracted_source(
        &w,
        "text/csv",
        json!(["name", "price"]),
        json!({"name":"name","price":"price"}),
        vec![json!({"page":null,"line":1,"cells":["AI canonical-looking header","1.25"]})],
    )
    .await;
    sqlx::query("INSERT INTO product_bulk_ai_costs(workspace_id,run_id,source_id,request_key,upper_bound_usd,actual_usd,state) VALUES($1,$2,$3,$4,0,0,'settled')").bind(w.workspace).bind(ai).bind(source).bind(format!("ai-provenance:{source}")).execute(&w.state.db).await.unwrap();
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{ai}/rows"), None).await;
    assert_eq!(
        rows["rows"][0]["status"], "error",
        "durable AI provenance overrides canonical-looking header default"
    );
    let mapped=request(&w,Method::PATCH,&format!("/{ai}/mapping"),Some(json!({"revision":revision,"source_id":source,"columns":{"name":"name","price":"price"}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{ai}/rows"), None).await;
    let changed=request(&w,Method::PATCH,&format!("/{ai}/rows/{}",rows["rows"][0]["id"].as_str().unwrap()),Some(json!({"revision":mapped["revision"],"values":{"name":"Only name changed","price":"1.25"}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{ai}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "error");
    assert!(rows["rows"][0]["issues"][0]["fields"]["currency_id"].is_array());
    request(&w,Method::PATCH,&format!("/{ai}/rows/{}",rows["rows"][0]["id"].as_str().unwrap()),Some(json!({"revision":changed["revision"],"values":{"name":"Explicit EUR","price":"1.25","currency_id":eur}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{ai}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "ready");
}

#[sqlx::test]
async fn pdf_chat_mapping_never_reparses_or_skips_first_body_rows(db: PgPool) {
    let w = world(db).await;
    for media in ["application/pdf", "text/plain"] {
        let raws = if media == "application/pdf" {
            vec![
                json!({"page":1,"line":1,"cells":["Page one","1"]}),
                json!({"page":2,"line":1,"cells":["Page two","2"]}),
            ]
        } else {
            vec![json!({"page":null,"line":1,"cells":["Chat row","3"]})]
        };
        let expected = raws.len();
        let (run, source, revision) = mocked_extracted_source(
            &w,
            media,
            json!(["Name", "Price"]),
            json!({"Name":"name","Price":"price"}),
            raws,
        )
        .await;
        drain(&w).await;
        let mapped=request(&w,Method::PATCH,&format!("/{run}/mapping"),Some(json!({"revision":revision,"source_id":source,"header_row":1,"columns":{"Name":"name","Price":"price"}}))).await;
        assert_eq!(mapped["mapping"][format!("{source}:")]["header_row"], 0);
        let before:Value=sqlx::query_scalar("SELECT jsonb_build_object('state',state,'cursor',cursor,'generation',generation,'rows',(SELECT count(*) FROM product_bulk_rows WHERE run_id=$2)) FROM product_bulk_sources WHERE id=$1").bind(source).bind(run).fetch_one(&w.state.db).await.unwrap();
        assert_eq!(before["state"], "done");
        assert_eq!(before["rows"], expected);
        assert_eq!(before["cursor"], json!({"page":3,"total_pages":2}));
        assert_eq!(before["generation"], 1);
        // Defend legacy/malicious stored header metadata as well as normalizing the API input.
        sqlx::query("UPDATE product_bulk_runs SET mapping=jsonb_set(mapping,ARRAY[$2,'header_row'],'1') WHERE id=$1").bind(run).bind(format!("{source}:")).execute(&w.state.db).await.unwrap();
        drain(&w).await;
        let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
        assert_eq!(rows["rows"].as_array().unwrap().len(), expected);
        assert!(
            rows["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["status"] == "error"
                    && r["included"] == true
                    && r["issues"][0]["fields"]["currency_id"].is_array()),
            "{rows}"
        );
        let after: Value =
            sqlx::query_scalar("SELECT cursor FROM product_bulk_sources WHERE id=$1")
                .bind(source)
                .fetch_one(&w.state.db)
                .await
                .unwrap();
        assert_eq!(
            after, before["cursor"],
            "mapping must not call extraction/provider again"
        );
    }
}

#[sqlx::test]
async fn semantic_questions_keep_raw_and_mapped_values_until_explicit_mapping(db: PgPool) {
    let w = world(db).await;
    let question = "Confirm whether Code is barcode/reference, and Price net/gross/currency.";
    let (run,source,revision)=mocked_extracted_source(&w,"application/pdf",json!(["Name","Code","Price","Description"]),json!({"Name":"name","Description":"description"}),vec![json!({"page":1,"line":1,"cells":["Rice","0012","1.25","Kept after ambiguous columns"],"needs_input":question})]).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    let row = &rows["rows"][0];
    assert_eq!(row["status"], "error");
    assert_eq!(row["raw"]["needs_input"], question);
    assert_eq!(row["values"]["name"], "Rice");
    assert_eq!(row["values"]["description"], "Kept after ambiguous columns");
    assert!(
        row["issues"][0]["fields"]["mapping"][0]
            .as_str()
            .unwrap()
            .contains(question)
    );
    let mapped=request(&w,Method::PATCH,&format!("/{run}/mapping"),Some(json!({"revision":revision,"source_id":source,"header_row":0,"columns":{"Name":"name","Code":"barcode","Price":"price","Description":"description"}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "error");
    assert!(rows["rows"][0]["issues"][0]["fields"]["currency_id"].is_array());
    assert_eq!(rows["rows"][0]["values"]["barcode"], "0012");
    assert_eq!(rows["rows"][0]["values"]["price"], "1.25");
    assert_eq!(
        rows["rows"][0]["raw"]["needs_input"], question,
        "original clarification survives explicit resolution"
    );
    let eur: Uuid = sqlx::query_scalar(
        "SELECT id FROM currencies WHERE workspace_id=$1 AND lower(asset_type)='eur'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    let mut values = rows["rows"][0]["values"].clone();
    values["currency_id"] = json!(eur);
    request(
        &w,
        Method::PATCH,
        &format!("/{run}/rows/{}", rows["rows"][0]["id"].as_str().unwrap()),
        Some(json!({"revision":mapped["revision"],"values":values})),
    )
    .await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "ready");
}

#[sqlx::test]
async fn source_money_cannot_escape_currency_guard_through_remainder_or_original_retention(
    db: PgPool,
) {
    let w = world(db).await;
    let (original, source, _) = mocked_extracted_source(
        &w,
        "application/pdf",
        json!(["Name", "Net price EUR"]),
        json!({"Name":"name","Net price EUR":"price"}),
        vec![json!({"page":1,"line":1,"cells":["Remainder EUR","1.25"]})],
    )
    .await;
    drain(&w).await;
    request(&w, Method::POST, &format!("/{original}/cancel"), None).await;
    let remainder = request(&w, Method::POST, &format!("/{original}/remainder"), None).await;
    let remainder = Uuid::parse_str(remainder["id"].as_str().unwrap()).unwrap();
    // Original outcomes/source metadata eventually expire; the immutable provenance must suffice.
    sqlx::query("DELETE FROM product_bulk_runs WHERE id=$1")
        .bind(original)
        .execute(&w.state.db)
        .await
        .unwrap();
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{remainder}/rows"), None).await;
    let row = &rows["rows"][0];
    assert!(row["raw"].is_null());
    assert!(row["source_id"].is_null());
    assert_eq!(row["provenance"]["source_id"], source.to_string());
    assert_eq!(row["status"], "error");
    assert!(row["issues"][0]["fields"]["currency_id"].is_array());
    let eur: Uuid = sqlx::query_scalar(
        "SELECT id FROM currencies WHERE workspace_id=$1 AND lower(asset_type)='eur'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    let mut values = row["values"].clone();
    values["currency_id"] = json!(eur);
    let current = request(&w, Method::GET, &format!("/{remainder}"), None).await;
    let corrected = request(
        &w,
        Method::PATCH,
        &format!("/{remainder}/rows/{}", row["id"].as_str().unwrap()),
        Some(json!({"revision":current["revision"],"values":values})),
    )
    .await;
    drain(&w).await;
    approve(
        &w,
        remainder,
        corrected["revision"].as_i64().unwrap(),
        "write",
    )
    .await;
    drain(&w).await;
    let actual: Uuid = sqlx::query_scalar(
        "SELECT currency_id FROM products WHERE workspace_id=$1 AND name='Remainder EUR'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(actual, eur);
}

#[sqlx::test]
#[ignore = "requires GENESY_PRODUCT_PARSER_BIN pointing to the real API binary; run explicitly"]
async fn real_csv_header_candidates_retain_data_require_confirmation_and_allow_exclusion(
    db: PgPool,
) {
    assert!(
        std::env::var_os("GENESY_PRODUCT_PARSER_BIN").is_some(),
        "Set GENESY_PRODUCT_PARSER_BIN to the built debug API for actual child-process extraction"
    );
    let mut w = world(db).await;
    w.state.files = Some(genesy_360_api::storage::Store::memory());
    w.app = genesy_360_api::build_app(w.state.clone());
    let (run, _) = stored_csv(&w, "name,brand\nname,brand\n").await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(
        rows["rows"].as_array().unwrap().len(),
        1,
        "legitimate all-text row must never disappear"
    );
    let row = &rows["rows"][0];
    assert_eq!(row["raw"]["header_candidate"], true);
    assert_eq!(row["values"]["name"], "name");
    assert_eq!(row["values"]["brand"], "brand");
    assert_eq!(row["status"], "error");
    assert_eq!(row["issues"][0]["code"], "header_candidate");
    let current = request(&w, Method::GET, &format!("/{run}"), None).await;
    let (status, body) = send(
        &w.app,
        Method::POST,
        &format!("{}/{run}/reviews", w.base),
        Some(json!({"revision":current["revision"],"scope":"write"})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let confirmed=request(&w,Method::PATCH,&format!("/{run}/rows/{}",row["id"].as_str().unwrap()),
        Some(json!({"revision":current["revision"],"values":{"name":"name","brand":"brand","price":"0"}}))).await;
    drain(&w).await;
    let confirmed_rows = request(&w, Method::GET, &format!("/{run}/rows"), None).await;
    assert_eq!(
        confirmed_rows["rows"][0]["raw"]["header_candidate"], true,
        "immutable evidence survives confirmation"
    );
    assert_eq!(confirmed_rows["rows"][0]["status"], "ready");
    approve(&w, run, confirmed["revision"].as_i64().unwrap(), "write").await;
    drain(&w).await;
    let imported: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='name' AND brand='brand'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(imported, 1);

    // A genuine repeated header remains an explicit exclusion; other valid rows stay reviewable.
    let (repeated, _) = stored_csv(&w, "name,brand,price\nWidget,Acme,1\nname,brand,price\n").await;
    drain(&w).await;
    let current = request(&w, Method::GET, &format!("/{repeated}"), None).await;
    let rows = request(&w, Method::GET, &format!("/{repeated}/rows"), None).await;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 2);
    assert_eq!(rows["rows"][0]["status"], "ready");
    assert_eq!(rows["rows"][1]["issues"][0]["code"], "header_candidate");
    let review = request(
        &w,
        Method::POST,
        &format!("/{repeated}/reviews"),
        Some(json!({"revision":current["revision"],"scope":"write"})),
    )
    .await;
    assert_eq!(review["row_count"], 1);
    let excluded = request(
        &w,
        Method::PATCH,
        &format!(
            "/{repeated}/rows/{}",
            rows["rows"][1]["id"].as_str().unwrap()
        ),
        Some(json!({"revision":current["revision"],"included":false})),
    )
    .await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{repeated}/rows"), None).await;
    assert_eq!(rows["rows"][1]["status"], "excluded");
    let review = request(
        &w,
        Method::POST,
        &format!("/{repeated}/reviews"),
        Some(json!({"revision":excluded["revision"],"scope":"write"})),
    )
    .await;
    assert_eq!(review["row_count"], 1);

    let (changed, _) = stored_csv(&w, "name,brand\nbrand,name\n").await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{changed}/rows"), None).await;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    assert_eq!(rows["rows"][0]["raw"]["header_candidate"], true);
    assert_eq!(rows["rows"][0]["issues"][0]["code"], "header_candidate");

    let (headerless, source) = stored_csv(&w, "name,brand\n").await;
    let current = request(&w, Method::GET, &format!("/{headerless}"), None).await;
    request(&w,Method::PATCH,&format!("/{headerless}/mapping"),Some(json!({"revision":current["revision"],"source_id":source,"header_row":0,"columns":{"A":"name","B":"brand"}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{headerless}/rows"), None).await;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    assert_eq!(rows["rows"][0]["raw"]["header_candidate"], false);
    assert_eq!(rows["rows"][0]["values"]["name"], "name");
    assert!(
        rows["rows"][0]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["code"] != "header_candidate")
    );
}

#[sqlx::test]
async fn staging_is_inert_and_delete_has_separate_immutable_scope(db: PgPool) {
    let w = world(db).await;
    let old = native(&w, "Old product", "2").await;
    let (id, revision) = stage(
        &w,
        json!([
            {"operation":"create","values":{"name":"New product","price":"1.25"}},
            {"operation":"create","values":{"name":"","price":"1"}},
            {"operation":"delete","target":{"product_id":old}}
        ]),
    )
    .await;
    drain(&w).await;
    let new: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='New product'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(new, 0);
    let review = approve(&w, id, revision, "write").await;
    assert_eq!(review["row_count"], 1);
    let row = request(&w, Method::GET, &format!("/{id}/rows"), None).await["rows"][0]["id"].clone();
    let (status, _) = send(
        &w.app,
        Method::PATCH,
        &format!("{}/{id}/rows/{}", w.base, row.as_str().unwrap()),
        Some(json!({"revision":revision,"included":false})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    drain(&w).await;
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM products WHERE id=$1")
            .bind(old)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
    assert!(!deleted);
    let review = approve(&w, id, revision, "delete").await;
    assert_eq!(review["row_count"], 1);
    drain(&w).await;
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM products WHERE id=$1")
            .bind(old)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
    assert!(deleted);
    let run = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(run["counts"]["applied"], 2);
    assert_eq!(run["counts"]["errors"], 1);
    assert_eq!(run["permissions"]["edit"], false);
}

#[sqlx::test]
async fn changed_material_stales_row_without_replaying_and_remainder_gets_fresh_review(db: PgPool) {
    let w = world(db).await;
    let product = native(&w, "Rice", "3").await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"update","target":{"product_id":product},"values":{"price":"4"}}]),
    )
    .await;
    drain(&w).await;
    approve(&w, id, revision, "write").await;
    let (status, value) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/products/{product}", w.workspace),
        Some(json!({"price":"5"})),
        Some(&w.token),
    )
    .await;
    assert!(status.is_success(), "{value}");
    drain(&w).await;
    let price: Decimal = sqlx::query_scalar("SELECT price FROM products WHERE id=$1")
        .bind(product)
        .fetch_one(&w.state.db)
        .await
        .unwrap();
    assert_eq!(price, Decimal::from(5));
    let rows = request(&w, Method::GET, &format!("/{id}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "stale");
    let rest = request(&w, Method::POST, &format!("/{id}/remainder"), None).await;
    let rest = Uuid::parse_str(rest["id"].as_str().unwrap()).unwrap();
    let copied = request(&w, Method::GET, &format!("/{rest}/rows"), None).await;
    assert_eq!(
        copied["rows"][0]["provenance"]["origin_run_id"],
        id.to_string()
    );
    assert_eq!(
        copied["rows"][0]["provenance"]["row_id"],
        rows["rows"][0]["id"]
    );
    assert!(copied["rows"][0]["source_id"].is_null());
    drain(&w).await;
    assert_eq!(
        sqlx::query_scalar::<_, Decimal>("SELECT price FROM products WHERE id=$1")
            .bind(product)
            .fetch_one(&w.state.db)
            .await
            .unwrap(),
        Decimal::from(5)
    );
    approve(&w, rest, 1, "write").await;
    drain(&w).await;
    assert_eq!(
        sqlx::query_scalar::<_, Decimal>("SELECT price FROM products WHERE id=$1")
            .bind(product)
            .fetch_one(&w.state.db)
            .await
            .unwrap(),
        Decimal::from(4)
    );
}

#[sqlx::test]
async fn cancelled_queued_work_and_lost_claim_cannot_apply(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Never write","price":"0"}}]),
    )
    .await;
    drain(&w).await;
    approve(&w, id, revision, "write").await;
    let job = jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE jobs SET locked_by=uuidv7() WHERE id=$1")
        .bind(job.id)
        .execute(&w.state.db)
        .await
        .unwrap();
    assert!(
        product_bulk_runs::worker::run(&ctx(&w), &job)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='Never write'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(count, 0);
    request(&w, Method::POST, &format!("/{id}/cancel"), None).await;
    assert_eq!(
        product_bulk_runs::worker::run(&ctx(&w), &job)
            .await
            .unwrap(),
        Step::Done
    );
}

#[sqlx::test]
async fn per_run_budget_reserves_unknown_cost_and_rejects_replay(db: PgPool) {
    let w = world(db).await;
    let run = request(&w, Method::POST, "", Some(json!({}))).await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    assert!(
        product_bulk_runs::spend::reserve_ai_cost(
            &w.state.db,
            w.workspace,
            run,
            None,
            "first",
            Decimal::new(6, 2)
        )
        .await
        .is_err()
    );
    request(
        &w,
        Method::POST,
        "/settings",
        Some(json!({"per_run_cap_usd":"0.10"})),
    )
    .await;
    let reservation = product_bulk_runs::spend::reserve_ai_cost(
        &w.state.db,
        w.workspace,
        run,
        None,
        "first",
        Decimal::new(6, 2),
    )
    .await
    .unwrap();
    assert!(
        product_bulk_runs::spend::reserve_ai_cost(
            &w.state.db,
            w.workspace,
            run,
            None,
            "second",
            Decimal::new(5, 2)
        )
        .await
        .is_err()
    );
    assert!(
        product_bulk_runs::spend::reserve_ai_cost(
            &w.state.db,
            w.workspace,
            run,
            None,
            "first",
            Decimal::new(1, 2)
        )
        .await
        .is_err()
    );
    product_bulk_runs::spend::settle_ai_cost(
        &w.state.db,
        w.workspace,
        reservation,
        Decimal::new(1, 2),
        &json!({"input_tokens":1}),
    )
    .await
    .unwrap();
    product_bulk_runs::spend::reserve_ai_cost(
        &w.state.db,
        w.workspace,
        run,
        None,
        "second",
        Decimal::new(5, 2),
    )
    .await
    .unwrap();
    let (other, _) = signup(&w.app, "other-bulk@example.test", "other").await;
    let (status, _) = send(
        &w.app,
        Method::GET,
        &format!("{}/{run}", w.base),
        None,
        Some(&other),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn selection_expands_raw_price_and_never_defaults_to_all(db: PgPool) {
    let w = world(db).await;
    let p = native(&w, "Selected", "3").await;
    let q = native(&w, "Other", "7").await;
    let run = request(&w, Method::POST, "", Some(json!({"mode":"update"}))).await;
    let id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let(status,_)=send(&w.app,Method::POST,&format!("{}/{id}/stage",w.base),Some(json!({"revision":1,"selection":{},"operation":"update","changes":{"price":{"kind":"percent","value":"5"}}})),Some(&w.token)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let staged=request(&w,Method::POST,&format!("/{id}/stage"),Some(json!({"revision":1,"selection":{"ids":[p]},"operation":"update","changes":{"price":{"kind":"percent","value":"5"}}}))).await;
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{id}/rows?q=3.15"), None).await;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    assert_eq!(rows["rows"][0]["values"]["price"], "3.15");
    approve(&w, id, staged["revision"].as_i64().unwrap(), "write").await;
    drain(&w).await;
    let row = sqlx::query("SELECT price FROM products WHERE id=$1")
        .bind(q)
        .fetch_one(&w.state.db)
        .await
        .unwrap();
    assert_eq!(row.get::<Decimal, _>("price"), Decimal::from(7));
}

#[sqlx::test]
async fn header_change_reparses_only_source_and_recounts_actual_rows(db: PgPool) {
    let w = world(db).await;
    let run = request(&w, Method::POST, "", Some(json!({}))).await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let source = Uuid::now_v7();
    sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state,cursor,columns) VALUES($1,$2,$3,$4,'source.csv','text/csv',1,repeat('0',64),'done','{\"offset\":99}','{\"\":[\"name\"]}')").bind(source).bind(w.workspace).bind(run).bind(format!("genesy/product-bulk/{}/{run}/{source}",w.workspace)).execute(&w.state.db).await.unwrap();
    let staged=request(&w,Method::POST,&format!("/{run}/stage"),Some(json!({"revision":1,"rows":[{"operation":"create","values":{"name":"Source row","price":"0"},"source_id":source},{"operation":"create","values":{"name":"Manual row","price":"0"}}]}))).await;
    drain(&w).await;
    let revision = staged["revision"].as_i64().unwrap();
    request(
        &w,
        Method::POST,
        &format!("/{run}/reviews"),
        Some(json!({"revision":revision,"scope":"write"})),
    )
    .await;
    sqlx::query("UPDATE product_bulk_rows SET ordinal=50001 WHERE run_id=$1 AND source_id IS NULL")
        .bind(run)
        .execute(&w.state.db)
        .await
        .unwrap();
    let mapped=request(&w,Method::PATCH,&format!("/{run}/mapping"),Some(json!({"revision":revision,"source_id":source,"header_row":2,"columns":{"name":"name"}}))).await;
    assert_eq!(mapped["counts"]["total"], 1);
    assert_eq!(mapped["cell_count"], 2);
    assert_eq!(mapped["sources"][0]["state"], "stored");
    assert_eq!(mapped["reviews"][0]["state"], "superseded");
    let cursor: Value = sqlx::query_scalar("SELECT cursor FROM product_bulk_sources WHERE id=$1")
        .bind(source)
        .fetch_one(&w.state.db)
        .await
        .unwrap();
    assert_eq!(cursor, json!({}));
    let next=request(&w,Method::POST,&format!("/{run}/stage"),Some(json!({"revision":mapped["revision"],"rows":[{"operation":"create","values":{"name":"Another row","price":"0"}}]}))).await;
    assert_eq!(next["counts"]["total"], 2);
}

#[sqlx::test]
async fn typed_selection_writes_value_and_unknown_changes_are_refused(db: PgPool) {
    let w = world(db).await;
    let product = native(&w, "Typed selection", "1").await;
    let (status, field) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/product-fields", w.workspace),
        Some(json!({"key":"bulk_number","label":"Bulk number","type":"number","status":"active"})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    let field = field["data"]["id"].as_str().unwrap().to_owned();
    let run = request(&w, Method::POST, "", Some(json!({"mode":"update"}))).await;
    let id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let(status,_)=send(&w.app,Method::POST,&format!("{}/{id}/stage",w.base),Some(json!({"revision":1,"selection":{"ids":[product]},"operation":"update","changes":{"prcie":{"kind":"set","value":"17"}}})),Some(&w.token)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let staged=request(&w,Method::POST,&format!("/{id}/stage"),Some(json!({"revision":1,"selection":{"ids":[product]},"operation":"update","changes":{format!("custom_fields.{field}"):{"kind":"set","value":"17"}}}))).await;
    drain(&w).await;
    approve(&w, id, staged["revision"].as_i64().unwrap(), "write").await;
    drain(&w).await;
    let value:Decimal=sqlx::query_scalar("SELECT value_number FROM product_field_values WHERE workspace_id=$1 AND product_id=$2 AND field_id=$3").bind(w.workspace).bind(product).bind(Uuid::parse_str(&field).unwrap()).fetch_one(&w.state.db).await.unwrap();
    assert_eq!(value, Decimal::from(17));
}

#[sqlx::test]
async fn template_uses_live_native_schema_and_includes_explicit_clear(db: PgPool) {
    let w = world(db).await;
    let template = request(&w, Method::GET, "/template", None).await;
    let fields = template["fields"].as_array().unwrap();
    assert!(fields.len() > 5);
    for field in [
        "name",
        "price",
        "custom_fields",
        "_operation",
        "_key",
        "_product_id",
        "_clear",
    ] {
        assert!(fields.iter().any(|f| f["key"] == field), "missing {field}");
    }
    assert_eq!(template["limits"]["rows"], 50000);
    assert!(template["schema"]["properties"]["price"].is_object());
}

#[sqlx::test]
async fn cached_formulas_import_missing_cache_errors_and_corrections_keep_raw_evidence(db: PgPool) {
    let w = world(db).await;
    let run = request(&w, Method::POST, "", Some(json!({}))).await;
    let id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let source = Uuid::now_v7();
    sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state,columns) VALUES($1,$2,$3,$4,'formulas.xlsx','application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',1,repeat('0',64),'done','{\"\":[\"name\",\"price\"]}')").bind(source).bind(w.workspace).bind(id).bind(format!("genesy/product-bulk/{}/{id}/{source}",w.workspace)).execute(&w.state.db).await.unwrap();
    request(&w,Method::POST,&format!("/{id}/stage"),Some(json!({"revision":1,"rows":[{"operation":"create","source_id":source},{"operation":"create","source_id":source}]}))).await;
    for (ordinal, name, price) in [(1, "Cached", "3"), (2, "Missing", "")] {
        let raw = json!({"source_id":source,"sheet":null,"page":null,"line":ordinal+1,"columns":["name","price"],"cells":[name,price],"formula_columns":[1]});
        sqlx::query(
            "UPDATE product_bulk_rows SET raw=$3,cell_count=$4 WHERE run_id=$1 AND ordinal=$2",
        )
        .bind(id)
        .bind(ordinal as i64)
        .bind(raw)
        .bind(if price.is_empty() { 1i32 } else { 2i32 })
        .execute(&w.state.db)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE product_bulk_runs SET cell_count=3 WHERE id=$1")
        .bind(id)
        .execute(&w.state.db)
        .await
        .unwrap();
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{id}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "ready");
    assert_eq!(rows["rows"][1]["status"], "error");
    assert_eq!(rows["rows"][0]["raw"]["formula_columns"], json!([1]));
    let run = request(&w, Method::GET, &format!("/{id}"), None).await;
    let row = rows["rows"][0]["id"].as_str().unwrap();
    let changed = request(
        &w,
        Method::PATCH,
        &format!("/{id}/rows/{row}"),
        Some(json!({"revision":run["revision"],"values":{"name":"Cached","price":"4"}})),
    )
    .await;
    drain(&w).await;
    let raw = request(
        &w,
        Method::GET,
        &format!("/{id}/rows/{row}/raw?after=0&page_size=8"),
        None,
    )
    .await;
    assert_eq!(raw["cells"][1]["value"], "3");
    assert_eq!(raw["cells"][1]["formula"], true);
    let rows = request(&w, Method::GET, &format!("/{id}/rows"), None).await;
    assert_eq!(rows["rows"][0]["values"]["price"], "4");
    assert_eq!(rows["rows"][0]["manual_override"], true);
    approve(&w, id, changed["revision"].as_i64().unwrap(), "write").await;
    drain(&w).await;
    let price: Decimal =
        sqlx::query_scalar("SELECT price FROM products WHERE workspace_id=$1 AND name='Cached'")
            .bind(w.workspace)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
    assert_eq!(price, Decimal::from(4));
}

#[sqlx::test]
async fn exhausted_configuration_pause_can_resume_preparation_but_never_approved_execution(
    db: PgPool,
) {
    let w = world(db).await;
    let run = request(&w, Method::POST, "", Some(json!({}))).await;
    let id = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    request(
        &w,
        Method::POST,
        &format!("/{id}/stage"),
        Some(json!({"revision":1,"text":"Create Rice, price 3"})),
    )
    .await;
    drain(&w).await;
    let paused = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["permissions"]["resume"], true);
    let resumed = request(
        &w,
        Method::POST,
        &format!("/{id}/resume"),
        Some(json!({"revision":paused["revision"]})),
    )
    .await;
    assert_eq!(
        resumed["revision"].as_i64().unwrap(),
        paused["revision"].as_i64().unwrap() + 1
    );
    assert_eq!(resumed["state"], "preparing");
    assert_eq!(resumed["permissions"]["resume"], false);
    request(&w, Method::POST, &format!("/{id}/cancel"), None).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Approval cannot resume","price":"0"}}]),
    )
    .await;
    drain(&w).await;
    approve(&w, id, revision, "write").await;
    sqlx::query("UPDATE product_bulk_runs SET state='failed' WHERE id=$1")
        .bind(id)
        .execute(&w.state.db)
        .await
        .unwrap();
    let failed = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(failed["permissions"]["resume"], false);
    let (status, _) = send(
        &w.app,
        Method::POST,
        &format!("{}/{id}/resume", w.base),
        Some(json!({"revision":revision})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test]
async fn terminal_reaped_jobs_surface_pause_and_fresh_recovery(db: PgPool) {
    let w = world(db).await;
    let (id, _) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Recovered worker","price":"1"}}]),
    )
    .await;
    let job = jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE jobs SET attempts=max_attempts,locked_until=now()-interval '1 second' WHERE id=$1",
    )
    .bind(job.id)
    .execute(&w.state.db)
    .await
    .unwrap();
    jobs::repo::reap(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    let paused = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["issues"][0]["code"], "worker_interrupted");
    let resumed = request(
        &w,
        Method::POST,
        &format!("/{id}/resume"),
        Some(json!({"revision":paused["revision"]})),
    )
    .await;
    drain(&w).await;
    let ready = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["counts"]["ready"], 1);
    approve(&w, id, resumed["revision"].as_i64().unwrap(), "write").await;
    let job = jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE jobs SET attempts=max_attempts,locked_until=now()-interval '1 second' WHERE id=$1",
    )
    .bind(job.id)
    .execute(&w.state.db)
    .await
    .unwrap();
    jobs::repo::reap(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    let failed = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(failed["state"], "failed");
    let (status, _) = send(
        &w.app,
        Method::POST,
        &format!("{}/{id}/resume", w.base),
        Some(json!({"revision":failed["revision"]})),
        Some(&w.token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let remainder = request(&w, Method::POST, &format!("/{id}/remainder"), None).await;
    assert_eq!(remainder["counts"]["total"], 1);
    let written: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='Recovered worker'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(written, 0);
}

#[sqlx::test]
async fn approval_expiry_stales_without_mutation_and_requires_new_review(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Expired approval","price":"1"}}]),
    )
    .await;
    drain(&w).await;
    let review = approve(&w, id, revision, "write").await;
    sqlx::query("UPDATE product_bulk_reviews SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(Uuid::parse_str(review["id"].as_str().unwrap()).unwrap()).execute(&w.state.db).await.unwrap();
    sqlx::query("UPDATE product_bulk_runs SET approved_until=clock_timestamp()-interval '1 second' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    drain(&w).await;
    let rows = request(&w, Method::GET, &format!("/{id}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "stale");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='Expired approval'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(count, 0);
    let rest = request(&w, Method::POST, &format!("/{id}/remainder"), None).await;
    assert_eq!(rest["counts"]["total"], 1);
    assert_eq!(rest["reviews"], json!([]));
}

#[sqlx::test]
async fn approving_member_permission_is_checked_again_during_execution(db: PgPool) {
    let w = world(db).await;
    let email = "bulk-approver@example.test";
    let token = common::member_with(
        &w.app,
        &w.state.db,
        &w.workspace.to_string(),
        email,
        &["product.list", "product.view", "product.create"],
    )
    .await;
    let member = World {
        app: w.app.clone(),
        state: w.state.clone(),
        token,
        workspace: w.workspace,
        base: w.base.clone(),
    };
    let (id, revision) = stage(
        &member,
        json!([{ "operation":"create","values":{"name":"Revoked approval","price":"1"}}]),
    )
    .await;
    drain(&member).await;
    approve(&member, id, revision, "write").await;
    sqlx::query("DELETE FROM team_permissions p USING teams t,team_members m,users u WHERE p.team_id=t.id AND m.team_id=t.id AND m.user_id=u.id AND t.workspace_id=$1 AND u.email=$2 AND p.permission='product.create'").bind(w.workspace).bind(email).execute(&w.state.db).await.unwrap();
    drain(&member).await;
    let rows = request(&w, Method::GET, &format!("/{id}/rows"), None).await;
    assert_eq!(rows["rows"][0]["status"], "error");
    assert_eq!(rows["rows"][0]["issues"][0]["code"], "permission");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='Revoked approval'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn same_workspace_noncreator_cannot_read_or_control_private_run(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Private staged row","price":"1"}}]),
    )
    .await;
    drain(&w).await;
    let review = request(
        &w,
        Method::POST,
        &format!("/{id}/reviews"),
        Some(json!({"revision":revision,"scope":"write"})),
    )
    .await;
    let reader = common::member_with(
        &w.app,
        &w.state.db,
        &w.workspace.to_string(),
        "bulk-peer@example.test",
        &["product.list", "product.view", "product.create"],
    )
    .await;
    for (method, suffix, body) in [
        (Method::GET, format!("/{id}"), None),
        (Method::GET, format!("/{id}/rows"), None),
        (Method::POST, format!("/{id}/cancel"), None),
        (
            Method::POST,
            format!("/{id}/approve"),
            Some(json!({"revision":revision,"review_id":review["id"]})),
        ),
    ] {
        let (status, _) = send(
            &w.app,
            method,
            &format!("{}{suffix}", w.base),
            body,
            Some(&reader),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{suffix}");
    }
    let owner = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(owner["state"], "ready");
    assert_eq!(owner["counts"]["ready"], 1);
}

#[sqlx::test]
async fn seven_day_draft_boundary_blocks_work_and_exposes_expiry(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Draft expires","price":"1"}}]),
    )
    .await;
    sqlx::query("UPDATE product_bulk_runs SET created_at=clock_timestamp()-interval '7 days'+interval '1 hour' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert_ne!(
        request(&w, Method::GET, &format!("/{id}"), None).await["state"],
        "expired"
    );
    sqlx::query("UPDATE product_bulk_runs SET created_at=clock_timestamp()-interval '7 days'-interval '1 second' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    let expired = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(expired["state"], "expired");
    assert!(!expired["finished_at"].is_null());
    assert_eq!(expired["permissions"]["edit"], false);
    let(status,_)=send(&w.app,Method::POST,&format!("{}/{id}/stage",w.base),Some(json!({"revision":revision,"rows":[{"operation":"create","values":{"name":"Late row","price":"1"}}]})),Some(&w.token)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    drain(&w).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name='Draft expires'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn retention_uses_terminal_age_and_preserves_canonical_outcome_until_ninety_days(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Retention product","price":"1"}}]),
    )
    .await;
    let source = Uuid::now_v7();
    let key = format!("genesy/product-bulk/{}/{id}/{source}", w.workspace);
    sqlx::query("INSERT INTO product_bulk_sources(id,workspace_id,run_id,object_key,name,media_type,size_bytes,sha256,state,columns) VALUES($1,$2,$3,$4,'retention.csv','text/csv',1,repeat('0',64),'done','{\"\":[\"name\",\"price\"]}')").bind(source).bind(w.workspace).bind(id).bind(&key).execute(&w.state.db).await.unwrap();
    w.state
        .files
        .as_ref()
        .unwrap()
        .put(
            &key,
            b"original source".to_vec(),
            "text/csv",
            "retention.csv",
            false,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE product_bulk_rows SET source_id=$2,raw=$3 WHERE run_id=$1").bind(id).bind(source).bind(json!({"source_id":source,"sheet":null,"page":null,"line":2,"cells":["Retention product","1"],"columns":["name","price"],"formula_columns":[]})).execute(&w.state.db).await.unwrap();
    drain(&w).await;
    approve(&w, id, revision, "write").await;
    drain(&w).await;
    sqlx::query("UPDATE product_bulk_runs SET created_at=clock_timestamp()-interval '200 days',finished_at=clock_timestamp()-interval '7 days'+interval '1 hour' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert!(
        w.state
            .files
            .as_ref()
            .unwrap()
            .get(&key)
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query("UPDATE product_bulk_runs SET finished_at=clock_timestamp()-interval '7 days'-interval '1 second' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert!(
        w.state
            .files
            .as_ref()
            .unwrap()
            .get(&key)
            .await
            .unwrap()
            .is_none()
    );
    let row =
        sqlx::query("SELECT raw,values,material,receipt FROM product_bulk_rows WHERE run_id=$1")
            .bind(id)
            .fetch_one(&w.state.db)
            .await
            .unwrap();
    assert!(row.get::<Option<Value>, _>("raw").is_none());
    assert_eq!(row.get::<Value, _>("values")["name"], "Retention product");
    assert!(!row.get::<Option<Value>, _>("material").unwrap().is_null());
    assert_eq!(row.get::<Value, _>("receipt")["status"], "completed");
    sqlx::query("UPDATE product_bulk_runs SET finished_at=clock_timestamp()-interval '90 days'+interval '1 hour' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM product_bulk_runs WHERE id=$1)")
            .bind(id)
            .fetch_one(&w.state.db)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE product_bulk_runs SET finished_at=clock_timestamp()-interval '90 days'-interval '1 second' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM product_bulk_runs WHERE id=$1)"
        )
        .bind(id)
        .fetch_one(&w.state.db)
        .await
        .unwrap()
    );
}

#[sqlx::test]
async fn uncertain_cost_retains_only_minimal_ledger_after_outcome_expiry(db: PgPool) {
    let w = world(db).await;
    let (id, revision) = stage(
        &w,
        json!([{ "operation":"create","values":{"name":"Erase outcome","price":"1"}}]),
    )
    .await;
    request(
        &w,
        Method::POST,
        "/settings",
        Some(json!({"per_run_cap_usd":"1"})),
    )
    .await;
    let reservation = product_bulk_runs::spend::reserve_ai_cost(
        &w.state.db,
        w.workspace,
        id,
        None,
        "uncertain-retention",
        Decimal::new(5, 1),
    )
    .await
    .unwrap();
    drain(&w).await;
    approve(&w, id, revision, "write").await;
    drain(&w).await;
    sqlx::query("UPDATE product_bulk_runs SET finished_at=clock_timestamp()-interval '91 days',instruction='raw instruction',mapping='{\"private\":\"mapping\"}',selection='{\"private\":true}' WHERE id=$1").bind(id).execute(&w.state.db).await.unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    let run = request(&w, Method::GET, &format!("/{id}"), None).await;
    assert_eq!(run["instruction"], "");
    assert_eq!(run["mapping"], json!({}));
    assert!(run["selection"].is_null());
    assert_eq!(run["counts"]["total"], 0);
    assert_eq!(run["reviews"], json!([]));
    assert_eq!(run["sources"], json!([]));
    let costs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_bulk_ai_costs WHERE run_id=$1 AND state='reserved'",
    )
    .bind(id)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(costs, 1);
    product_bulk_runs::spend::settle_ai_cost(
        &w.state.db,
        w.workspace,
        reservation,
        Decimal::new(1, 1),
        &json!({"input_tokens":1}),
    )
    .await
    .unwrap();
    product_bulk_runs::worker::prune(&w.state.db, w.state.files.as_ref())
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM product_bulk_runs WHERE id=$1)"
        )
        .bind(id)
        .fetch_one(&w.state.db)
        .await
        .unwrap()
    );
}

/// Run explicitly on the Linux release gate; never substitutes for correctness tests.
#[sqlx::test]
#[ignore = "50k native mutations and background/foreground latency benchmark"]
async fn bulk_50000_native_mixed_load(db: PgPool) {
    use std::time::Instant;
    let w = world(db).await;
    let (status, body) = send(
        &w.app,
        Method::POST,
        &format!("/v1/workspace/{}/warehouses", w.workspace),
        Some(json!({"name":"Bulk benchmark warehouse","default_warehouse":true,"is_store":true})),
        Some(&w.token),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let(status,body)=send(&w.app,Method::POST,&format!("/v1/workspace/{}/product-fields",w.workspace),Some(json!({"key":"benchmark_number","label":"Benchmark number","type":"number","status":"active"})),Some(&w.token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let field = body["data"]["id"].as_str().unwrap();
    let field = Uuid::parse_str(field).unwrap();
    let baseline = native(&w, "Benchmark template", "10").await;
    sqlx::query("INSERT INTO products SELECT (jsonb_populate_record(NULL::products,to_jsonb(p)||jsonb_build_object('id',uuidv7(),'product_number',p.product_number+n,'product_nr_formatted','BENCH-'||n,'name','Existing '||n))).* FROM products p CROSS JOIN generate_series(1,33333) AS g(n) WHERE p.id=$1").bind(baseline).execute(&w.state.db).await.unwrap();
    sqlx::query("INSERT INTO product_variants SELECT (jsonb_populate_record(NULL::product_variants,to_jsonb(v)||jsonb_build_object('id',uuidv7(),'product_id',p.id))).* FROM product_variants v CROSS JOIN products p WHERE v.product_id=$1 AND v.is_default AND p.workspace_id=$2 AND p.id<>$1").bind(baseline).bind(w.workspace).execute(&w.state.db).await.unwrap();
    sqlx::query("INSERT INTO product_variants SELECT (jsonb_populate_record(NULL::product_variants,to_jsonb(v)||jsonb_build_object('id',uuidv7(),'is_default',false,'inherits_price',true))).* FROM product_variants v WHERE v.workspace_id=$1 AND v.is_default").bind(w.workspace).execute(&w.state.db).await.unwrap();
    sqlx::query("INSERT INTO product_variants SELECT (jsonb_populate_record(NULL::product_variants,to_jsonb(v)||jsonb_build_object('id',uuidv7(),'is_default',false,'inherits_price',false))).* FROM product_variants v WHERE v.workspace_id=$1 AND v.is_default").bind(w.workspace).execute(&w.state.db).await.unwrap();
    sqlx::query("INSERT INTO stocks SELECT (jsonb_populate_record(NULL::stocks,to_jsonb(s)||jsonb_build_object('id',uuidv7(),'product_id',p.id))).* FROM stocks s CROSS JOIN products p WHERE s.product_id=$1 AND p.workspace_id=$2 AND p.id<>$1").bind(baseline).bind(w.workspace).execute(&w.state.db).await.unwrap();
    sqlx::query("INSERT INTO product_field_values(workspace_id,product_id,field_id,type,value_number) SELECT workspace_id,id,$2,'number',10 FROM products WHERE workspace_id=$1").bind(w.workspace).bind(field).execute(&w.state.db).await.unwrap();
    sqlx::query("UPDATE product_field_definitions SET used=true WHERE id=$1")
        .bind(field)
        .execute(&w.state.db)
        .await
        .unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM products WHERE workspace_id=$1 AND id<>$2 ORDER BY product_number",
    )
    .bind(w.workspace)
    .bind(baseline)
    .fetch_all(&w.state.db)
    .await
    .unwrap();
    assert_eq!(ids.len(), 33333);
    let update_ids: Vec<Uuid> = ids.iter().step_by(2).copied().collect();
    assert_eq!(update_ids.len(), 16667);
    sqlx::query("UPDATE stocks SET stock_lvl=5 WHERE workspace_id=$1 AND product_id=ANY($2)")
        .bind(w.workspace)
        .bind(&update_ids)
        .execute(&w.state.db)
        .await
        .unwrap();

    let run = request(
        &w,
        Method::POST,
        "",
        Some(json!({"name":"50k measured mixed native changes"})),
    )
    .await;
    let run = Uuid::parse_str(run["id"].as_str().unwrap()).unwrap();
    let mut revision = 1i64;
    let started = Instant::now();
    let mut existing = 0usize;
    for page in 0..100 {
        let mut rows = Vec::new();
        for offset in 0..500 {
            let i = page * 500 + offset;
            let op = i % 3;
            if op == 0 {
                rows.push(json!({"operation":"create","values":{"name":format!("Bulk created {i}"),"price":"11","custom_fields":{field.to_string():"11"}}}));
            } else {
                let id = ids[existing];
                existing += 1;
                rows.push(if op==1{json!({"operation":"update","target":{"product_id":id},"values":{"price":"11","custom_fields":{field.to_string():"11"}}})}else{json!({"operation":"delete","target":{"product_id":id}})});
            }
        }
        let staged = request(
            &w,
            Method::POST,
            &format!("/{run}/stage"),
            Some(json!({"revision":revision,"rows":rows})),
        )
        .await;
        revision = staged["revision"].as_i64().unwrap();
    }
    let stage_seconds = started.elapsed().as_secs_f64();
    let mut chunk_ms = Vec::new();
    let mut foreground_ms = Vec::new();
    let mut restarted = false;
    let mut chunks = 0;
    let started = Instant::now();
    loop {
        let Some(job) =
            jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
                .await
                .unwrap()
        else {
            break;
        };
        if !restarted && chunks == 2 {
            sqlx::query(
                "UPDATE jobs SET status='queued',locked_by=NULL,locked_until=NULL WHERE id=$1",
            )
            .bind(job.id)
            .execute(&w.state.db)
            .await
            .unwrap();
            assert!(
                product_bulk_runs::worker::run(&ctx(&w), &job)
                    .await
                    .is_err()
            );
            restarted = true;
            continue;
        }
        let began = Instant::now();
        let foreground = async {
            let began = Instant::now();
            let (status, body) = send(
                &w.app,
                Method::GET,
                &format!("/v1/workspace/{}/products?page_size=5", w.workspace),
                None,
                Some(&w.token),
            )
            .await;
            assert!(status.is_success(), "{body}");
            let elapsed = began.elapsed();
            assert!(elapsed < w.state.request_timeout);
            elapsed.as_secs_f64() * 1000.0
        };
        let context = ctx(&w);
        let (result, latency) =
            tokio::join!(product_bulk_runs::worker::run(&context, &job), foreground);
        let result = result.unwrap();
        chunk_ms.push(began.elapsed().as_secs_f64() * 1000.0);
        foreground_ms.push(latency);
        chunks += 1;
        match result {
            Step::Done => {
                jobs::repo::finish(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
            Step::More => {
                jobs::repo::requeue(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
        }
        assert!(chunks < 20_000, "bounded job failed to make progress");
    }
    let prepare_seconds = started.elapsed().as_secs_f64();
    let prepared = request(&w, Method::GET, &format!("/{run}"), None).await;
    assert_eq!(prepared["counts"]["ready"], 50000);
    assert_eq!(prepared["counts"]["errors"], 0);
    approve(&w, run, revision, "write").await;
    approve(&w, run, revision, "delete").await;
    let started = Instant::now();
    let mut applied_chunks = 0usize;
    loop {
        let Some(job) =
            jobs::repo::claim(&w.state.db, &[jobs::kinds::PRODUCT_BULK], 2, jobs::LEASE)
                .await
                .unwrap()
        else {
            break;
        };
        let began = Instant::now();
        let foreground = async {
            let began = Instant::now();
            let (status, body) = send(
                &w.app,
                Method::GET,
                &format!("/v1/workspace/{}/products?page_size=5", w.workspace),
                None,
                Some(&w.token),
            )
            .await;
            assert!(status.is_success(), "{body}");
            let elapsed = began.elapsed();
            assert!(elapsed < w.state.request_timeout);
            elapsed.as_secs_f64() * 1000.0
        };
        let context = ctx(&w);
        let (result, latency) =
            tokio::join!(product_bulk_runs::worker::run(&context, &job), foreground);
        let result = result.unwrap();
        chunk_ms.push(began.elapsed().as_secs_f64() * 1000.0);
        foreground_ms.push(latency);
        applied_chunks += 1;
        match result {
            Step::Done => {
                jobs::repo::finish(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
            Step::More => {
                jobs::repo::requeue(&w.state.db, job.id, job.token)
                    .await
                    .unwrap();
            }
        }
        assert!(
            applied_chunks < 20_000,
            "bounded apply failed to make progress"
        );
    }
    let apply_seconds = started.elapsed().as_secs_f64();
    let final_run = request(&w, Method::GET, &format!("/{run}"), None).await;
    assert_eq!(final_run["counts"]["applied"], 50000);
    assert_eq!(final_run["counts"]["errors"], 0);
    assert_eq!(final_run["counts"]["stale"], 0);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_bulk_rows WHERE run_id=$1 AND receipt->>'status'='completed'",
    )
    .bind(run)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(receipts, 50000);
    let created: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND name LIKE 'Bulk created %'",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(created, 16667);
    let deleted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE workspace_id=$1 AND deleted_at IS NOT NULL",
    )
    .bind(w.workspace)
    .fetch_one(&w.state.db)
    .await
    .unwrap();
    assert_eq!(deleted, 16666);
    let independent:i64=sqlx::query_scalar("SELECT count(*) FROM product_variants v WHERE v.workspace_id=$1 AND v.product_id=ANY($2) AND NOT v.inherits_price AND v.price=10 AND v.deleted_at IS NULL").bind(w.workspace).bind(&update_ids).fetch_one(&w.state.db).await.unwrap();
    assert_eq!(
        independent, 16667,
        "independent variant prices remain unchanged"
    );
    let following:i64=sqlx::query_scalar("SELECT count(*) FROM product_variants v WHERE v.workspace_id=$1 AND v.product_id=ANY($2) AND v.inherits_price AND v.price=11 AND v.deleted_at IS NULL").bind(w.workspace).bind(&update_ids).fetch_one(&w.state.db).await.unwrap();
    assert_eq!(
        following, 33334,
        "both inheriting variants follow native price updates"
    );
    let stocks:i64=sqlx::query_scalar("SELECT count(*) FROM stocks WHERE workspace_id=$1 AND product_id=ANY($2) AND stock_lvl=5 AND deleted_at IS NULL").bind(w.workspace).bind(&update_ids).fetch_one(&w.state.db).await.unwrap();
    assert_eq!(stocks, 16667, "price updates preserve positive stock");

    chunk_ms.sort_by(f64::total_cmp);
    foreground_ms.sort_by(f64::total_cmp);
    let percentile = |v: &[f64], p: usize| v[(v.len() - 1) * p / 100];
    eprintln!(
        "bulk_50k stage_s={stage_seconds:.3} prepare_s={prepare_seconds:.3} apply_s={apply_seconds:.3} chunks={} chunk_ms_p50={:.3} chunk_ms_p95={:.3} chunk_ms_max={:.3} foreground_ms_p95={:.3} pool_size={} pool_idle={} restarted={restarted}",
        chunk_ms.len(),
        percentile(&chunk_ms, 50),
        percentile(&chunk_ms, 95),
        chunk_ms.last().unwrap(),
        percentile(&foreground_ms, 95),
        w.state.db.size(),
        w.state.db.num_idle()
    );
}
