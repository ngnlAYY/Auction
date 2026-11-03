//! CLI 连接用途装配；读取 Worker 敏感目标不会把运维连接归入 Worker 服务档。

use auction_config::{AppConfig, DatabaseProfileMode};
use auction_storage::{DatabaseConnectionProfile, DatabaseConnectionPurpose};

pub(crate) fn resolve(
    config: &AppConfig,
    purpose: DatabaseConnectionPurpose,
) -> DatabaseConnectionProfile {
    if config.database_profiles.mode == DatabaseProfileMode::Legacy {
        return DatabaseConnectionProfile::legacy();
    }
    let maintenance = config.database_profiles.maintenance;
    DatabaseConnectionProfile::maintenance(
        purpose,
        maintenance.max_connections,
        maintenance.acquire_timeout_millis,
    )
}
