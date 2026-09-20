#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_worker::run().await
}
