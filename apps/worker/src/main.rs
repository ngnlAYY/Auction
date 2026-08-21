use anyhow::Context;
use auction_config::{AppConfig, Role};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    auction_observability::init("auction_worker", config.log_filter.as_deref())
        .context("initialize tracing")?;
    tracing::info!(database_configured = config.database_url.is_some(), "worker configured");

    auction_jobs::run_until_shutdown().await?;
    Ok(())
}
