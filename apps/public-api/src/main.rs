#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_public_api::run().await
}
