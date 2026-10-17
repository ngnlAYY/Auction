//! 应用配置模块装配；TOML 输入模型、加载、getter 与最终校验构建由子模块拥有，
//! 此处只稳定导出 `AppConfig`。

mod accessors;
mod build;
mod load;
mod model;

pub use model::AppConfig;
