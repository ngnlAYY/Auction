//! 按节奏补扫挂单，启用期间回收失效解析租约。

use std::time::Duration;

use auction_application::consignment::{ListingMaintenanceService, LookupWorkStore};
use auction_observability::WorkerResult;
use tokio::sync::watch;
use tokio::time::Instant;

use super::ConsignmentLoop;
use super::{error_code, observe_iteration, stopped};
use crate::worker::runtime::wait_until_or_shutdown;

const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

pub(super) async fn run(
    service: &ListingMaintenanceService,
    work: Option<&dyn LookupWorkStore>,
    mut drain: watch::Receiver<bool>,
    observe: &(impl Fn(ConsignmentLoop, WorkerResult, Duration) + Sync),
) {
    while !stopped(&drain) {
        if let Some(work) = work {
            // 不变量 5：回收只处理持久租约，没有出站请求；外部暂停时仍可运行。
            // 停用后的旧工作由自动清退终结，避免在关闭代次重排旧工作。
            if let Err(error) = work.reap_expired().await {
                tracing::warn!(code = error_code(error), "寄售解析租约回收失败");
            }
        }
        while !stopped(&drain) {
            let started_at = Instant::now();
            let (result, has_more) = match service.sweep_page().await {
                Ok(page) => (WorkerResult::Success, page.has_more),
                Err(error) => {
                    // 不变量 5、8：本页已提交的单保留，持久游标后续回绕时重查失败项。
                    tracing::warn!(code = error_code(error), "寄售维护本页未全部完成");
                    (WorkerResult::Error, false)
                }
            };
            observe_iteration(observe, ConsignmentLoop::Maintenance, result, started_at);
            if !has_more {
                break;
            }
            tokio::task::yield_now().await;
        }
        if wait_until_or_shutdown(&mut drain, Instant::now() + SWEEP_INTERVAL).await {
            return;
        }
    }
}
