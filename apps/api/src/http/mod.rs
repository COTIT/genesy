//! Shared HTTP glue: extractors used by more than one feature.

pub mod app_key;
pub mod assistant;
pub mod auth;
pub mod farm;
pub mod json;
pub mod list;
pub mod rate_limit;
pub mod response;
pub mod slots;
pub mod timeout;
pub mod trace;
pub mod workspace;

/// App state with test defaults: high limits, captured mail, no optional services.
#[cfg(test)]
pub(crate) fn test_state(pool: sqlx::PgPool) -> crate::AppState {
    use crate::AppState;
    use chrono::Duration;
    AppState {
        health_db: pool.clone(),
        db: pool,
        token_ttl: Duration::days(30),
        mailer: crate::mail::Mailer::capture().0,
        frontend_url: "https://app.test".into(),
        api_public_url: "https://api.test".into(),
        shop_public_url: Some("https://shop.test".into()),
        rate_limit: crate::RateLimit {
            burst: 1000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        invites_per_hour: 1000,
        private_rate_limit: crate::RateLimit {
            burst: 1000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        preview_rate_limit: crate::RateLimit {
            burst: 1000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        private_ip_rate_limit: crate::RateLimit {
            burst: 1000,
            replenish: std::time::Duration::from_millis(1),
            trust_forwarded: false,
        },
        slots: crate::Slots::new(crate::WorkspaceSlots {
            per_workspace: 1000,
            per_user: 1000,
            wait: std::time::Duration::from_secs(5),
        }),
        request_timeout: std::time::Duration::from_secs(60),
        body_deadline: std::time::Duration::from_secs(60),
        credentials: None,
        assistant_bridge_secret: None,
        product_bulk_enabled: true,
        anthropic_api_key: None,
        files: None,
        pdf: None,
        payments: crate::payments::Payments::default()
            .with_globadyme_environment(Some(crate::payments::GlobadymeEnvironment::Sandbox)),
        checkout_hold_max: 500,
        signup_enabled: true,
    }
}
