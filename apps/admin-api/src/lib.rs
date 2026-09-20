//! Admin 角色及首次设置的装配入口；每个角色保留独立进程。

mod bootstrap;
mod web_assets;

pub use bootstrap::{run, run_from_path};
pub use web_assets::AdminWebAssets;
