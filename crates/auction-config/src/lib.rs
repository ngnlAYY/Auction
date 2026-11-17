//! TOML 配置与敏感配置加载的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod reader;
mod runtime;
mod scheduling;
mod settings;
#[cfg(unix)]
mod setup;

pub use runtime::{
    RuntimeFileStamp, RuntimeFileStoreError, TomlRuntimeFileStore, load_worker_hot_config,
    worker_hot_config,
};
pub use scheduling::{StandardCronSchedule, StandardCronScheduleError};
pub use settings::*;
#[cfg(unix)]
pub use setup::{FirstRunConfig, FirstRunError, managed_runtime_directory};
