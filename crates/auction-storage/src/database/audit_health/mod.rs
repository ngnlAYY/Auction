//! 审计 SQL 失败的进程级饱和计数、就绪投影与影子观测。
//! 仅附加安全信号，原错误、同事务审计、回滚和事务 owner 的重试规则全部保留。

mod classification;

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use auction_application::operations::AuditFailureMode;
use auction_observability::{
    AuditFailureClass, AuditFailureKind, AuditFailureObservation, AuditMetricsSnapshot,
    AuditMetricsSource, AuditSqlstateClass,
};

const SQLSTATE_CLASSES: usize = AuditSqlstateClass::ALL.len();
static FAILURES: [AtomicI64; AuditFailureKind::ALL.len() * SQLSTATE_CLASSES] =
    [const { AtomicI64::new(0) }; AuditFailureKind::ALL.len() * SQLSTATE_CLASSES];
static CLASS_FAILURES: [AtomicI64; 3] = [const { AtomicI64::new(0) }; 3];
static SHADOW_MISMATCHES: [AtomicI64; 3] = [const { AtomicI64::new(0) }; 3];

fn increment(counter: &AtomicI64) {
    // MSRV 1.94.1 尚未稳定 try_update；饱和计数不能回绕为负或被成功写入清零。
    #[allow(deprecated)]
    let _ = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| Some(count.saturating_add(1)));
}

/// 为原始审计 SQL 的结果附加分类；不得吞错或扩大事务自动重试范围。
pub(crate) trait AuditWriteResult<T> {
    fn record_audit_failure(self) -> Self;
}

impl<T> AuditWriteResult<T> for Result<T, sqlx::Error> {
    fn record_audit_failure(self) -> Self {
        if let Err(error) = &self {
            let (kind, sqlstate_class) = classification::classify(error);
            let class = kind.class();
            increment(&CLASS_FAILURES[class as usize]);
            increment(&FAILURES[kind as usize * SQLSTATE_CLASSES + sqlstate_class as usize]);
            let blocks_classified_readiness = class != AuditFailureClass::Transient;
            if blocks_classified_readiness {
                tracing::error!(target: "auction::storage::audit_health", class = class.label(), sqlstate_class = sqlstate_class.label(), kind = kind.label(), blocks_classified_readiness, "审计写入失败");
            } else {
                tracing::warn!(target: "auction::storage::audit_health", class = class.label(), sqlstate_class = sqlstate_class.label(), kind = kind.label(), blocks_classified_readiness, "审计写入瞬时失败");
            }
        }
        self
    }
}

fn counts() -> (i64, i64) {
    // 计数单调递增，未观察到的并发失败由下一次探针收敛。
    let permanent = CLASS_FAILURES[AuditFailureClass::Permanent as usize].load(Ordering::Relaxed);
    let unknown = CLASS_FAILURES[AuditFailureClass::Unknown as usize].load(Ordering::Relaxed);
    let blocking = permanent.saturating_add(unknown);
    let transient = CLASS_FAILURES[AuditFailureClass::Transient as usize].load(Ordering::Relaxed);
    (blocking.saturating_add(transient), blocking)
}

/// 心跳与 readiness 共用模式投影；unknown 是未知风险处置，不代表已证明不可重试。
pub(super) fn failure_count(mode: AuditFailureMode) -> i64 {
    let (total, blocking) = counts();
    match mode {
        AuditFailureMode::Legacy | AuditFailureMode::Shadow => total,
        AuditFailureMode::Classified => blocking,
    }
}

/// 数据库依赖已正常时比对审计门禁；影子计数按实际检查次数累计，不按 HTTP 等待者累计。
pub(super) fn is_ready(mode: AuditFailureMode) -> bool {
    let (total, blocking) = counts();
    if mode == AuditFailureMode::Shadow && total > 0 && blocking == 0 {
        increment(&SHADOW_MISMATCHES[AuditFailureClass::Transient as usize]);
        tracing::warn!(target: "auction::storage::audit_health", class = "transient", sqlstate_class = "none", kind = "shadow_readiness_mismatch", blocks_classified_readiness = false, "审计就绪影子判定出现预期差异");
    }
    gate_ready(mode, total, blocking)
}

/// 门禁口径的纯投影：shadow 按决策 72 保留旧判定，classified 只看 permanent + unknown。
const fn gate_ready(mode: AuditFailureMode, total: i64, blocking: i64) -> bool {
    match mode {
        AuditFailureMode::Legacy | AuditFailureMode::Shadow => total == 0,
        AuditFailureMode::Classified => blocking == 0,
    }
}

struct ProcessAuditMetricsSource {
    switches: auction_application::runtime_switches::RuntimeSwitches,
}

impl AuditMetricsSource for ProcessAuditMetricsSource {
    fn snapshot(&self) -> AuditMetricsSnapshot {
        let mut failures = Vec::new();
        for kind in AuditFailureKind::ALL {
            for sqlstate_class in AuditSqlstateClass::ALL {
                let count = FAILURES[kind as usize * SQLSTATE_CLASSES + sqlstate_class as usize]
                    .load(Ordering::Relaxed);
                if count > 0 {
                    failures.push(AuditFailureObservation { kind, sqlstate_class, count });
                }
            }
        }
        AuditMetricsSnapshot {
            failures,
            readiness_blocked: failure_count(self.switches.readiness_policy().audit_failure_mode)
                > 0,
            shadow_mismatches: std::array::from_fn(|index| {
                SHADOW_MISMATCHES[index].load(Ordering::Relaxed)
            }),
        }
    }
}

pub(super) fn metrics_source(
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> Arc<dyn AuditMetricsSource> {
    Arc::new(ProcessAuditMetricsSource { switches })
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;

    #[test]
    fn classified_ignores_transient_failures_but_not_blocking_ones() {
        assert!(gate_ready(AuditFailureMode::Classified, 3, 0));
        assert!(!gate_ready(AuditFailureMode::Classified, 3, 1));
    }

    #[test]
    fn legacy_and_shadow_keep_the_old_any_failure_decision() {
        // 决策 72：shadow 只观察差异，不提前切换实际判定。
        assert!(!gate_ready(AuditFailureMode::Legacy, 1, 0));
        assert!(!gate_ready(AuditFailureMode::Shadow, 1, 0));
        assert!(gate_ready(AuditFailureMode::Shadow, 0, 0));
    }
}
