use super::handlers;
use crate::AppState;
use utoipa_axum::{router::OpenApiRouter, routes};
pub fn private() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(handlers::list, handlers::create))
        .routes(routes!(handlers::get, handlers::update, handlers::archive))
}
