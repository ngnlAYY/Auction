//! 审计 SQL 失败的进程级饱和计数、就绪投影与影子观测。
//! 仅附加安全信号，原错误、同事务审计、回滚和事务 owner 的重试规则全部保留。
//!
//! 审计写入失败的分类表；`AuditWriteResult` 按此顺序匹配 SQLSTATE 与驱动变体，
//! 分类只用于诊断与就绪门禁，不改变原错误返回。
//!
//! | 原始错误（按表中顺序匹配） | class / kind |
//! | --- | --- |
//! | `40001` / `40P01` | transient / `serialization_conflict`、`deadlock` |
//! | `57014` / `55P03` | transient / `query_canceled`、`lock_unavailable` |
//! | 整个 `08` 类（含 `08007`、`08P01`） | transient / `connection_exception`；只放行健康门禁，不允许自动重放结果未知事务 |
//! | sqlx `PoolTimedOut` / `Io` | transient / `pool_timed_out`、`connection_io` |
//! | `53300`（决策 71） | transient / `connection_capacity` |
//! | `57P01`（决策 71） | transient / `server_shutdown` |
//! | `57P02`（决策 71） | transient / `server_crash` |
//! | `57P03`（决策 71） | transient / `server_starting` |
//! | `23` 类 | permanent / `constraint_violation`，包括 `23505`，不扩大冲突重试 |
//! | `55000`、`09` / `27` / `P0` 类、`39P01/39P02/39P03` | permanent / `trigger_or_routine_rejected` |
//! | `42501` / `28` 类 | permanent / `permission_denied`、`authentication_rejected` |
//! | 其他 `42` 类、`3D` / `3F` 类 | permanent / `schema_or_sql_mismatch` |
//! | `21` / `22` / `44` 类 | permanent / `invalid_audit_data` |
//! | sqlx `Configuration/ConfigFile`、`InvalidArgument` | permanent / `driver_configuration`、`driver_invalid_argument` |
//! | sqlx `RowNotFound/TypeNotFound/ColumnIndexOutOfBounds/ColumnNotFound/ColumnDecode/Encode/Decode` | permanent / `driver_data` |
//! | sqlx `PoolClosed/WorkerCrashed/InvalidSavePointStatement` | permanent / `driver_state` |
//! | sqlx `Tls` / `Protocol` | unknown / `driver_tls`、`driver_protocol` |
//! | 未列出的、缺失或非法 SQLSTATE；其他及未来 sqlx 变体（含 `BeginFailed/AnyDriverError/Migrate`） | unknown / `unknown_sqlstate`、`unknown_driver`；依决策 71，`57P05`、`25P03/25P04` 仍按未知粘滞阻断 |

mod classification;

use std::sync::{
    Arc, OnceLock,
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
/// 共享影子框架的进程级记录器；审计失败计数本身就是进程级状态，因此同样只安装一次。
static SHADOW_RECORDER: OnceLock<Arc<dyn auction_application::shadow_run::ShadowRecorder>> =
    OnceLock::new();

/// 由 Public/Admin/Worker 装配根各安装一次共享影子记录器；重复安装保留第一个，测试与 CLI
/// 不安装时只保留本模块原有计数。
pub fn install_audit_shadow_recorder(
    recorder: Arc<dyn auction_application::shadow_run::ShadowRecorder>,
) {
    let _ = SHADOW_RECORDER.set(recorder);
}

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
            // 决策 72：每次失败按类别记一条共享影子样本——旧口径任一失败阻断，分类口径只有
            // permanent/unknown 阻断；瞬时类是预期差异。样本只用于切换观察，不改变门禁。
            if let Some(recorder) = SHADOW_RECORDER.get() {
                auction_application::shadow_run::record_comparison(
                    recorder.as_ref(),
                    auction_application::shadow_run::ShadowOwner::AuditReadiness,
                    class.label(),
                    &true,
                    &(class != AuditFailureClass::Transient),
                );
            }
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
pub(in crate::database) fn failure_count(mode: AuditFailureMode) -> i64 {
    let (total, blocking) = counts();
    match mode {
        AuditFailureMode::Legacy | AuditFailureMode::Shadow => total,
        AuditFailureMode::Classified => blocking,
    }
}

/// 数据库依赖已正常时比对审计门禁；影子计数按实际检查次数累计，不按 HTTP 等待者累计。
pub(in crate::database) fn is_ready(mode: AuditFailureMode) -> bool {
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

pub(in crate::database) fn metrics_source(
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> Arc<dyn AuditMetricsSource> {
    Arc::new(ProcessAuditMetricsSource { switches })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[derive(Default)]
    struct Samples(std::sync::Mutex<Vec<auction_application::shadow_run::ShadowSample>>);

    impl auction_application::shadow_run::ShadowRecorder for Samples {
        fn record(&self, sample: auction_application::shadow_run::ShadowSample) {
            self.0.lock().expect("样本锁").push(sample);
        }
    }

    /// 决策 72：瞬时审计失败在共享框架记为 `transient` 分支的预期差异，旧门禁不变。
    #[test]
    fn transient_audit_failure_records_expected_shared_shadow_mismatch() {
        let samples = Arc::new(Samples::default());
        install_audit_shadow_recorder(samples.clone());
        let result: Result<(), sqlx::Error> = Err(sqlx::Error::PoolTimedOut);
        let _ = result.record_audit_failure();
        let recorded = samples.0.lock().expect("样本锁");
        assert!(recorded.iter().any(|sample| {
            sample.owner == auction_application::shadow_run::ShadowOwner::AuditReadiness
                && sample.branch == "transient"
                && sample.outcome == auction_application::shadow_run::ShadowOutcome::Mismatch
        }));
    }
}
