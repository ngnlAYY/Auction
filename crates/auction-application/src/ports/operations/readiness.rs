//! 进程 readiness 的窄端口。
//! HTTP surface 只需要知道当前进程是否可以接收业务流量，不应直接依赖具体数据库或连接池。
//! 具体适配器负责执行只读的依赖检查，并将失败转换为不泄露下游细节的应用错误。

use super::ApplicationFuture;

/// 审计失败的进程就绪策略；TOML 是基线，可经数据库开关选择，不改变事务错误或重试规则。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditFailureMode {
    /// 所有审计失败均阻断，不计算影子判定。
    Legacy,
    /// 沿用旧判定，同时观察分类后的预期差异。
    Shadow,
    /// 仅不可重试及未知风险失败阻断；默认模式。
    #[default]
    Classified,
}

/// 迁移兼容就绪策略；只选择 schema 校验路径，不授权迁移、恢复或数据写入。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationCompatibilityMode {
    /// 数据库迁移历史必须恰好等于二进制内嵌集合。
    Strict,
    /// 实际判定仍严格，同时观测兼容超前的预期差异。
    #[default]
    Shadow,
    /// 已知前缀严格匹配且未来后缀全部明确兼容时允许就绪。
    Compatible,
}

/// 检查进程是否满足旧版 `/health/ready` 的最小依赖条件。
pub trait ProcessReadinessPort: Send + Sync {
    /// 执行一次只读 readiness 检查；失败时调用方只应返回有限状态码和稳定错误码。
    fn check(&self) -> ApplicationFuture<'_, ()>;
}

#[cfg(test)]
mod tests {
    use super::AuditFailureMode;

    #[test]
    fn default_audit_failure_mode_is_classified() {
        // 用户决定默认基线为 classified；legacy 与 shadow 仍可显式选择。
        assert_eq!(AuditFailureMode::default(), AuditFailureMode::Classified);
    }

    #[test]
    fn all_audit_failure_modes_stay_deserializable() {
        for (value, expected) in [
            ("\"legacy\"", AuditFailureMode::Legacy),
            ("\"shadow\"", AuditFailureMode::Shadow),
            ("\"classified\"", AuditFailureMode::Classified),
        ] {
            let mode: AuditFailureMode = serde_json::from_str(value).expect("已知模式必须接受");
            assert_eq!(mode, expected);
        }
        assert!(serde_json::from_str::<AuditFailureMode>("\"permanent\"").is_err());
    }
}
