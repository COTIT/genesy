//! Catalog-wide native authentication and unapproved-action refusal checks.

use std::collections::{BTreeSet, HashMap};

use crate::common;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;

const IDS: &str = include_str!("../fixtures/assistant_eligible_operations.json");
const EFFECT_IDS: &str = include_str!("../fixtures/assistant_effect_operations.json");
const ID: &str = "01950000-0000-7000-8000-000000000001";

// Mirror private native protocol; exposing runtime internals would widen production API.
fn request_hash(method: &str, path: &str, query: Option<&str>, body: Option<&Value>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in [method, path, query.unwrap_or("")] {
        hash.update(part.as_bytes());
        hash.update(b"\n");
    }
    if let Some(body) = body {
        hash.update(body.to_string().as_bytes());
    }
    hash.finalize().into()
}

fn request_hash_raw(
    method: &str,
    path: &str,
    query: Option<&str>,
    content_type: &str,
    digest: &str,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in [
        method,
        path,
        query.unwrap_or(""),
        "raw-multipart-sha256",
        content_type,
        digest,
    ] {
        hash.update(part.as_bytes());
        hash.update(b"\n");
    }
    hash.finalize().into()
}

fn resolved<'a>(spec: &'a Value, schema: &'a Value) -> &'a Value {
    let mut current = schema;
    let mut seen = BTreeSet::new();
    while let Some(reference) = current["$ref"].as_str() {
        assert!(reference.starts_with("#/components/schemas/"));
        assert!(seen.insert(reference), "recursive parameter schema");
        current = spec.pointer(&reference[1..]).unwrap();
    }
    current
}

