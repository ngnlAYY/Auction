//! Admin 装备来源与 Public 真实装备 URL 适配器；canonical JSON 与旧版 Hentaiverse
//! 装备页最终归一为同一份脱敏快照。

mod config;
mod game_link;
mod model;
mod parser;
mod reader;
mod worker_proxy;

pub use config::EquipmentSourcePolicy;
pub use game_link::EquipmentGameLinkResolver;
pub use reader::{EquipmentSourceAdapter, build_equipment_http_client, canonical_json_snapshot};
pub use worker_proxy::WorkerEquipmentSourceReader;
