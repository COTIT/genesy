//! Shared integration-test helpers: app with test defaults + one-call JSON request.
#![allow(dead_code)] // each test binary uses a different subset

pub mod contract;
pub mod fake_pdf;
pub mod fake_smtp;
pub mod farm;
pub mod invoicing;
pub mod native_fixture;

use axum::extract::ConnectInfo;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use chrono::Duration;
use genesy_360_api::{
    AppState, RateLimit, Slots, WorkspaceSlots, build_app,
    mail::{Mailer, Outbox},
};
use std::net::SocketAddr;

/// Peer address every test request comes from (axum::serve sets it in prod).
pub const PEER: &str = "127.0.0.1:40000";
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

pub const FRONTEND_URL: &str = "https://app.test";
/// Server key every test app seals credentials with.
pub const CREDENTIALS_KEY: &str =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
pub const API_PUBLIC_URL: &str = "https://api.test";

/// Test defaults; mail goes to `outbox`. Rate limit high enough that only
/// tests/api/rate_limit.rs (which sets its own) ever hits it.
pub fn state(pool: PgPool, mailer: Mailer) -> AppState {
    AppState {
        health_db: pool.clone(),
        db: pool,
        token_ttl: Duration::days(30),
        mailer,
        frontend_url: FRONTEND_URL.into(),
        api_public_url: API_PUBLIC_URL.into(),
        shop_public_url: Some("https://shop.test".into()),
        rate_limit: RateLimit {
            burst: 10_000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        invites_per_hour: 1_000,
        private_rate_limit: RateLimit {
            burst: 100_000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        preview_rate_limit: RateLimit {
            burst: 100_000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        private_ip_rate_limit: RateLimit {
            burst: 100_000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        slots: Slots::new(WorkspaceSlots {
            per_workspace: 1_000,
            per_user: 1_000,
            wait: std::time::Duration::from_secs(5),
        }),
        request_timeout: std::time::Duration::from_secs(60),
        body_deadline: std::time::Duration::from_secs(60),
        credentials: Some(std::sync::Arc::new(
            genesy_360_api::CredentialKey::from_hex(CREDENTIALS_KEY).unwrap(),
        )),
        assistant_bridge_secret: None,
        product_bulk_enabled: true,
        anthropic_api_key: None,
        files: Some(genesy_360_api::storage::Store::memory()),
        pdf: None,
        payments: genesy_360_api::payments::Payments::default().with_globadyme_environment(Some(
            genesy_360_api::payments::GlobadymeEnvironment::Sandbox,
        )),
        checkout_hold_max: 500,
        signup_enabled: true,
    }
}

pub fn app(pool: PgPool) -> Router {
    app_with_mail(pool).0
}

/// App plus captured outgoing mail.
pub fn app_with_mail(pool: PgPool) -> (Router, Outbox) {
    let (mailer, outbox) = Mailer::capture();
    (build_app(state(pool, mailer)), outbox)
}

/// Send request, return status + JSON body (`Value::Null` when body empty or not JSON).
/// Replies on documented routes are checked against `openapi.json` (see `contract`).
pub async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let (status, bytes) = send_raw(app, method.clone(), uri, body, bearer, &[]).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    contract::check(&method, uri, status, &body);
    (status, body)
}

/// `send` with extra headers; the body as raw bytes.
pub async fn send_raw(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::body::Bytes) {
    let (status, _, bytes) = send_full(app, method, uri, body, bearer, headers).await;
    (status, bytes)
}

/// `send_raw` plus the response headers.
pub async fn send_full(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, axum::body::Bytes) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let req = match body {
        Some(json) => req
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(PEER.parse::<SocketAddr>().unwrap()));

    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    (
        status,
        headers,
        res.into_body().collect().await.unwrap().to_bytes(),
    )
}

/// Seconds in the `Retry-After` header of `POST uri` with `body`, if any.
pub async fn retry_after(app: &Router, uri: &str, body: Value) -> (StatusCode, Option<u64>) {
    let (status, headers, _) = send_full(app, Method::POST, uri, Some(body), None, &[]).await;
    let secs = headers
        .get("retry-after")
        .map(|v| v.to_str().unwrap().parse().unwrap());
    (status, secs)
}

pub async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    send(app, Method::POST, uri, Some(body), None).await
}

pub const PASSWORD: &str = "Str0ng!pass";

/// Sign up through the API, return (access_token, workspace_id).
pub async fn signup(app: &Router, email: &str, workspace: &str) -> (String, String) {
    let (status, res) = post(
        app,
        "/v1/create-workspace",
        serde_json::json!({
            "first_name": "Ada",
            "last_name": "Lovelace",
            "email": email,
            "password": PASSWORD,
            "workspace_name": workspace,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "signup failed: {res}");
    (
        res["data"]["access_token"].as_str().unwrap().to_owned(),
        res["data"]["workspace_ids"][0].as_str().unwrap().to_owned(),
    )
}

/// Invite emails only (signup also sends a verification email to the same outbox).
pub fn invite_mails(outbox: &Outbox) -> Vec<genesy_360_api::mail::Outgoing> {
    mails_where(outbox, "invited")
}

/// Verification emails only.
pub fn verification_mails(outbox: &Outbox) -> Vec<genesy_360_api::mail::Outgoing> {
    mails_where(outbox, "Verify your email")
}

fn mails_where(outbox: &Outbox, subject_part: &str) -> Vec<genesy_360_api::mail::Outgoing> {
    outbox
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.subject.contains(subject_part))
        .cloned()
        .collect()
}

/// GET without following redirects; returns status + Location header (if any).
pub async fn get_redirect(app: &Router, uri: &str) -> (StatusCode, Option<String>) {
    let mut req = Request::get(uri).body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(PEER.parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    let location = res
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_owned());
    (res.status(), location)
}

/// Mark an email verified directly (tests that need a verified owner/invitee).
pub async fn verify_email(pool: &PgPool, email: &str) {
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE lower(email) = lower($1)")
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
}

/// Team in `ws` with `perms`, soft-deleted when `deleted`. Returns its id. Direct SQL, so tests
/// of other features don't depend on the teams endpoints.
pub async fn add_team(
    pool: &PgPool,
    ws: &str,
    name: &str,
    perms: &[&str],
    deleted: bool,
) -> String {
    let id: String = sqlx::query_scalar(
        "INSERT INTO teams (workspace_id, name, description, deleted_at)
         VALUES ($1::uuid, $2, 'd', CASE WHEN $3 THEN now() END) RETURNING id::text",
    )
    .bind(ws)
    .bind(name)
    .bind(deleted)
    .fetch_one(pool)
    .await
    .unwrap();
    for p in perms {
        sqlx::query("INSERT INTO team_permissions (team_id, permission) VALUES ($1::uuid, $2)")
            .bind(&id)
            .bind(p)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

pub async fn join_team(pool: &PgPool, team: &str, email: &str) {
    sqlx::query(
        "INSERT INTO team_members (team_id, user_id)
         SELECT $1::uuid, id FROM users WHERE email = $2",
    )
    .bind(team)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
}

/// Active membership straight in the DB, skipping the invite flow.
pub async fn join_workspace(pool: &PgPool, ws: &str, email: &str) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, user_id)
         SELECT $1::uuid, id FROM users WHERE email = $2",
    )
    .bind(ws)
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
}

/// `email` signs up (own workspace) and joins `ws` in a team holding `perms`. Returns their token.
pub async fn member_with(
    app: &Router,
    pool: &PgPool,
    ws: &str,
    email: &str,
    perms: &[&str],
) -> String {
    let own = format!("ws-{}", &email[..email.find('@').unwrap()]);
    let (token, _) = signup(app, email, &own).await;
    join_workspace(pool, ws, email).await;
    let team = add_team(pool, ws, "Crew", perms, false).await;
    join_team(pool, &team, email).await;
    token
}

/// One workspace resource under test: Ada owns `acme`, requests go to
/// `/v1/workspace/{ws}{base}`. Holds the access checks every reference-data route shares.
pub struct Api {
    pub app: Router,
    pub pool: PgPool,
    pub owner: String,
    pub ws: String,
    base: &'static str,
}

impl Api {
    pub async fn new(pool: PgPool, base: &'static str) -> Self {
        let app = app(pool.clone());
        let (owner, ws) = signup(&app, "ada@example.com", "acme").await;
        Self {
            app,
            pool,
            owner,
            ws,
            base,
        }
    }

    /// Same app and base, a new user owning a new workspace (cross-workspace checks).
    pub async fn as_new_owner(&self, email: &str, workspace: &str) -> Self {
        let (owner, ws) = signup(&self.app, email, workspace).await;
        Self {
            app: self.app.clone(),
            pool: self.pool.clone(),
            owner,
            ws,
            base: self.base,
        }
    }

    /// Same app, owner and workspace, requests under another base path (nested routes).
    pub fn with_base(&self, base: &'static str) -> Self {
        Self {
            app: self.app.clone(),
            pool: self.pool.clone(),
            owner: self.owner.clone(),
            ws: self.ws.clone(),
            base,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        token: &str,
    ) -> (StatusCode, Value) {
        let uri = format!("/v1/workspace/{}{}{path}", self.ws, self.base);
        send(&self.app, method, &uri, body, Some(token)).await
    }

    /// POST as owner, expect 201, return `data.<id_key>`.
    pub async fn create(&self, body: Value, id_key: &str) -> String {
        let (status, res) = self.call(Method::POST, "", Some(body), &self.owner).await;
        assert_eq!(status, StatusCode::CREATED, "{res}");
        res["data"][id_key].as_str().unwrap().to_owned()
    }

    /// GET one as owner, expect 200, return `data`.
    pub async fn get(&self, id: &str) -> Value {
        let (status, res) = self
            .call(Method::GET, &format!("/{id}"), None, &self.owner)
            .await;
        assert_eq!(status, StatusCode::OK, "{res}");
        res["data"].clone()
    }

    /// GET list as owner, expect 200, return the whole body.
    pub async fn list(&self, query: &str) -> Value {
        let (status, res) = self.call(Method::GET, query, None, &self.owner).await;
        assert_eq!(status, StatusCode::OK, "{res}");
        res
    }

    /// POST /{id} as owner; returns status + body.
    pub async fn update(&self, id: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, &format!("/{id}"), Some(body), &self.owner)
            .await
    }

    /// A member without permissions gets 403 on every route, before body validation (422).
    pub async fn assert_member_forbidden(&self, id: &str) {
        let bob = member_with(&self.app, &self.pool, &self.ws, "bob@example.com", &[]).await;
        for (method, path) in [
            (Method::GET, String::new()),
            (Method::POST, String::new()),
            (Method::GET, format!("/{id}")),
            (Method::POST, format!("/{id}")),
            (Method::DELETE, format!("/{id}")),
        ] {
            let body = Some(serde_json::json!({ "name": null }));
            let (status, res) = self.call(method.clone(), &path, body, &bob).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {res}");
            assert_eq!(res["message"], "This action is unauthorized.");
        }
    }

    /// `perms` let a member do everything: create, list, get, update, delete.
    pub async fn assert_granted(&self, perms: &[&str], body: Value, id_key: &str) {
        let carl = member_with(&self.app, &self.pool, &self.ws, "carl@example.com", perms).await;
        let (status, res) = self.call(Method::POST, "", Some(body.clone()), &carl).await;
        assert_eq!(status, StatusCode::CREATED, "{res}");
        let id = res["data"][id_key].as_str().unwrap().to_owned();
        for (method, path, body) in [
            (Method::GET, String::new(), None),
            (Method::GET, format!("/{id}"), None),
            (Method::POST, format!("/{id}"), Some(body)),
            (Method::DELETE, format!("/{id}"), None),
        ] {
            let (status, res) = self.call(method.clone(), &path, body, &carl).await;
            assert_eq!(status, StatusCode::OK, "{method} {path}: {res}");
        }
    }

    /// Another tenant can't reach this workspace, nor this row through its own workspace,
    /// and never sees the row in its own list.
    pub async fn assert_foreign_404(&self, id: &str, update: Value) {
        let (eve, eve_ws) = signup(&self.app, "eve@example.com", "evil").await;
        let (status, _) = self.call(Method::GET, "", None, &eve).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "outsider list");
        for method in [Method::GET, Method::POST, Method::DELETE] {
            let uri = format!("/v1/workspace/{eve_ws}{}/{id}", self.base);
            let body = (method == Method::POST).then(|| update.clone());
            let (status, _) = send(&self.app, method.clone(), &uri, body, Some(&eve)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} via own workspace");
        }
        let uri = format!("/v1/workspace/{eve_ws}{}?page_size=100", self.base);
        let (_, listed) = send(&self.app, Method::GET, &uri, None, Some(&eve)).await;
        assert!(
            listed["data"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["id"] != id),
            "foreign list leaks {id}"
        );
        let (status, _) = self
            .call(Method::GET, &format!("/{id}"), None, &self.owner)
            .await;
        assert_eq!(status, StatusCode::OK, "row untouched");
    }

    /// Malformed, unknown, and soft-deleted ids are all 404.
    pub async fn assert_bad_ids_404(&self, deleted: &str, update: Value) {
        let unknown = "01900000-0000-7000-8000-000000000001";
        for id in ["nope", "123", unknown, deleted] {
            for method in [Method::GET, Method::POST, Method::DELETE] {
                let body = (method == Method::POST).then(|| update.clone());
                let (status, _) = self
                    .call(method.clone(), &format!("/{id}"), body, &self.owner)
                    .await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{method} {id}");
            }
        }
    }

    /// DELETE as owner -> `{"message":"ok"}`.
    pub async fn delete(&self, id: &str) {
        let (status, res) = self
            .call(Method::DELETE, &format!("/{id}"), None, &self.owner)
            .await;
        assert_eq!(
            (status, res),
            (StatusCode::OK, serde_json::json!({ "message": "ok" }))
        );
    }
}
