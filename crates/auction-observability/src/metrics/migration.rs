//! 迁移 readiness 的闭集分支与影子计数；版本号只允许进入日志。

use prometheus::{IntCounterVec, Opts, Registry};

use super::types::MetricsError;

/// 一轮数据库迁移就绪检查的固定结果类别。
#[derive(Debug, Clone, Copy)]
pub enum MigrationReadinessBranch {
    Current,
    Behind,
    Ahead,
    CompatibleAhead,
    IncompatibleAhead,
    InvalidHistory,
    QueryFailed,
}

impl MigrationReadinessBranch {
    /// 返回不含迁移版本或数据库身份的固定 label。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Behind => "behind",
            Self::Ahead => "ahead",
            Self::CompatibleAhead => "compatible_ahead",
            Self::IncompatibleAhead => "incompatible_ahead",
            Self::InvalidHistory => "invalid_history",
            Self::QueryFailed => "query_failed",
        }
    }
}

/// 可克隆的迁移就绪记录器；默认关闭，不影响判定或建立后台任务。
#[derive(Debug, Clone, Default)]
pub struct MigrationReadinessRecorder {
    checks: Option<IntCounterVec>,
    shadow: Option<IntCounterVec>,
}

impl MigrationReadinessRecorder {
    /// 记录实际底层检查；HTTP 命中缓存或合并等待者不重复累计。
    pub fn observe(&self, branch: MigrationReadinessBranch, shadow_mismatch: bool) {
        if let Some(checks) = &self.checks {
            checks.with_label_values(&[branch.label()]).inc();
        }
        if shadow_mismatch && let Some(shadow) = &self.shadow {
            shadow.with_label_values(&[branch.label()]).inc();
        }
    }
}

pub(super) fn build(registry: &Registry) -> Result<MigrationReadinessRecorder, MetricsError> {
    let checks = IntCounterVec::new(
        Opts::new(
            "auction_readiness_migration_checks_total",
            "Total underlying migration readiness checks by fixed branch.",
        ),
        &["branch"],
    )
    .map_err(MetricsError::Registry)?;
    // 暂沿用 W0.14 的 readiness 影子命名与 class 标签，W0.15 落地后由共享框架统一。
    let shadow = IntCounterVec::new(
        Opts::new(
            "auction_readiness_migration_shadow_mismatch_total",
            "Total shadow migration readiness decisions differing from strict history.",
        ),
        &["class"],
    )
    .map_err(MetricsError::Registry)?;
    for branch in [
        MigrationReadinessBranch::Current,
        MigrationReadinessBranch::Behind,
        MigrationReadinessBranch::Ahead,
        MigrationReadinessBranch::CompatibleAhead,
        MigrationReadinessBranch::IncompatibleAhead,
        MigrationReadinessBranch::InvalidHistory,
        MigrationReadinessBranch::QueryFailed,
    ] {
        checks.with_label_values(&[branch.label()]);
        shadow.with_label_values(&[branch.label()]);
    }
    registry.register(Box::new(checks.clone())).map_err(MetricsError::Registry)?;
    registry.register(Box::new(shadow.clone())).map_err(MetricsError::Registry)?;
    Ok(MigrationReadinessRecorder { checks: Some(checks), shadow: Some(shadow) })
}