fn sample(spec: &Value, schema: &Value, name: &str) -> Value {
    let current = resolved(spec, schema);
    let mut root = schema.clone();
    root["components"] = spec["components"].clone();
    let validator = jsonschema::validator_for(&root).unwrap();
    let minimum = current["minimum"].as_i64().unwrap_or(1);
    let maximum = current["maximum"].as_i64().unwrap_or(i64::MAX);
    let length = current["minLength"].as_u64().unwrap_or(1).max(1);
    assert!(length <= 2048, "unreviewed parameter length: {name}");
    let mut candidates = vec![
        current["const"].clone(),
        current["default"].clone(),
        current["enum"][0].clone(),
        current["example"].clone(),
        json!(ID),
        json!("invoice"),
        json!("currency"),
        json!("test"),
        json!("2026-11-01"),
        json!("2026-11-01T00:00:00Z"),
        json!("test@example.test"),
        json!("1"),
        json!("x".repeat(length as usize)),
        json!(true),
        json!(minimum.min(maximum)),
        json!(1),
    ];
    let formatted = match current["format"].as_str() {
        Some("uuid") => Some(json!(ID)),
        Some("date") => Some(json!(if name == "to" {
            "2026-11-02"
        } else {
            "2026-11-01"
        })),
        Some("date-time") => Some(json!(if name == "to" {
            "2026-11-02T00:00:00Z"
        } else {
            "2026-11-01T00:00:00Z"
        })),
        Some("email") => Some(json!("test@example.test")),
        _ => None,
    };
    if let Some(value) = formatted {
        candidates.insert(0, value);
    }
    if name == "workspace" {
        candidates.insert(0, json!(ID));
    }
    candidates
        .into_iter()
        .find(|value| !value.is_null() && validator.is_valid(value))
        .unwrap_or_else(|| panic!("No valid reviewed parameter sample for {name}"))
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

struct CapabilityRequest {
    id: String,
    method: Method,
    uri: String,
    json_body: bool,
    multipart: bool,
}

fn requests(spec: &Value, selected: &[&str], workspace: &str) -> Vec<CapabilityRequest> {
    assert_eq!(
        selected.iter().copied().collect::<BTreeSet<_>>().len(),
        selected.len()
    );
    let mut operations = HashMap::new();
    for (path, methods) in spec["paths"].as_object().unwrap() {
        for (method, operation) in methods.as_object().unwrap() {
            if let Some(id) = operation["operationId"].as_str() {
                assert!(
                    operations
                        .insert(id, (path, method, operation, methods))
                        .is_none()
                );
            }
        }
    }
    let mut requests = Vec::new();
    for id in selected {
        let (path, method, operation, path_item) = operations
            .get(*id)
            .unwrap_or_else(|| panic!("Missing registered operation {id}"));
        assert_eq!(
            operation["security"],
            json!([{"bearer":[]}]),
            "non-bearer capability cannot be silently skipped: {id}"
        );
        assert!(
            path.as_str() == "/workspace/{workspace}"
                || path.starts_with("/workspace/{workspace}/"),
            "unreviewed capability boundary: {id}"
        );
        let parameters: Vec<&Value> = path_item["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(operation["parameters"].as_array().into_iter().flatten())
            .collect();
        let mut actual_path = format!("/v1{path}");
        let mut query = Vec::new();
        for parameter in parameters {
            let name = parameter["name"].as_str().unwrap();
            if parameter["in"] == "path" {
                let value = if name == "workspace" {
                    json!(workspace)
                } else {
                    sample(spec, &parameter["schema"], name)
                };
                actual_path = actual_path.replace(&format!("{{{name}}}"), &text(&value));
            } else if parameter["in"] == "query" && parameter["required"] == true {
                query.push((name, text(&sample(spec, &parameter["schema"], name))));
            }
        }
        assert!(
            !actual_path.contains(['{', '}']),
            "unfilled registered path: {id}"
        );
        let mut url = reqwest::Url::parse("http://127.0.0.1").unwrap();
        url.set_path(&actual_path);
        for (name, value) in query {
            url.query_pairs_mut().append_pair(name, &value);
        }
        let uri = format!(
            "{}{}",
            url.path(),
            url.query().map(|q| format!("?{q}")).unwrap_or_default()
        );
        let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes()).unwrap();
        requests.push(CapabilityRequest {
            id: (*id).to_owned(),
            method,
            uri,
            json_body: operation["requestBody"]["content"]
                .get("application/json")
                .is_some(),
            multipart: operation["requestBody"]["content"]
                .get("multipart/form-data")
                .is_some(),
        });
    }
    requests
}

#[sqlx::test]
async fn every_eligible_bearer_capability_requires_authentication(pool: PgPool) {
    let fixture: Value = serde_json::from_str(IDS).unwrap();
    let selected: Vec<&str> = fixture["eligibleOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(selected.len(), 430, "reviewed capability count changed");
    let spec = genesy_360_api::openapi::spec().unwrap();
    let app = common::app(pool);
    let mut exercised = BTreeSet::new();
    for CapabilityRequest {
        id, method, uri, ..
    } in requests(&spec, &selected, ID)
    {
        let body = matches!(method, Method::POST | Method::PUT | Method::PATCH).then(|| json!({}));
        let (status, body) = common::send(&app, method, &uri, body, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{id}: expected401; a404 never proves authentication"
        );
        assert!(
            body["message"].is_string(),
            "{id}: missing authentication response"
        );
        assert!(exercised.insert(id));
    }
    assert_eq!(exercised.len(), 430);
}

#[sqlx::test]
async fn every_effectful_capability_refuses_its_exact_unapproved_action(pool: PgPool) {
    let fixture: Value = serde_json::from_str(EFFECT_IDS).unwrap();
    let selected: Vec<&str> = fixture["effectOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(selected.len(), 233, "reviewed effect count changed");
    let spec = genesy_360_api::openapi::spec().unwrap();
    let app = common::app(pool.clone());
    let (owner, workspace) =
        common::signup(&app, "approval-matrix@example.test", "approval-matrix").await;
    let (status, session) = common::send(
        &app,
        Method::POST,
        &format!("/v1/workspace/{workspace}/assistant/sessions"),
        Some(json!({"language":"en"})),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{session}");
    let session: uuid::Uuid = session["data"]["id"].as_str().unwrap().parse().unwrap();
    let (user, login): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT user_id,login_id FROM assistant_sessions WHERE workspace_id=$1::uuid AND id=$2",
    )
    .bind(&workspace)
    .bind(session)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut exercised = BTreeSet::new();
    for (index, case) in requests(&spec, &selected, &workspace)
        .into_iter()
        .enumerate()
    {
        let url = reqwest::Url::parse(&format!("http://127.0.0.1{}", case.uri)).unwrap();
        let body = (case.json_body && !case.multipart).then(|| json!({}));
        let content_type = "multipart/form-data; boundary=assistant-refusal-matrix";
        let raw = b"--assistant-refusal-matrix\r\nContent-Disposition: form-data; name=\"test\"\r\n\r\nsynthetic\r\n--assistant-refusal-matrix--\r\n";
        let raw_digest = hex::encode(Sha256::digest(raw));
        let digest = if case.multipart {
            request_hash_raw(
                case.method.as_str(),
                url.path(),
                url.query(),
                content_type,
                &raw_digest,
            )
        } else {
            request_hash(case.method.as_str(), url.path(), url.query(), body.as_ref())
        };
        let mut proposal = json!({"operation_id":case.id,"method":case.method.as_str(),"path":url.path(),"query":url.query(),"body":body});
        if case.multipart {
            proposal["body_sha256"] = json!(raw_digest);
            proposal["content_type"] = json!(content_type);
        }
        // Seed pending request; domain validation and review capture remain separate checks.
        let action: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO assistant_actions(workspace_id,session_id,user_id,login_id,operation_id,step_index,request_hash,proposal,state) VALUES($1::uuid,$2,$3,$4,$5,$6,$7,$8,'proposed') RETURNING id")
            .bind(&workspace).bind(session).bind(user).bind(login).bind(&case.id)
            .bind((index+1) as i64).bind(digest.as_slice())
            .bind(proposal)
            .fetch_one(&pool).await.unwrap();
        let mut request = Request::builder()
            .method(case.method.clone())
            .uri(&case.uri)
            .header("Authorization", format!("Bearer {owner}"))
            .header("Assistant-Action-Id", action.to_string())
            .header("Assistant-Session-Id", session.to_string());
        let bytes = if case.multipart {
            request = request.header("Content-Type", content_type);
            raw.to_vec()
        } else if let Some(body) = body {
            request = request.header("Content-Type", "application/json");
            body.to_string().into_bytes()
        } else {
            Vec::new()
        };
        let mut request = request.body(Body::from(bytes)).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            common::PEER.parse::<std::net::SocketAddr>().unwrap(),
        ));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let reply: Value = serde_json::from_slice(&bytes).unwrap();
        common::contract::check(&case.method, &case.uri, status, &reply);
        let (expected_status, expected_reply) = if case.id == "app_keys_create" {
            // Key creation must stay in secure Office UI even when an assistant claim is supplied.
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "message":"Use the secure Office form for this action.",
                    "errors":{"operation_id":["Use the secure Office form for this action."]}
                }),
            )
        } else {
            (
                StatusCode::CONFLICT,
                json!({"message":"Action approval is missing, expired, changed, requires a new review or already consumed."}),
            )
        };
        assert_eq!(status, expected_status, "{}: wrong refusal class", case.id);
        assert_eq!(reply, expected_reply, "{}", case.id);
        let saved: (String,bool,bool,bool,bool) = sqlx::query_as(
            "SELECT state,approved_at IS NULL,dispatched_at IS NULL,http_status IS NULL,finished_at IS NULL FROM assistant_actions WHERE workspace_id=$1::uuid AND session_id=$2 AND id=$3")
            .bind(&workspace).bind(session).bind(action).fetch_one(&pool).await.unwrap();
        assert_eq!(
            saved,
            ("proposed".into(), true, true, true, true),
            "{}: unapproved claim changed",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 233);
}

