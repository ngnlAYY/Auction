/// Worker lifecycle placeholder. The concrete scheduler will own durable
/// cursors and retry state instead of relying on in-memory timers.
pub async fn run_until_shutdown() -> std::io::Result<()> {
    tracing::info!("auction worker started");
    tokio::signal::ctrl_c().await?;
    tracing::info!("auction worker stopping");
    Ok(())
}
