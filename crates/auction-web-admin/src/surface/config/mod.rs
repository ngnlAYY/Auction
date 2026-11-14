//! Admin runtime 配置页面。
//! 页面用类型化表单编辑服务端下发的完整非敏感投影；候选必须先经过服务端规范化校验，
//! 更新再以 fingerprint 和可重试的 command ID 作为版本与幂等边界。成功更新会由服务端
//! 原子写入 runtime TOML，角色启动配置、公共配置、密钥和命令账本永远不进入浏览器。

mod draft;
mod early_bird;
mod form;
mod hot_fields;
mod mail_templates;
mod model;
mod operational_fields;
mod page;
mod review;
mod runtime_switches;
mod validation;
mod worlds;
pub(super) use page::{page, use_config_draft};
