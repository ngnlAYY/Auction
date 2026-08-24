//! Worker、论坛和 MoogleMail 配置模型的模块装配。
//!
//! 每个 Worker owner 在独立文件中维护自己的配置字段和默认值；这里仅负责稳定导出，
//! 不改变 `[worker]` 的 TOML 结构或 `auction-config` 的公共类型路径。

mod auction;
mod control;
mod delivery;
mod forum;
mod fulfillment;
mod mooglemail;
mod operations_alert;
mod recovery;

pub use auction::WorkerConfig;
pub use control::WorkerControlConfig;
pub use delivery::DeliveryWorkerConfig;
pub use forum::ForumSyncWorkerConfig;
pub use fulfillment::FulfillmentWorkerConfig;
pub use mooglemail::{
    MoogleMailEquipmentFilterConfig, MoogleMailNotificationTemplatesConfig, MoogleMailWorkerConfig,
    MoogleMailWorldConfig,
};
pub use operations_alert::OperationsAlertWorkerConfig;
pub use recovery::RecoveryWorkerConfig;
