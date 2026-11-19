//! 可选单文件服务的薄装配入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// 二进制 crate 里从未使用的 pub 条目不在默认的 dead_code 检查范围内。测试构建的入口是测试框架，
// 只经生产入口可达的条目会被误报，所以只在非测试构建打开。
#![cfg_attr(not(test), warn(dead_code_pub_in_binary))]

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod bootstrap;

// supervisor 保持单线程：子进程由主线程派生，PR_SET_PDEATHSIG 绑定派生线程，因此不得改成多线程；
// `--role` 子角色与分体部署一致，使用多线程 runtime。选择逻辑由 bootstrap 拥有。
fn main() -> anyhow::Result<()> {
    bootstrap::main().map_err(|error| match auction_process::task_error_kind(error.as_ref()) {
        Some(kind) => anyhow::anyhow!("server 任务失败：{kind}"),
        None => error,
    })
}
