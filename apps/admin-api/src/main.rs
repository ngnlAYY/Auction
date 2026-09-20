#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_admin_api::run().await
}
