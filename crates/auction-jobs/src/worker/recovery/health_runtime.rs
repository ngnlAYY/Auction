//! Recovery health 周期刷新的 Worker 运行时。

use std::sync::{Arc, Mutex};

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::recovery::RecoveryHealthRefreshPort;
use auction_time::{TimeDelta, Timestamp};

use crate::worker::diagnostics;

/// 按独立周期触发 recovery health 刷新的 Worker 运行时；同一时刻只允许一次远端读取。
pub struct RecoveryHealthRuntime {
    refresher: Arc<dyn RecoveryHealthRefreshPort>,
    refresh_interval: std::time::Duration,
    state: Mutex<RefreshState>,
}

#[derive(Default)]
struct RefreshState {
    next_refresh_at: Option<Timestamp>,
    in_flight: bool,
}

struct RefreshInFlight<'a>(&'a Mutex<RefreshState>);

impl Drop for RefreshInFlight<'_> {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.in_flight = false;
    }
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
        Ok(Self { refresher, refresh_interval, state: Mutex::new(RefreshState::default()) })
    }

    /// 到期时执行一次刷新；首次调用立即执行。
    pub async fn run_once(&self, now: Timestamp) -> Result<bool, ApplicationError> {
        let interval = TimeDelta::from_std(self.refresh_interval).map_err(|_| {
            ApplicationError::Repository(RepositoryError::Failed(
                "recovery health 刷新周期超出时间范围".to_owned(),
            ))
        })?;
        let next_refresh_at = now.checked_add_signed(interval).ok_or_else(|| {
            ApplicationError::Repository(RepositoryError::Failed(
                "recovery health 下一次刷新时间溢出".to_owned(),
            ))
        })?;
        {
            let mut state = self.state.lock().map_err(|error| {
                diagnostics::warn(
                    "recovery_health",
                    "lock_schedule",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                ApplicationError::Repository(RepositoryError::Failed(
                    "recovery health 刷新状态锁损坏".to_owned(),
                ))
            })?;
            if state.in_flight || state.next_refresh_at.is_some_and(|value| now < value) {
                return Ok(false);
            }
            state.next_refresh_at = Some(next_refresh_at);
            state.in_flight = true;
        }
        let _in_flight = RefreshInFlight(&self.state);
        self.refresher.refresh(now).await?;
        Ok(true)
    }
}
