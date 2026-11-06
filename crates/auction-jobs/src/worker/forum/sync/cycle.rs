//! 领取循环与单项超时边界：只领取即将执行的一条，超时按实际写入进度决定重试或对账。

use std::sync::Arc;

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{ApplicationError, Clock};
use auction_application::forum::{ForumSyncClaim, ForumWriteProgress};
use chrono::Duration;

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError};

use super::claim::process_claim_inner;
use super::failure::{record_failure, requires_reconciliation, scoped_failure_code};
use super::runtime::{ForumSyncRuntime, MAX_BATCH_SIZE};

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumSyncRuntime,
    lease_duration: std::time::Duration,
    hot: Option<auction_application::admin::WorkerForumHotConfig>,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let lease_duration = Duration::from_std(lease_duration)
        .map_err(|_| WorkerError::InvalidForumSyncConfiguration)
        .inspect_err(|error| {
            diagnostics::unpersisted_error(
                "forum_sync",
                "convert_lease_duration",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
        })?;
    // 热快照只替换批次与单项预算；与启动租约冲突时退回启动值，不能让 claim 越过租约。
    let (batch_size, item_timeout) = hot
        .filter(|hot| {
            let item = std::time::Duration::from_secs(hot.item_timeout_seconds);
            hot.batch_size > 0
                && hot.batch_size <= MAX_BATCH_SIZE
                && !item.is_zero()
                && lease_duration.to_std().is_ok_and(|lease| item < lease)
        })
        .map(|hot| (hot.batch_size, std::time::Duration::from_secs(hot.item_timeout_seconds)))
        .unwrap_or((runtime.batch_size, runtime.item_timeout));
    let mut processed = 0_u16;
    let mut first_error = None;
    while processed < batch_size && !worker.control.is_draining() {
        let available_worlds =
            [auction_domain::SettlementWorld::Main, auction_domain::SettlementWorld::Isekai]
                .into_iter()
                .filter(|world| {
                    runtime.credential_availability.has_available_forum_credential(*world)
                })
                .collect::<Vec<_>>();
        if available_worlds.is_empty() {
            diagnostics::unpersisted_error(
                "forum_sync",
                "require_credential",
                SafeErrorKind::CAPABILITY_UNAVAILABLE,
                DiagnosticContext::default(),
            );
            first_error.get_or_insert(WorkerError::ForumTopicProcessingFailed);
        }
        // 领取与远端处理共用领取前截止，数据库等待不能重新获得完整处理预算。
        let item_deadline = tokio::time::Instant::now()
            .checked_add(item_timeout)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "check_item_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let now = worker.clock.now();
        let recovery_before = *runtime.startup_recovery_before.get_or_init(|| now);
        let startup_recovery_before = (!available_worlds.is_empty()).then_some(recovery_before);
        let lease_until = now
            .checked_add_signed(lease_duration)
            .ok_or(WorkerError::InvalidForumSyncConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "check_lease_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        // 只领取即将执行的一条；空世界集合仍初始化 live 投影，但不占租约或发起外呼。
        let claims = worker
            .write_lifecycle
            .background(tokio::time::timeout_at(
                item_deadline,
                runtime.store.claim_due(
                    worker.worker_id.clone(),
                    now,
                    lease_until,
                    startup_recovery_before,
                    available_worlds,
                    1,
                ),
            ))
            .await;
        let Some(claims) = claims else {
            break;
        };
        let mut claims = claims
            .map_err(|timeout| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "claim_topic_timeout",
                    diagnostics::timeout_kind(&timeout),
                    DiagnosticContext::default(),
                );
                let _elapsed = timeout;
                WorkerError::ForumTopicProcessingFailed
            })?
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "claim_topic",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let Some(claim) = claims.pop() else {
            break;
        };
        // 领取回执跨过 drain 时尚未外呼；保留原 claim，不能在停机阶段开始发布主题。
        if worker.control.is_draining() {
            break;
        }
        processed += 1;
        let diagnostic_auction_id = claim.auction_id;
        if let Err(error) = process_claim(
            runtime,
            Arc::clone(&worker.clock),
            claim,
            item_deadline,
            &worker.write_lifecycle,
        )
        .await
        .inspect_err(|error| {
            if !matches!(error, WorkerError::ForumTopicProcessingFailed) {
                diagnostics::unpersisted_error(
                    "forum_sync",
                    "process_claim",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        auction_id: diagnostic_id(&diagnostic_auction_id),
                        ..Default::default()
                    },
                );
            }
        }) && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[tracing::instrument(
    skip_all,
    fields(
        auction_id = %claim.auction_id,
    )
)]
async fn process_claim<C: Clock + 'static>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    item_deadline: tokio::time::Instant,
    lifecycle: &auction_application::operations::ExternalWriteLifecycle,
) -> Result<(), WorkerError> {
    let activity = lifecycle.activity(auction_application::operations::CriticalWriteKind::Forum);
    let progress = ForumWriteProgress::with_activity(activity.clone());
    activity
        .run(process_claim_attempt(runtime, clock, claim, item_deadline, &progress))
        .await
        .unwrap_or(Ok(()))
}

async fn process_claim_attempt<C>(
    runtime: &ForumSyncRuntime,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    item_deadline: tokio::time::Instant,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let store = Arc::clone(&runtime.store);
    let failure_store = Arc::clone(&store);
    let failure_clock = Arc::clone(&clock);
    let failure_claim = claim.clone();
    let reconciliation_only = requires_reconciliation(&claim);
    let result = if tokio::time::Instant::now() >= item_deadline {
        // 领取响应已到截止时，不首次 poll 内层任务；仍记录已知未开始外写的安全失败。
        None
    } else {
        tokio::time::timeout_at(
            item_deadline,
            process_claim_inner(runtime, clock, claim, write_progress),
        )
        .await
        .ok()
    };
    match result {
        Some(result) => result,
        None => {
            // 只有真实 POST 边界或历史未知结果才保留只读门禁；纯预读/渲染超时可普通重试。
            let failure_code = if write_progress.has_unconfirmed_write() {
                "write_unknown"
            } else {
                "topic_processing_timeout"
            };
            record_failure(
                write_progress,
                failure_store,
                failure_clock,
                failure_claim,
                scoped_failure_code(reconciliation_only, failure_code),
                ApplicationError::Repository(auction_application::core::RepositoryError::Failed(
                    "论坛同步单项处理超时，按实际写入进度保留重试边界".to_owned(),
                )),
            )
            .await?;
            Err(WorkerError::ForumTopicProcessingFailed)
        }
    }
}
