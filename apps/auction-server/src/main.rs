//! 可选单文件服务的薄装配入口。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod bootstrap;

// 只监管子进程并转发信号：单线程 runtime 足以覆盖等待、超时与 shutdown，避免每个核各起一个空闲工作线程。
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await.map_err(|error| match auction_process::task_error_kind(error.as_ref()) {
        Some(kind) => anyhow::anyhow!("server task failed: {kind}"),
        None => error,
    })
}
