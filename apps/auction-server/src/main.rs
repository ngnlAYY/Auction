//! 可选单文件服务的薄装配入口。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await.map_err(|error| match auction_process::task_error_kind(error.as_ref()) {
        Some(kind) => anyhow::anyhow!("server task failed: {kind}"),
        None => error,
    })
}
