//! 可选单文件服务的薄装配入口。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod bootstrap;

// supervisor 保持单线程：子进程由主线程派生，PR_SET_PDEATHSIG 绑定派生线程，因此不得改成多线程；
// `--role` 子角色与分体部署一致，使用多线程 runtime。选择逻辑由 bootstrap 拥有。
fn main() -> anyhow::Result<()> {
    bootstrap::main().map_err(|error| match auction_process::task_error_kind(error.as_ref()) {
        Some(kind) => anyhow::anyhow!("server task failed: {kind}"),
        None => error,
    })
}
