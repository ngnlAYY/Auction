use std::sync::{Arc, Mutex};

use auction_application::{ApplicationError, RecoveryHealthRefreshPort, RepositoryError};
use chrono::{DateTime, Duration as ChronoDuration, Utc};

/// 按独立周期触发 recovery health 刷新的 Worker 运行时。
/// Worker 主轮询可以比 health 周期更频繁；`next_refresh_at` 在开始远端读取前推进，避免
/// 外部服务慢或失败时并发堆积多个 health 刷新。
pub struct RecoveryHealthRuntime {
    refresher: Arc<dyn RecoveryHealthRefreshPort>,
    refresh_interval: std::time::Duration,
    next_refresh_at: Mutex<Option<DateTime<Utc>>>,
}

impl RecoveryHealthRuntime {
    /// 建立带有界刷新周期的 health 运行时。
    pub fn new(
        refresher: Arc<dyn RecoveryHealthRefreshPort>,
        refresh_interval: std::time::Duration,
    ) -> Result<Self, &'static str> {
        if refresh_interval.is_zero() {
            return Err("recovery health 刷新周期必须大于零");
        }
        Ok(Self { refresher, refresh_interval, next_refresh_at: Mutex::new(None) })
    }

    /// 到期时执行一次刷新；首次调用立即执行。
    pub async fn run_once(&self, now: DateTime<Utc>) -> Result<bool, ApplicationError> {
        let interval = ChronoDuration::from_std(self.refresh_interval).map_err(|_| {
            ApplicationError::Repository(RepositoryError::Failed(
                "recovery health 刷新周期超出时间范围".to_owned(),
            ))
        })?;
        let next_refresh_at = now.checked_add_signed(interval).ok_or_else(|| {
            ApplicationError::Repository(RepositoryError::Failed(
                "recovery health 下一次刷新时间溢出".to_owned(),
            ))
        })?;
        let should_refresh = {
            let mut scheduled = self.next_refresh_at.lock().map_err(|_| {
                ApplicationError::Repository(RepositoryError::Failed(
                    "recovery health 刷新状态锁损坏".to_owned(),
                ))
            })?;
            if scheduled.is_some_and(|value| now < value) {
                false
            } else {
                *scheduled = Some(next_refresh_at);
                true
            }
        };
        if !should_refresh {
            return Ok(false);
        }
        self.refresher.refresh(now).await?;
        Ok(true)
    }
}