#[sqlx::test]
async fn every_permission_protected_capability_refuses_an_active_member_without_grants(
    pool: PgPool,
) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/assistant_restricted_operations.json"
    ))
    .unwrap();
    let selected: Vec<&str> = fixture["restrictedOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(
        selected.len(),
        284,
        "reviewed permission-protected count changed"
    );
    let f = common::farm::Farm::new(pool.clone()).await;
    let email = "restricted-matrix@example.test";
    let token = common::member_with(&f.app, &pool, &f.ws, email, &[]).await;
    let workspace: uuid::Uuid = f.ws.parse().unwrap();
    let member: uuid::Uuid = f.user_id(email).await.parse().unwrap();
    let (active, owner, grants): (bool, bool, Vec<String>) = sqlx::query_as(
        "SELECT m.active,w.created_by_user_id=m.user_id,ARRAY(SELECT DISTINCT p.permission FROM team_members tm JOIN teams t ON t.id=tm.team_id AND t.workspace_id=w.id AND t.deleted_at IS NULL JOIN team_permissions p ON p.team_id=t.id WHERE tm.user_id=m.user_id) FROM workspaces w JOIN workspace_members m ON m.workspace_id=w.id AND m.user_id=$2 WHERE w.id=$1 AND w.deleted_at IS NULL")
        .bind(workspace).bind(member).fetch_one(&pool).await.unwrap();
    assert!(active && !owner && grants.is_empty());
    let roles: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM farm_memberships WHERE workspace_id=$1 AND user_id=$2 AND revoked_at IS NULL"
    ).bind(workspace).bind(member).fetch_one(&pool).await.unwrap();
    assert_eq!(roles, 0, "restricted actor must have no farm roles");
    let owner: uuid::Uuid = f.user_id("owner@ruaha.test").await.parse().unwrap();
    let membership: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM farm_memberships WHERE workspace_id=$1 AND user_id=$2 AND role='admin' AND revoked_at IS NULL"
    ).bind(workspace).bind(owner).fetch_one(&pool).await.unwrap();
    let spec = genesy_360_api::openapi::spec().unwrap();
    let mut exercised = BTreeSet::new();
    for mut case in requests(&spec, &selected, &f.ws) {
        let body = if case.id == "farm_memberships_create" {
            // Native grant validates before its role policy; use actual user and valid fields.
            Some(json!({"user_id":owner,"role":"ops"}))
        } else {
            matches!(case.method, Method::POST | Method::PUT | Method::PATCH).then(|| json!({}))
        };
        if case.id == "farm_memberships_revoke" {
            // Native revoke looks up before its role policy; a missing row proves nothing.
            case.uri = format!("/v1/workspace/{workspace}/farm/memberships/{membership}");
        }
        let (status, reply) =
            common::send(&f.app, case.method, &case.uri, body, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{}: no-grant member must be403;404/422 never prove permission refusal",
            case.id
        );
        assert!(
            reply["message"].is_string(),
            "{}: missing denial message",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 284);
}

