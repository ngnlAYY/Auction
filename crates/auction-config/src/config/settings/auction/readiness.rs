//! 三角色同源的审计、迁移就绪策略与 HTTP 探针缓存开关。

use serde::Deserialize;

/// 启动时冻结的 readiness 配置；修改后须重启，不由 Admin runtime 覆盖。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReadinessConfig {
    /// 审计失败模式；默认 shadow，影子期仍由全部失败累计阻断就绪。
    pub audit_failure_mode: auction_application::AuditFailureMode,
    /// 迁移兼容模式；默认 shadow，只有显式 compatible 才放宽数据库超前。
    pub migration_compatibility_mode: auction_application::MigrationCompatibilityMode,
    /// 是否缓存 HTTP 最终判定并合并并发检查；默认启用。
    pub cache_enabled: bool,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            audit_failure_mode: auction_application::AuditFailureMode::Shadow,
            migration_compatibility_mode: auction_application::MigrationCompatibilityMode::Shadow,
            cache_enabled: true,
        }
    }
}
