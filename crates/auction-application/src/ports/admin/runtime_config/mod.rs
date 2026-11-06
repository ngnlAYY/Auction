//! Admin runtime 配置端口的模块装配。

mod business;
mod commands;
mod document;
mod errors;
mod hot;
mod mail_templates;
mod operations;
mod ports;
mod validation;

pub use business::{
    AdminRuntimeEquipmentFilterConfig, AdminRuntimeEquipmentLevelConfig, AdminRuntimeWorldConfig,
    AdminRuntimeWorldsConfig,
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
pub use hot::{
    WORKER_HOT_FIELD_PATHS, WORKER_RESTART_ONLY_FIELD_PATHS, WorkerDeliveryHotConfig,
    WorkerFieldEffect, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
    worker_hot_field_effect,
};
pub use mail_templates::{AdminRuntimeMailTemplatesConfig, MailTemplateError};
pub use operations::{
    AdminRuntimeAlertsConfig, AdminRuntimeDeliveryConfig, AdminRuntimeEconomicsConfig,
    AdminRuntimeForumConfig, AdminRuntimeFulfillmentConfig, AdminRuntimeMailConfig,
    AdminRuntimeMailDeliveryBudget, AdminRuntimeOperationsConfig, AdminRuntimeRecoveryConfig,
    AdminRuntimeScheduleConfig, AdminRuntimeWorkerConfig,
};
pub use ports::{AdminRuntimeCommandStore, AdminRuntimeConfigPort, AdminRuntimeFileStore};
pub use validation::{valid_allowlist_host, valid_allowlist_path_prefix, valid_listener_group};
