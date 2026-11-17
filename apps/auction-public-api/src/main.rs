//! Public API 进程的薄入口；装配与生命周期由库 crate 的 `run` 拥有。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_public_api::run().await.map_err(|error| {
        // 默认 Termination 会格式化整条 anyhow 链；仅在输出边界投影 join 原因。
        match auction_process::task_error_kind(error.as_ref()) {
            Some(kind) => anyhow::anyhow!("public-api 任务失败：{kind}"),
            None => error,
        }
    })
}
