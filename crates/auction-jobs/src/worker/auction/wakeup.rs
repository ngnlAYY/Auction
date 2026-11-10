//! Auction 主循环的到期唤醒提示：在常规轮询间隔内按最早的未来到期时刻提前醒来。

use std::time::Duration;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::worker::diagnostics;
use crate::worker::runtime::PersistentWorker;

/// 三个只读提示查询的总预算；超时或失败时沿用常规轮询，不阻塞主循环或停机。
const HINT_TIMEOUT: Duration = Duration::from_secs(2);
/// 晚于到期时刻少量醒来，吸收数据库与进程时钟的取整差，避免醒得过早而落空一轮。
const DUE_MARGIN: Duration = Duration::from_millis(50);
/// 到期驱动唤醒的下限，防止连续的近零间隔空转。
const MIN_DUE_WAIT: Duration = Duration::from_millis(200);

/// 返回距生命周期与慢速代理最早未来到期时刻的等待时长；调用方再与常规轮询取较小值。
/// 结算使用独立的 [`settlement_due_wait`]，两个循环各自锚定自己的最早到期。
/// 提示只决定何时醒来，领取与执行仍由各 owner 的锁内谓词决定；已过期仍未领取的事项（失败
/// 退避、租约或批次上限）不缩短等待，持续失败时保持原轮询节奏。
pub(super) async fn next_due_wait<C>(worker: &PersistentWorker<C>) -> Option<Duration>
where
    C: Clock + 'static,
{
    match tokio::time::timeout(HINT_TIMEOUT, earliest_due_wait(worker)).await {
        Ok(wait) => wait.map(|wait| wait.saturating_add(DUE_MARGIN).max(MIN_DUE_WAIT)),
        Err(error) => {
            diagnostics::warn(
                "auction_wakeup",
                "read_hints_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );

            None
        }
    }
}

async fn earliest_due_wait<C>(worker: &PersistentWorker<C>) -> Option<Duration>
where
    C: Clock + 'static,
{
    // 每类提示都以其领取所用的时钟计算：生命周期与慢速代理用数据库时钟，结算沿用进程时钟。
    // 提示须锚定读取时刻；后续查询和网络等待不能在最终 sleep 时重新获得一遍等待时长。
    let database_observed_at = Instant::now();
    let database_now = match worker.schedule.database_now().await {
        Ok(now) => Some(now),
        Err(error) => {
            diagnostics::debug(
                "auction_wakeup",
                "read_database_clock",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            None
        }
    };
    let mut wait = None;
    if let Some(now) = database_now {
        let lifecycle = hint(worker.schedule.next_claimable_at(now).await, "read_lifecycle_hint");
        wait = shorter(wait, until(now, database_observed_at, lifecycle));
        if let Some(reader) = &worker.slow_proxy_due {
            let slow_proxy = hint(reader.next_slow_proxy_due_at(now).await, "read_slow_proxy_hint");
            wait = shorter(wait, until(now, database_observed_at, slow_proxy));
        }
    }
    wait.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// 独立结算循环的最早到期等待；未注入结算端口时没有提示。
/// 结算提示沿用进程时钟（与领取时的 lease 计算一致），生命周期与慢速代理用数据库时钟。
pub(super) async fn settlement_due_wait<C>(worker: &PersistentWorker<C>) -> Option<Duration>
where
    C: Clock + 'static,
{
    let store = worker.settlement_store.as_ref()?;
    match tokio::time::timeout(HINT_TIMEOUT, async {
        let observed_at = Instant::now();
        let now = worker.clock.now();
        until(now, observed_at, hint(store.next_claimable_at(now).await, "read_settlement_hint"))
    })
    .await
    {
        Ok(wait) => wait.map(|deadline| deadline.saturating_duration_since(Instant::now())),
        Err(error) => {
            let _ = error;
            None
        }
    }
}

fn hint(
    result: Result<Option<DateTime<Utc>>, auction_application::core::ApplicationError>,
    phase: &'static str,
) -> Option<DateTime<Utc>> {
    result.unwrap_or_else(|error| {
        diagnostics::debug(
            "auction_wakeup",
            phase,
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );
        None
    })
}

/// 只接受严格晚于参考时刻的到期；无法表示为标准时长的值按无提示处理。
fn until(now: DateTime<Utc>, observed_at: Instant, due: Option<DateTime<Utc>>) -> Option<Instant> {
    let delay = due.filter(|due| *due > now).and_then(|due| (due - now).to_std().ok())?;
    observed_at.checked_add(delay)
}

fn shorter(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}
