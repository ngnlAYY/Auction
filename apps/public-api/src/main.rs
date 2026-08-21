use anyhow::Context;
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load public-api configuration")?;
    auction_observability::init("auction_public_api", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let cache = configure_cache(&config).context("configure public-api cache")?;
    let address = config.listen_addr.context("public-api must have a listen address")?;
    let listener = TcpListener::bind(address).await?;
    let app = auction_http::public_router::<()>();

    tracing::info!(%address, cache_backend = cache.backend(), "public api listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn configure_cache(config: &AppConfig) -> anyhow::Result<CacheStore> {
    match config.cache.backend {
        CacheBackend::Disabled => Ok(CacheStore::disabled()),
        CacheBackend::Redis => {
            let redis_url = config.redis_url().context("redis cache URL is missing")?;
            CacheStore::redis(redis_url).map_err(Into::into)
        }
    }
}
