//! Worker 进程的薄入口；装配与生命周期由库 crate 的 `run` 拥有。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_worker::run().await.map_err(|error| {
        // WorkerError 和原 join source 在运行时保持原样，只收窄最终进程错误报告。
        match auction_process::task_error_kind(error.as_ref()) {
            Some(kind) => anyhow::anyhow!("worker task failed: {kind}"),
            None => error,
        }
    })
}
