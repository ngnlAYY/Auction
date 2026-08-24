//! 进程配置模型、加载和校验的模块边界。
//!
//! `settings/` 按配置域拥有普通/敏感 TOML 的稳定字段、默认值、构建和校验；本入口只负责
//! 导出配置公共 API。runtime TOML 文件的原子读写另由 `src/runtime/` owner 负责。

mod settings;

pub use settings::*;
