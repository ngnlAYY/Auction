mod bootstrap;
mod web_assets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await
}
