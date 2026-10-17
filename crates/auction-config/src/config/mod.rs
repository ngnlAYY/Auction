//! 进程配置模型、加载和校验的模块边界；`settings/` 按配置域拥有 TOML 字段与校验，
//! runtime TOML 原子读写由 `src/runtime/` 负责。

mod settings;

pub use settings::*;
