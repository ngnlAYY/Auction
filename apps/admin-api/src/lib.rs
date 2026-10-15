//! Admin 角色及首次设置的装配入口；每个角色保留独立进程。

// 原生部署限定 Linux x86_64；浏览器 WASM 由独立 web crate 构建。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("Auction native applications require Linux x86_64");

mod bootstrap;
mod web_assets;

pub use bootstrap::{run, run_from_path};
pub use web_assets::AdminWebAssets;