#[sqlx::test]
async fn gateway_reads_enforce_the_payment_owner_permission_without_list_view_substitution(
    pool: PgPool,
) {
    use uuid::Uuid;

    let f = common::invoicing::setup_with(pool, false).await;
    let invoice = common::invoicing::draft(
        &f,
        json!([{"title":"Work","quantity":1,"price":"10","vat":0}]),
        false,
    )
    .await;
    let order = f
        .api
        .with_base("/orders")
        .create(
            json!({"lines":[{"product_id":f.product,"quantity":1}]}),
            "order_id",
        )
        .await;
    let ws: Uuid = f.api.ws.parse().unwrap();
    let invoice_payment = Uuid::now_v7();
    let order_payment = Uuid::now_v7();
    sqlx::query("INSERT INTO payment_accounts (workspace_id,environment,merchant_id,principal_hash) VALUES ($1,'sandbox','permission-test',decode(repeat('00',32),'hex'))")
        .bind(ws).execute(&f.api.pool).await.unwrap();
    for (id, invoice_id, order_id) in [
        (
            invoice_payment,
            Some(invoice.parse::<Uuid>().unwrap()),
            None,
        ),
        (order_payment, None, Some(order.parse::<Uuid>().unwrap())),
    ] {
        sqlx::query("INSERT INTO order_payments (id,workspace_id,invoice_id,order_id,kind,provider,environment,merchant_id,account_revision,request_id,status,amount,currency,decimals,customer_id,country,payer_name) VALUES ($1,$2,$3,$4,'payment','globadyme','sandbox','permission-test',1,$1,'open',1000,'EUR',2,$1::text,'NL','Client')")
            .bind(id).bind(ws).bind(invoice_id).bind(order_id).execute(&f.api.pool).await.unwrap();
    }
    let payments = f.api.with_base("/payments");
    for (email, grants, visible, allowed, denied, invoice_view) in [
        (
            "invoice-list-order-view@example.test",
            vec!["invoice.list", "order.view"],
            invoice_payment,
            order_payment,
            invoice_payment,
            false,
        ),
        (
            "order-list-invoice-view@example.test",
            vec!["order.list", "invoice.view"],
            order_payment,
            invoice_payment,
            order_payment,
            true,
        ),
    ] {
        let token = common::member_with(&f.api.app, &f.api.pool, &f.api.ws, email, &grants).await;
        let (status, reply) = payments.call(Method::GET, "", None, &token).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let rows = reply["data"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "list must filter by the owner's list permission"
        );
        assert_eq!(rows[0]["payment_id"], visible.to_string());
        let (status, reply) = payments
            .call(Method::GET, &format!("/{allowed}"), None, &token)
            .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["data"]["payment_id"], allowed.to_string());
        for (method, path) in [
            (Method::GET, format!("/{denied}")),
            (Method::POST, format!("/{denied}/refresh")),
        ] {
            let (status, reply) = payments.call(method, &path, None, &token).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "other-owner view permission cannot read or reconcile this payment: {reply}"
            );
        }
        let (status, reply) = f
            .api
            .call(Method::GET, &format!("/{invoice}/gateway"), None, &token)
            .await;
        assert_eq!(
            status,
            if invoice_view {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "{reply}"
        );
    }
    let changed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM order_payments WHERE workspace_id=$1 AND (claimed_at IS NOT NULL OR status <> 'open'))")
        .bind(ws).fetch_one(&f.api.pool).await.unwrap();
    assert!(
        !changed,
        "permission checks must not dispatch or reconcile payments"
    );
}

