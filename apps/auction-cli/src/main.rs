//! Auction 运维命令行的薄入口；命令定义、分派与退出码由 `app` 模块拥有。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod app;
mod backup;
mod cli;
mod config_dir;
mod database;
mod deploy;
mod operations;
mod receipt;
mod service;
mod support;

#[tokio::main]
async fn main() {
    // 备份、证据与 passfile 只能由调用账号读取；子进程（pg_dump 等）继承同一 umask。
    // SAFETY：umask 只修改本进程的文件创建掩码，不涉及指针。
    unsafe { libc::umask(0o077) };
    let code = app::run().await;
    std::process::exit(i32::from(code));
}
