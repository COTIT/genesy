//! App assembly: shared state + router. main.rs boots it, tests call build_app directly.
// The product resource is one large `json!` literal.
#![recursion_limit = "512"]

pub mod bootstrap;
pub mod cleanup;
pub mod config;
mod crypto;
pub mod db;
pub mod demo;
mod error;
pub mod farm_demo;
mod health;
pub mod healthcheck;
mod http;
mod idempotency;
pub mod jobs;
pub mod mail;
mod modules;
mod money;
mod numbering;
pub mod openapi;
pub mod outbound;
pub mod payments;
pub mod pdf;
mod php;
pub mod storage;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, header},
    routing::get,
};
use sqlx::PgPool;
use tower_http::{set_header::SetResponseHeaderLayer, trace::TraceLayer};

pub use crypto::CredentialKey;
/// Job handlers (`jobs::runner::Handler`) return it.
pub use error::AppError;
pub use http::assistant::AssistantSecret;
pub use http::{
    rate_limit::RateLimit,
    slots::{Slots, WorkspaceSlots},
};
pub use modules::gateway;
pub use modules::invoice_payments;
pub use modules::invoices::gateway as invoice_gateway;
pub use modules::{product_bulk_runs, product_fields};

/// Largest JSON body is a relation with its addresses (capped at 50); 64 KiB covers it
/// and keeps junk bodies (axum default 2 MB) off the parser.
const BODY_LIMIT: usize = 64 * 1024;

/// Handles every handler can reach. Clone is cheap, pool is Arc inside.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    /// One connection for /healthz (`db::health`), apart from the request pool.
    pub health_db: PgPool,
    /// Lifetime of newly issued bearer tokens.
    pub token_ttl: chrono::Duration,
    pub mailer: mail::Mailer,
    /// vimdesk-web origin for email links, no trailing slash.
    pub frontend_url: std::sync::Arc<str>,
    /// This API's public origin, for links that must hit the API (email verification).
    pub api_public_url: std::sync::Arc<str>,
    /// Trusted deployed Shop origin, independent of shopper-supplied request headers.
    pub shop_public_url: Option<std::sync::Arc<str>>,
    /// Applied to public auth routes only, per client IP.
    pub rate_limit: RateLimit,
    /// Applied to authenticated routes, per session (bearer token).
    pub private_rate_limit: RateLimit,
    /// Ceiling per client IP on authenticated routes, around the per-session limit.
    pub private_ip_rate_limit: RateLimit,
    /// Storefront preview-link resolve, per app key, inside the private limits.
    pub preview_rate_limit: RateLimit,
    /// Requests in flight per workspace and per user, taken by `WorkspaceAccess`.
    pub slots: Slots,
    /// Longest a `/v1` request may run (then 503).
    pub request_timeout: std::time::Duration,
    /// Longest a request body may take to arrive (then 408); item totals get twice this.
    pub body_deadline: std::time::Duration,
    /// Max invites one user may create or re-send per hour.
    pub invites_per_hour: i64,
    /// Seals integration credentials (`CREDENTIALS_KEY`); None refuses storing one.
    pub credentials: Option<std::sync::Arc<CredentialKey>>,
    /// Server-only assistant bridge signing key; None refuses signed controls.
    pub assistant_bridge_secret: Option<std::sync::Arc<AssistantSecret>>,
    /// Rollout admission switch; disabling it does not hide or discard existing runs.
    pub product_bulk_enabled: bool,
    pub anthropic_api_key: Option<std::sync::Arc<AssistantSecret>>,
    /// File objects (the uploads bucket); None answers file routes with 503.
    pub files: Option<storage::Store>,
    /// HTML to PDF (Gotenberg); None answers PDF routes with 503.
    pub pdf: Option<pdf::Renderer>,
    /// Web payments (`PAYMENTS_PROVIDER`); None answers payment intents with 503.
    pub payments: payments::Payments,
    /// Most web orders one workspace may have awaiting payment (`CHECKOUT_HOLD_MAX`).
    pub checkout_hold_max: i64,
    /// `SIGNUP_ENABLED`: `POST /v1/create-workspace` answers; off = 404.
    pub signup_enabled: bool,
}

/// One-off boot work before serving: login timing-equalizer hash, assistant operation inventory.
pub async fn warm_up() {
    if let Err(err) = modules::auth::password::warm_up().await {
        tracing::warn!(error = %err, "password warm-up failed; first unknown-email login is slower");
    }
    // Spec build costs ~0.1-0.8 s; paid here, not by first assistant proposal or dispatch.
    match tokio::task::spawn_blocking(modules::assistant::service::warm_operations).await {
        Ok(true) => {}
        _ => tracing::warn!("assistant operation inventory warm-up failed"),
    }
}

/// True once the assistant operation inventory is built.
pub fn assistant_operations_ready() -> bool {
    modules::assistant::service::operations_ready()
}

/// Full router. One entry point so tests run same middleware stack as prod.
/// Serve with `into_make_service_with_connect_info::<SocketAddr>()`: the rate limit keys on peer IP.
pub fn build_app(state: AppState) -> Router {
    let public = http::rate_limit::apply(openapi::public().split_for_parts().0, state.rate_limit);
    let public = http::timeout::body_deadline(public, state.body_deadline);
    let private = Router::new().merge(openapi::private().split_for_parts().0);
    // Item totals, invoices and file uploads take large bodies: twice the deadline of the
    // 64 KiB routes.
    let documents = openapi::documents().split_for_parts().0;
    let previews = http::rate_limit::apply_session(
        openapi::previews().split_for_parts().0,
        state.preview_rate_limit,
    );
    let private = http::timeout::body_deadline(private, state.body_deadline)
        .merge(http::timeout::body_deadline(
            documents,
            state.body_deadline * 2,
        ))
        .merge(http::timeout::body_deadline(previews, state.body_deadline));
    let private = http::timeout::request_timeout(private, state.request_timeout);
    let private = http::assistant::apply(private, state.clone());
    let private = http::rate_limit::apply_private(
        private,
        state.private_rate_limit,
        state.private_ip_rate_limit,
    );
    state.slots.spawn_cleanup();
    let public = http::timeout::request_timeout(public, state.request_timeout);
    let v1 = public
        .merge(private)
        // Responses carry bearer tokens and invite data: never cache, never sniff.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    Router::new()
        .route("/healthz", get(health::healthz))
        .nest("/v1", v1)
        .layer(TraceLayer::new_for_http().make_span_with(http::trace::span))
        .with_state(state)
}
