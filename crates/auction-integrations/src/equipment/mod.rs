//! Admin 装备来源与 Public 真实装备 URL 适配器。
//!
//! 本模块只负责装配固定来源适配器；读取策略、规范化模型、远端页面解析和传输边界分别由
//! 子模块拥有。canonical JSON 与旧版 Hentaiverse 装备页最终归一为同一份脱敏快照。

mod config;
mod game_link;
mod model;
mod parser;
mod reader;

pub use config::{EquipmentSourceConfigurationError, EquipmentSourcePolicy};
pub use game_link::EquipmentGameLinkResolver;
pub use reader::{EquipmentSourceAdapter, build_equipment_http_client};
