//! 应用配置模块装配。
//!
//! TOML 输入模型、加载、公共 getter 和最终校验构建分别由子模块拥有；本文件只保留
//! `AppConfig` 的稳定导出，避免配置入口重新变成巨型实现文件。

mod accessors;
mod build;
mod load;
mod model;

pub use model::AppConfig;