#[sqlx::test]
async fn reviewed_collection_capabilities_refuse_a_real_outsider_on_a_live_workspace(pool: PgPool) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/assistant_collection_tenant_operations.json"
    ))
    .unwrap();
    let selected: Vec<&str> = fixture["collectionOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(
        selected.len(),
        92,
        "reviewed collection boundary count changed"
    );
    let f = common::farm::Farm::new(pool.clone()).await;
    let email = "collection-outsider@example.test";
    let (outsider, _) = common::signup(&f.app, email, "collection-outsider").await;
    let user: uuid::Uuid = f.user_id(email).await.parse().unwrap();
    let workspace: uuid::Uuid = f.ws.parse().unwrap();
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspaces w JOIN workspace_members m ON m.workspace_id=w.id AND m.user_id=$2 WHERE w.id=$1 AND w.deleted_at IS NULL)")
        .bind(workspace).bind(user).fetch_one(&pool).await.unwrap();
    assert!(
        !member,
        "outsider must not hold target-workspace membership"
    );
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=$1 AND deleted_at IS NULL)",
    )
    .bind(workspace)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        exists,
        "tenant boundary must target an actual live workspace"
    );
    let roles: i64 = sqlx::query_scalar("SELECT count(*) FROM farm_memberships WHERE workspace_id=$1 AND user_id=$2 AND revoked_at IS NULL")
        .bind(workspace).bind(user).fetch_one(&pool).await.unwrap();
    assert_eq!(
        roles, 0,
        "outsider must not hold a target-workspace farm role"
    );
    let spec = genesy_360_api::openapi::spec().unwrap();
    let mut exercised = BTreeSet::new();
    for case in requests(&spec, &selected, &f.ws) {
        let (template, _) =
            common::contract::find_route(&spec["paths"], &case.method, &case.uri).unwrap();
        assert_eq!(
            template.matches('{').count(),
            1,
            "{}: unresolved nested resource",
            case.id
        );
        let url = reqwest::Url::parse(&format!("http://127.0.0.1{}", case.uri)).unwrap();
        for (name, _) in url.query_pairs() {
            assert!(
                matches!(name.as_ref(), "from" | "to" | "compare_from" | "compare_to"),
                "{}: unresolved resource query {name}",
                case.id
            );
        }
        let body =
            matches!(case.method, Method::POST | Method::PUT | Method::PATCH).then(|| json!({}));
        let (status, reply) =
            common::send(&f.app, case.method, &case.uri, body, Some(&outsider)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{}: outsider must not reach an actual workspace collection",
            case.id
        );
        assert!(
            reply["message"].is_string(),
            "{}: missing tenant denial message",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 92);
}

