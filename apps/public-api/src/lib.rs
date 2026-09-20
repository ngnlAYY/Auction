//! Public 角色的可复用装配入口；每次调用必须位于独立进程。

mod bootstrap;
mod web_assets;

pub use bootstrap::{run, run_from_path};
