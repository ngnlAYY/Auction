//! Admin runtime 配置端口的模块装配。
//!
//! 文档模型、命令账本、适配器端口和本地校验分别由子模块持有；本文件只保留稳定的
//! 公共类型出口，避免把不同职责重新堆回一个巨型模块。

#[path = "runtime_config/commands.rs"]
mod commands;
#[path = "runtime_config/document.rs"]
mod document;
#[path = "runtime_config/errors.rs"]
mod errors;
#[path = "runtime_config/ports.rs"]
mod ports;
#[path = "runtime_config/validation.rs"]
mod validation;

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
pub use ports::{AdminRuntimeCommandStore, AdminRuntimeConfigPort, AdminRuntimeFileStore};
