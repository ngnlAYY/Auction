//! Public API 进程的薄入口；装配与生命周期由库 crate 的 `run` 拥有。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_public_api::run().await.map_err(|error| {
        // 默认 Termination 会格式化整条 anyhow 链；仅在输出边界投影 join 原因。
        match auction_process::task_error_kind(error.as_ref()) {
            Some(kind) => anyhow::anyhow!("public-api task failed: {kind}"),
            None => error,
        }
    })
}
