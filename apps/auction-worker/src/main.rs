//! Worker 进程的薄入口；装配与生命周期由库 crate 的 `run` 拥有。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_worker::run().await.map_err(|error| {
        // WorkerError 和原 join source 在运行时保持原样，只收窄最终进程错误报告。
        match auction_process::task_error_kind(error.as_ref()) {
            Some(kind) => anyhow::anyhow!("worker 任务失败：{kind}"),
            None => error,
        }
    })
}