#[sqlx::test]
async fn reviewed_object_body_capabilities_reject_boolean_json_before_domain_work(pool: PgPool) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/assistant_body_validation_operations.json"
    ))
    .unwrap();
    let selected: Vec<&str> = fixture["objectBodyOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(selected.len(), 10, "reviewed object-body count changed");
    assert_eq!(fixture["probe"], false);
    let f = common::farm::Farm::new(pool.clone()).await;
    let spec = genesy_360_api::openapi::spec().unwrap();
    let snapshot = "SELECT (SELECT count(*) FROM farm_cash_ledger WHERE workspace_id=$1::uuid),(SELECT count(*) FROM cash_vaults WHERE workspace_id=$1::uuid),(SELECT count(*) FROM email_send_logs WHERE workspace_id=$1::uuid)";
    let before: (i64, i64, i64) = sqlx::query_as(snapshot)
        .bind(&f.ws)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut exercised = BTreeSet::new();
    for case in requests(&spec, &selected, &f.ws) {
        assert!(
            case.json_body && !case.multipart,
            "{}: expected JSON object DTO",
            case.id
        );
        let (_, operation) =
            common::contract::find_route(&spec["paths"], &case.method, &case.uri).unwrap();
        let schema = resolved(
            &spec,
            &operation["requestBody"]["content"]["application/json"]["schema"],
        );
        assert_eq!(
            schema["type"], "object",
            "{}: unreviewed body schema",
            case.id
        );
        let (status, reply) = common::send(
            &f.app,
            case.method,
            &case.uri,
            Some(json!(false)),
            Some(&f.owner),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}: boolean body must stop before domain work",
            case.id
        );
        let errors = reply["errors"].as_object().unwrap();
        assert_eq!(
            errors.keys().map(String::as_str).collect::<Vec<_>>(),
            ["body"],
            "{}: expected DTO type error only",
            case.id
        );
        let messages = errors["body"].as_array().unwrap();
        assert!(
            !messages.is_empty() && messages.iter().all(Value::is_string),
            "{}: missing body error",
            case.id
        );
        assert!(
            reply["message"].is_string(),
            "{}: missing validation message",
            case.id
        );
        let after: (i64, i64, i64) = sqlx::query_as(snapshot)
            .bind(&f.ws)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "{}: rejected DTO created a cash/vault/mail record",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 10);
}

