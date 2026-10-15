//! Linux x86_64 运维 CLI 的薄装配入口。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

#[path = "bootstrap/runtime.rs"]
mod bootstrap;

#[tokio::main]
async fn main() {
    if let Err(error) = bootstrap::run().await {
        eprintln!("auctionctl: {error}");
        std::process::exit(i32::from(error.exit_code()));
    }
}
