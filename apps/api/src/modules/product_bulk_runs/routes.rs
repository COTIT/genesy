use super::{dto::MAX_SOURCE_BYTES, handlers};
use crate::AppState;
use axum::extract::DefaultBodyLimit;
use utoipa_axum::{router::OpenApiRouter, routes};
pub fn private() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(handlers::list, handlers::create))
        .routes(routes!(handlers::settings_get, handlers::settings_set))
        .routes(routes!(handlers::mappings_get, handlers::mappings_save))
        .routes(routes!(handlers::template))
        .routes(routes!(handlers::get))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(handlers::source))
                .layer(DefaultBodyLimit::max(MAX_SOURCE_BYTES + 65536)),
        )
        .merge(
            OpenApiRouter::new()
                .routes(routes!(handlers::stage))
                .layer(DefaultBodyLimit::max(3 * 1024 * 1024)),
        )
        .routes(routes!(handlers::mapping))
        .routes(routes!(handlers::rows))
        .routes(routes!(handlers::row))
        .routes(routes!(handlers::raw))
        .routes(routes!(handlers::review))
        .routes(routes!(handlers::approve))
        .routes(routes!(handlers::cancel))
        .routes(routes!(handlers::remainder))
        .routes(routes!(handlers::resume))
        .routes(routes!(handlers::report))
}