#[sqlx::test]
async fn reviewed_bodyless_uuid_capabilities_reject_malformed_resource_identifiers(pool: PgPool) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/assistant_uuid_validation_operations.json"
    ))
    .unwrap();
    let selected: Vec<&str> = fixture["uuidOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(selected.len(), 59, "reviewed UUID-only count changed");
    let malformed = fixture["malformed"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(malformed).is_err());
    let f = common::farm::Farm::new(pool).await;
    assert_ne!(f.ws, ID, "resource sample must never replace workspace");
    let spec = genesy_360_api::openapi::spec().unwrap();
    let mut exercised = BTreeSet::new();
    for case in requests(&spec, &selected, &f.ws) {
        assert!(
            !case.json_body && !case.multipart,
            "{}: body route not reviewed",
            case.id
        );
        let (template, operation) =
            common::contract::find_route(&spec["paths"], &case.method, &case.uri).unwrap();
        let parameters = spec["paths"][template]["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(operation["parameters"].as_array().into_iter().flatten());
        let mut resources = 0;
        for parameter in parameters {
            if parameter["in"] == "path" && parameter["name"] != "workspace" {
                assert_eq!(
                    resolved(&spec, &parameter["schema"])["format"],
                    "uuid",
                    "{}: string/domain path excluded",
                    case.id
                );
                resources += 1;
            }
            assert!(
                !(parameter["in"] == "query" && parameter["required"] == true),
                "{}: required query excluded",
                case.id
            );
        }
        assert!(resources > 0, "{}: missing resource UUID", case.id);
        let mut url = reqwest::Url::parse(&format!("http://127.0.0.1{}", case.uri)).unwrap();
        let path = url.path().replacen(ID, malformed, 1);
        assert_ne!(path, url.path(), "{}: resource UUID not replaced", case.id);
        assert!(
            path.contains(&format!("/workspace/{}/", f.ws)),
            "{}: workspace changed",
            case.id
        );
        assert_eq!(path.split('/').filter(|part| *part == malformed).count(), 1);
        url.set_path(&path);
        let uri = format!(
            "{}{}",
            url.path(),
            url.query().map(|q| format!("?{q}")).unwrap_or_default()
        );
        let (actual, _) = common::contract::find_route(&spec["paths"], &case.method, &uri).unwrap();
        assert_eq!(
            actual, template,
            "{}: malformed UUID changed route",
            case.id
        );
        let (status, reply) = common::send(&f.app, case.method, &uri, None, Some(&f.owner)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{}: malformed UUID must be404",
            case.id
        );
        assert_eq!(
            reply,
            json!({"message":"The requested resource could not be found."}),
            "{}",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 59);
}

#[sqlx::test]
async fn reviewed_no_business_input_capabilities_reject_malformed_bound_workspace(pool: PgPool) {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/assistant_workspace_validation_operations.json"
    ))
    .unwrap();
    let selected: Vec<&str> = fixture["workspaceOperationIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(selected.len(), 6, "reviewed bound-workspace count changed");
    let malformed = fixture["malformed"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(malformed).is_err());
    let f = common::farm::Farm::new(pool).await;
    let spec = genesy_360_api::openapi::spec().unwrap();
    let mut exercised = BTreeSet::new();
    for case in requests(&spec, &selected, &f.ws) {
        assert!(
            !case.json_body && !case.multipart,
            "{}: unexpected business body",
            case.id
        );
        let (template, operation) =
            common::contract::find_route(&spec["paths"], &case.method, &case.uri).unwrap();
        assert_eq!(
            template.matches('{').count(),
            1,
            "{}: business path input excluded",
            case.id
        );
        for parameter in spec["paths"][template]["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(operation["parameters"].as_array().into_iter().flatten())
        {
            assert!(
                parameter["in"] == "path" && parameter["name"] == "workspace",
                "{}: business parameter excluded",
                case.id
            );
        }
        let uri = case.uri.replacen(
            &format!("/workspace/{}", f.ws),
            &format!("/workspace/{malformed}"),
            1,
        );
        assert_ne!(uri, case.uri, "{}: workspace not replaced", case.id);
        let (status, reply) = common::send(&f.app, case.method, &uri, None, Some(&f.owner)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{}: malformed workspace must be404",
            case.id
        );
        assert_eq!(
            reply,
            json!({"message":"The requested resource could not be found."}),
            "{}",
            case.id
        );
        assert!(exercised.insert(case.id));
    }
    assert_eq!(exercised.len(), 6);
}
