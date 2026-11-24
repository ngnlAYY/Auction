//! 按数据库时刻续发寄售准入就绪心跳。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ListingWorkerReadiness, MailEvidenceReadinessRecord, MailEvidenceReadinessStore,
    MailEvidenceSessionDraft, MailReadinessPolicy,
};
use auction_observability::WorkerResult;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use super::ConsignmentLookupRuntime;
use super::ConsignmentLoop;
use super::{error_code, observe_iteration, stopped, wait_for_stop};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// 一个已装配收件读取器的世界的就绪续期输入。
///
/// 只有装配根确认该世界的读取器已就绪且 `policy.detail_ready` 为真时才注册本值；
/// 没有注册的世界不会写 `mail_evidence_sessions` 或 `consignment_mail_readiness`。
pub struct MailEvidenceReadinessRuntime {
    /// 就绪写端。
    pub store: Arc<dyn MailEvidenceReadinessStore>,
    /// 本次运行期固定的证据会话绑定。
    pub session: MailEvidenceSessionDraft,
    /// 世界能力与 policy 摘要。
    pub policy: MailReadinessPolicy,
    /// 每次续期的有效秒数。
    pub valid_for_seconds: u32,
}

pub(super) async fn run(
    runtime: &ConsignmentLookupRuntime,
    mail_readiness: &[MailEvidenceReadinessRuntime],
    epoch: u64,
    drain: watch::Receiver<bool>,
    observe: &(impl Fn(ConsignmentLoop, WorkerResult, Duration) + Sync),
) {
    let mut ticks = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    while !stopped(&drain) {
        let started_at = tokio::select! {
            biased;
            () = wait_for_stop(drain.clone()) => return,
            _ = ticks.tick() => Instant::now(),
        };
        let dependencies = tokio::select! {
            biased;
            () = wait_for_stop(drain.clone()) => return,
            result = runtime.readiness.dependencies() => result,
        };
        if stopped(&drain) {
            return;
        }
        // 不变量 5、6：任一能力未就绪时保留原期限，续期的时钟、代次和写入由数据库裁定。
        let result = match dependencies {
            Ok(deps)
                if deps.database
                    && deps.schema
                    && deps.opener
                    && deps.current_keyring
                    && deps.lookup =>
            {
                runtime.work.record_worker_readiness(epoch, ListingWorkerReadiness::Ready).await
            }
            Ok(_) => {
                tracing::debug!(code = "dependencies_unavailable", "寄售本轮不续就绪心跳");
                observe_iteration(
                    observe,
                    ConsignmentLoop::Readiness,
                    WorkerResult::Error,
                    started_at,
                );
                continue;
            }
            Err(error) => Err(error),
        };
        let worker_ready = result.is_ok();
        let mut outcome = match result {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::warn!(code = error_code(error), "寄售就绪心跳未确认续期");
                WorkerResult::Error
            }
        };
        if worker_ready {
            for readiness in mail_readiness {
                // 读取器未签收详情能力时保持门控关闭，不写任何有效就绪。
                if !readiness.policy.detail_ready {
                    continue;
                }
                let record = MailEvidenceReadinessRecord {
                    session: readiness.session.clone(),
                    policy: readiness.policy.clone(),
                    service_epoch: epoch,
                    valid_for_seconds: readiness.valid_for_seconds,
                };
                if let Err(error) = readiness.store.record_mail_readiness(record).await {
                    tracing::warn!(
                        code = error_code(error),
                        world_id = readiness.session.world.as_str(),
                        "寄售收件就绪未确认续期"
                    );
                    outcome = WorkerResult::Error;
                }
            }
        }
        observe_iteration(observe, ConsignmentLoop::Readiness, outcome, started_at);
    }
}
