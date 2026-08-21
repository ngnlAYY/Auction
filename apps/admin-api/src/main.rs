use anyhow::Context;
use auction_cache::CacheStore;
use auction_config::{AppConfig, CacheBackend, Role};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Admin).context("load admin-api configuration")?;
    auction_observability::init("auction_admin_api", config.log_filter.as_deref())
        .context("initialize tracing")?;
    let cache = configure_cache(&config).context("configure admin-api cache")?;
    let address = config.listen_addr.context("admin-api must have a listen address")?;
    let listener = TcpListener::bind(address).await?;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let app = auction_http::admin_router::<()>(env!("CARGO_PKG_VERSION"));

    tracing::info!(%address, cache_backend = cache.backend(), "admin api listening");
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
