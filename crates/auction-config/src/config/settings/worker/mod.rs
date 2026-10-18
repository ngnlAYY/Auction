//! Worker、论坛和 MoogleMail 配置模型的模块装配；每个 owner
//! 在独立文件维护字段与默认值，`[worker]` TOML 结构不变。

mod auction;
mod control;
mod delivery;
mod forum;
mod fulfillment;
mod mooglemail;
mod operations_alert;
mod recovery;
mod runtime;

pub(super) use runtime::{apply_equipment_colors, apply_runtime_document};

pub use auction::WorkerConfig;
pub use control::WorkerControlConfig;
pub use delivery::DeliveryWorkerConfig;
pub use forum::ForumSyncWorkerConfig;
pub use fulfillment::FulfillmentWorkerConfig;
pub use mooglemail::{
    MoogleMailDiagnosticsConfig, MoogleMailEquipmentFilterConfig,
    MoogleMailNotificationTemplatesConfig, MoogleMailWorkerConfig, MoogleMailWorldConfig,
};
pub use operations_alert::OperationsAlertWorkerConfig;
pub use recovery::RecoveryWorkerConfig;
