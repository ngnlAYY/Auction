#[path = "bootstrap/runtime.rs"]
mod bootstrap;

#[tokio::main]
async fn main() {
    if let Err(error) = bootstrap::run().await {
        eprintln!("auctionctl: {error}");
        std::process::exit(i32::from(error.exit_code()));
    }
}
