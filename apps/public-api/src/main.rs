#![recursion_limit = "512"]

use anyhow::Context;
use auction_config::{AppConfig, Role};
use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load public-api configuration")?;
    auction_observability::init("auction_public_api", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let address = config.listen_addr.context("public-api must have a listen address")?;
    let listener = TcpListener::bind(address).await?;
    let leptos_config = get_configuration(Some("crates/auction-web/Cargo.toml"))
        .context("load Leptos configuration")?;
    let leptos_options = leptos_config.leptos_options;
    let routes = generate_route_list(auction_web::app::App);
    let app = auction_http::public_router::<LeptosOptions>()
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || auction_web::app::shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(auction_web::app::shell))
        .with_state(leptos_options);

    tracing::info!(%address, "public api listening");
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
