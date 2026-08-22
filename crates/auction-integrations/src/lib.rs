//! 外部服务适配器的薄 crate 入口。

mod clients;
mod cryptobox;
mod equipment;
mod forum;

pub use clients::{build_http_client, ehentai, mooglemail};
pub use cryptobox::{DataKey, EquipmentKeySealer, EquipmentKeySealerError};
pub use equipment::{
    EquipmentSourceAdapter, EquipmentSourceConfigurationError, build_equipment_http_client,
};
pub use forum::ForumPreviewAdapter;
