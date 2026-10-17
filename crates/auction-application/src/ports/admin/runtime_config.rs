//! Admin runtime 配置端口的模块装配。

#[path = "runtime_config/business.rs"]
mod business;
#[path = "runtime_config/commands.rs"]
mod commands;
#[path = "runtime_config/document.rs"]
mod document;
#[path = "runtime_config/errors.rs"]
mod errors;
#[path = "runtime_config/operations/mod.rs"]
mod operations;
#[path = "runtime_config/ports.rs"]
mod ports;
#[path = "runtime_config/validation.rs"]
mod validation;

pub use business::{
    AdminRuntimeEquipmentFilterConfig, AdminRuntimeEquipmentLevelConfig,
    AdminRuntimeMailTemplatesConfig, AdminRuntimeWorldConfig, AdminRuntimeWorldsConfig,
};
pub use commands::{
    AdminRuntimeAuditRecord, AdminRuntimeCommandState, AdminRuntimeReplacement,
    AdminRuntimeStoredCommand, AdminRuntimeUpdateCommand,
};
pub use document::{
    ADMIN_RUNTIME_DIGEST_VERSION, AdminRuntimeDashboardMailbox, AdminRuntimeDocument,
    AdminRuntimeEquipmentSourceConfig, AdminRuntimeListenMode, AdminRuntimeListenerConfig,
    AdminRuntimeMetricsConfig, AdminRuntimeSecurityConfig, AdminRuntimeSessionConfig,
    AdminRuntimeSnapshot, AdminRuntimeTracingConfig, AdminRuntimeWorkerControlConfig,
    MAX_ADMIN_RUNTIME_DOCUMENT_BYTES,
};
pub use errors::{AdminRuntimeCommandError, AdminRuntimeConfigError, AdminRuntimeFileError};
pub use operations::{
    AdminRuntimeAlertsConfig, AdminRuntimeDeliveryConfig, AdminRuntimeEconomicsConfig,
    AdminRuntimeForumConfig, AdminRuntimeFulfillmentConfig, AdminRuntimeMailConfig,
    AdminRuntimeOperationsConfig, AdminRuntimeRecoveryConfig, AdminRuntimeScheduleConfig,
    AdminRuntimeWorkerConfig,
};
pub use ports::{AdminRuntimeCommandStore, AdminRuntimeConfigPort, AdminRuntimeFileStore};
