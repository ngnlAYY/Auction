//! 三角色同源的审计、迁移就绪策略与 HTTP 探针缓存开关。

use serde::Deserialize;

/// 启动时冻结的 readiness 配置；修改后须重启，不由 Admin runtime 覆盖。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReadinessConfig {
    /// 审计失败模式；默认 shadow，classified 须经影子观察后由用户显式切换。
    pub audit_failure_mode: auction_application::operations::AuditFailureMode,
    /// 迁移兼容模式；默认 shadow，只有显式 compatible 才放宽数据库超前。
    pub migration_compatibility_mode: auction_application::operations::MigrationCompatibilityMode,
    /// 是否缓存 HTTP 最终判定并合并并发检查；默认启用。
    pub cache_enabled: bool,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            audit_failure_mode: auction_application::operations::AuditFailureMode::Shadow,
            migration_compatibility_mode:
                auction_application::operations::MigrationCompatibilityMode::Shadow,
            cache_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReadinessConfig;
    use auction_application::operations::{AuditFailureMode, MigrationCompatibilityMode};

    #[test]
    fn default_baseline_shadows_audit_failures() {
        let config = ReadinessConfig::default();
        // 决策 72：默认影子观察，classified 须由用户显式切换。
        assert_eq!(config.audit_failure_mode, AuditFailureMode::Shadow);
        assert_eq!(config.migration_compatibility_mode, MigrationCompatibilityMode::Shadow);
        assert!(config.cache_enabled);
    }

    #[test]
    fn omitted_audit_mode_uses_the_shadow_field_default() {
        let config: ReadinessConfig = toml::from_str("").expect("空表必须全部取默认值");
        assert_eq!(config.audit_failure_mode, AuditFailureMode::Shadow);
    }

    #[test]
    fn every_audit_mode_stays_selectable() {
        for (value, expected) in [
            ("legacy", AuditFailureMode::Legacy),
            ("shadow", AuditFailureMode::Shadow),
            ("classified", AuditFailureMode::Classified),
        ] {
            let source = format!("audit_failure_mode = \"{value}\"");
            let config: ReadinessConfig = toml::from_str(&source).expect("合法枚举必须接受");
            assert_eq!(config.audit_failure_mode, expected);
        }
    }

    #[test]
    fn unknown_audit_mode_is_rejected() {
        let error = toml::from_str::<ReadinessConfig>("audit_failure_mode = \"permanent\"")
            .expect_err("未知模式必须拒绝加载");
        // 错误只含枚举闭集，不回显整个 TOML 文档。
        assert!(error.message().contains("expected one of"), "{}", error.message());
    }
}
