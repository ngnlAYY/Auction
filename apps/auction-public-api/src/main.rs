//! Public API 进程的薄入口；装配与生命周期由库 crate 的 `run` 拥有。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// 二进制 crate 里从未使用的 pub 条目不在默认的 dead_code 检查范围内。测试构建的入口是测试框架，
// 只经生产入口可达的条目会被误报，所以只在非测试构建打开。
#![cfg_attr(not(test), warn(dead_code_pub_in_binary))]

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
