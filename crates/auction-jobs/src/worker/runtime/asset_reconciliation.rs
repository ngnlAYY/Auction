//! 资产对账周期运行时。
//!
//! 外部 Armory 与余额读取属于最低优先级的只读观察：周期固定、无重入、失败只记录
//! 未持久化错误，不改变竞价、结算、义务、余额或装备。报告本身由存储 owner 追加为
//! 审计事实，运行时不解释差异。

use std::sync::{Arc, Mutex};

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::reconciliation::AssetReconciliationPort;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::worker::diagnostics;

/// 两轮资产对账之间的固定间隔。
///
/// 对账每轮最多读取两个 world × 七个 Armory 分类，属于昂贵的外部只读观察；间隔不进入
/// 运行配置，避免运维误把偿付能力监控关成分钟级扫描。失败后的下一轮仍按同一间隔。
pub const ASSET_RECONCILIATION_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(6 * 3_600);

/// 单轮对账的总预算；覆盖两个 world 的七类库存与两份报告追加。
pub const ASSET_RECONCILIATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// 按独立周期触发一次完整对账的运行时。
///
/// `next_run_at` 在开始外部读取前推进，避免上一轮较慢时并发堆积多个对账。
pub struct AssetReconciliationRuntime {
    reconciler: Arc<dyn AssetReconciliationPort>,
    interval: std::time::Duration,
    timeout: std::time::Duration,
    next_run_at: Mutex<Option<DateTime<Utc>>>,
}

impl AssetReconciliationRuntime {
    /// 建立带固定周期与单轮预算的运行时。
    pub fn new(
        reconciler: Arc<dyn AssetReconciliationPort>,
        interval: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Result<Self, &'static str> {
        if interval.is_zero() || timeout.is_zero() {
            return Err("资产对账周期与预算必须大于零");
        }
        Ok(Self { reconciler, interval, timeout, next_run_at: Mutex::new(None) })
    }

    /// 到期时执行一次对账；首次调用立即执行。
    pub async fn run_once(&self, now: DateTime<Utc>) -> Result<bool, ApplicationError> {
        let interval =
            ChronoDuration::from_std(self.interval).map_err(|_: chrono::OutOfRangeError| {
                ApplicationError::Repository(RepositoryError::Failed(
                    "资产对账周期超出时间范围".to_owned(),
                ))
            })?;
        let next_run_at = now.checked_add_signed(interval).ok_or_else(|| {
            ApplicationError::Repository(RepositoryError::Failed(
                "资产对账下一次运行时间溢出".to_owned(),
            ))
        })?;
        let should_run = {
            let mut scheduled = self.next_run_at.lock().map_err(|error| {
                diagnostics::warn(
                    "asset_reconciliation",
                    "lock_schedule",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                ApplicationError::Repository(RepositoryError::Failed(
                    "资产对账调度状态锁损坏".to_owned(),
                ))
            })?;
            if scheduled.is_some_and(|value| now < value) {
                false
            } else {
                *scheduled = Some(next_run_at);
                true
            }
        };
        if !should_run {
            return Ok(false);
        }
        tokio::time::timeout(self.timeout, self.reconciler.reconcile(now)).await.map_err(
            |_: tokio::time::error::Elapsed| {
                ApplicationError::Repository(RepositoryError::Failed(
                    "资产对账超出单轮预算".to_owned(),
                ))
            },
        )??;
        Ok(true)
    }
}
