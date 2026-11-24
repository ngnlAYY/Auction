//! 按节奏补扫挂单，启用期间回收失效解析租约。

use std::time::Duration;

use auction_application::consignment::{ListingMaintenanceService, LookupWorkStore};
use auction_observability::WorkerResult;
use tokio::sync::watch;
use tokio::time::Instant;

use super::ConsignmentLoop;
use super::{error_code, stopped};
use crate::worker::runtime::wait_until_or_shutdown;

const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

pub(super) async fn run(
    service: &ListingMaintenanceService,
    work: Option<&dyn LookupWorkStore>,
    mut drain: watch::Receiver<bool>,
    observe: &(impl Fn(ConsignmentLoop, WorkerResult, Duration, u32) + Sync),
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
            let (result, has_more, failed) = match service.sweep_page().await {
                Ok(page) => {
                    // 不变量 5、8：按持久游标继续翻页，失败项由下一次完整扫描重查。
                    if page.failed > 0 {
                        tracing::warn!(
                            processed = page.processed,
                            failed = page.failed,
                            "寄售维护本页存在清退失败"
                        );
                    }
                    (WorkerResult::Success, page.has_more, page.failed)
                }
                Err(error) => {
                    tracing::warn!(code = error_code(error), "寄售维护候选页无法处理");
                    (WorkerResult::Error, false, 0)
                }
            };
            observe(ConsignmentLoop::Maintenance, result, started_at.elapsed(), failed);
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
