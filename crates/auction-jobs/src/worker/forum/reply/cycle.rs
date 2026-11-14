//! 论坛回帖一轮：领取、按主题处理与失败记录。

use std::collections::HashSet;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::forum::ForumWriteProgress;
use chrono::Duration;

use super::topic::process_topic;
use super::{ForumReplyRuntime, MAX_BATCH_SIZE};
use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError};

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumReplyRuntime,
    lease_duration: std::time::Duration,
    poll_interval: std::time::Duration,
    hot: Option<auction_application::admin::WorkerForumHotConfig>,
) -> Result<Option<std::time::Duration>, WorkerError>
where
    C: auction_application::core::Clock + 'static,
{
    let lease_duration = Duration::from_std(lease_duration)
        .map_err(|_| WorkerError::InvalidForumConfiguration)
        .inspect_err(|error| {
            diagnostics::unpersisted_error(
                "forum_reply",
                "convert_lease_duration",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
        })?;
    let poll_interval = Duration::from_std(poll_interval)
        .map_err(|_| WorkerError::InvalidForumConfiguration)
        .inspect_err(|error| {
            diagnostics::unpersisted_error(
                "forum_reply",
                "convert_poll_interval",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
        })?;
    // 热快照只替换批次与单项预算；与启动租约冲突时退回启动值。
    let (batch_size, item_timeout) = hot
        .filter(|hot| {
            let item = std::time::Duration::from_secs(hot.item_timeout_seconds);
            hot.batch_size > 0
                && hot.batch_size <= MAX_BATCH_SIZE
                && !item.is_zero()
                && lease_duration.to_std().is_ok_and(|lease| item < lease)
        })
        .map_or((runtime.batch_size, runtime.item_timeout), |hot| {
            (hot.batch_size, std::time::Duration::from_secs(hot.item_timeout_seconds))
        });
    let mut seen_auctions = HashSet::new();
    // Final 阶段本轮排定的最早下一次导入；循环据此提前唤醒，而不是等满常规轮询间隔。
    let mut earliest_final_scan: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut processed = 0_u16;
    let mut topic_failed = false;
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
                "forum_reply",
                "require_credential",
                SafeErrorKind::CAPABILITY_UNAVAILABLE,
                DiagnosticContext::default(),
            );
            topic_failed = true;
            break;
        }
        // 内容租约必须覆盖领取和远端编辑的总预算，领取响应不重置单项时限。
        let item_deadline = tokio::time::Instant::now()
            .checked_add(item_timeout)
            .ok_or(WorkerError::InvalidForumConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_reply",
                    "check_item_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let now = worker.clock.now();
        let lease_until = now
            .checked_add_signed(lease_duration)
            .ok_or(WorkerError::InvalidForumConfiguration)
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_reply",
                    "check_lease_deadline",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        // 每次只领取即将处理的一场拍卖，避免批量候选在等待前序主题时耗尽内容租约。
        let topics = worker
            .write_lifecycle
            .background(tokio::time::timeout_at(
                item_deadline,
                runtime.store.claim_published_topics(
                    worker.worker_id.clone(),
                    now,
                    lease_until,
                    available_worlds,
                    1,
                ),
            ))
            .await;
        let Some(topics) = topics else {
            break;
        };
        let mut topics = topics
            .map_err(|timeout| {
                diagnostics::unpersisted_error(
                    "forum_reply",
                    "claim_topic_timeout",
                    diagnostics::timeout_kind(&timeout),
                    DiagnosticContext::default(),
                );
                let _elapsed = timeout;
                WorkerError::ForumTopicProcessingFailed
            })?
            .inspect_err(|error| {
                diagnostics::unpersisted_error(
                    "forum_reply",
                    "claim_topic",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
            })?;
        let Some(topic) = topics.pop() else {
            break;
        };
        let diagnostic_auction_id = topic.auction_id;
        if tokio::time::Instant::now() >= item_deadline {
            runtime.observe_budget(
                topic.world,
                auction_application::external_metrics::ForumBudgetPhase::TopicItem,
            );
            diagnostics::unpersisted_error(
                "forum_reply",
                "check_claim_deadline",
                SafeErrorKind::LEASE_EXPIRED,
                DiagnosticContext {
                    auction_id: diagnostic_id(&diagnostic_auction_id),
                    ..Default::default()
                },
            );
            // 不读取或编辑远端主题，也不把可能已提交的 claim 释放成新一轮工作。
            return Err(WorkerError::ForumTopicProcessingFailed);
        }
        // 尚未读取远端或导入命令，drain 后保留原租约等待恢复，不清除历史未知写入门禁。
        if worker.control.is_draining() {
            break;
        }
        if !seen_auctions.insert(topic.auction_id) {
            // 同一轮不重复读取远端主题；释放刚领取的重复 claim 后等待下一次 poll。
            let scanned_at = worker.clock.now();
            let next_scan_at = scanned_at
                .checked_add_signed(poll_interval)
                .ok_or(WorkerError::InvalidForumConfiguration)
                .inspect_err(|error| {
                    diagnostics::unpersisted_error(
                        "forum_reply",
                        "check_next_scan_time",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                })?;
            let processed_through_post_number = topic.processed_through_post_number;
            if let Some(failure_code) = topic.content_last_error_code.clone() {
                // 重复领取没有读取远端证明，不能借 finish 清掉上一轮的未知写入门禁。
                runtime
                    .store
                    .fail_topic_scan(topic, scanned_at, next_scan_at, failure_code)
                    .await
                    .inspect_err(|error| {
                        diagnostics::unpersisted_error(
                            "forum_reply",
                            "persist_duplicate_failure",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                auction_id: diagnostic_id(&diagnostic_auction_id),
                                ..Default::default()
                            },
                        );
                    })?;
                topic_failed = true;
            } else {
                runtime
                    .store
                    .finish_topic_scan(
                        topic,
                        processed_through_post_number,
                        scanned_at,
                        next_scan_at,
                    )
                    .await
                    .inspect_err(|error| {
                        diagnostics::unpersisted_error(
                            "forum_reply",
                            "finish_duplicate_scan",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                auction_id: diagnostic_id(&diagnostic_auction_id),
                                ..Default::default()
                            },
                        );
                    })?;
            }
            break;
        }
        processed += 1;
        let activity = worker
            .write_lifecycle
            .activity(auction_application::operations::CriticalWriteKind::Forum);
        let write_progress = ForumWriteProgress::with_activity(activity.clone());
        let operation = async {
            match tokio::time::timeout_at(
                item_deadline,
                process_topic(
                    runtime,
                    topic.clone(),
                    worker.clock.as_ref(),
                    poll_interval,
                    &write_progress,
                ),
            )
            .await
            {
                Ok(Ok(next_scan_at)) => {
                    // 只有 Final 阶段收紧的排期才提前唤醒；其他阶段保持常规轮询节奏。
                    if topic.reply_phase == auction_application::forum::ForumReplyPhase::Final
                        && let Some(next_scan_at) = next_scan_at
                    {
                        earliest_final_scan = Some(
                            earliest_final_scan
                                .map_or(next_scan_at, |current| current.min(next_scan_at)),
                        );
                    }
                }
                Ok(Err(error)) => {
                    diagnostics::warn(
                        "forum_reply",
                        "process_topic",
                        error.diagnostic_kind(),
                        DiagnosticContext {
                            auction_id: diagnostic_id(&topic.auction_id),
                            ..Default::default()
                        },
                    );
                    record_topic_failure(
                        runtime,
                        topic,
                        worker.clock.now(),
                        poll_interval,
                        &error,
                        &write_progress,
                    )
                    .await
                    .inspect_err(|error| {
                        diagnostics::unpersisted_error(
                            "forum_reply",
                            "record_topic_failure",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                auction_id: diagnostic_id(&diagnostic_auction_id),
                                ..Default::default()
                            },
                        );
                    })?;
                    topic_failed = true;
                }
                Err(error) => {
                    runtime.observe_budget(
                        topic.world,
                        auction_application::external_metrics::ForumBudgetPhase::TopicItem,
                    );
                    diagnostics::warn(
                        "forum_reply",
                        "process_topic_timeout",
                        diagnostics::timeout_kind(&error),
                        DiagnosticContext {
                            auction_id: diagnostic_id(&topic.auction_id),
                            ..Default::default()
                        },
                    );

                    record_topic_failure_code(
                        runtime,
                        topic,
                        worker.clock.now(),
                        poll_interval,
                        "topic_processing_timeout",
                        &write_progress,
                    )
                    .await
                    .inspect_err(|error| {
                        diagnostics::unpersisted_error(
                            "forum_reply",
                            "record_topic_timeout",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                auction_id: diagnostic_id(&diagnostic_auction_id),
                                ..Default::default()
                            },
                        );
                    })?;
                    topic_failed = true;
                }
            }
            Ok::<(), WorkerError>(())
        };
        let Some(result) = activity.run(operation).await else {
            break;
        };
        result?;
    }
    if topic_failed {
        return Err(WorkerError::ForumTopicProcessingFailed);
    }
    Ok(earliest_final_scan
        .map(|next_scan_at| (next_scan_at - worker.clock.now()).to_std().unwrap_or_default()))
}

pub(super) async fn record_topic_failure(
    runtime: &ForumReplyRuntime,
    topic: auction_application::forum::ForumReplyTopic,
    failed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
    error: &ApplicationError,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError> {
    let failure_code = match error {
        // 精确进度已证明未发送 POST 时，ownership 等预检的保守 write_unknown 不能锁死正文。
        ApplicationError::ForumRemote(remote) if remote.code() != "write_unknown" => remote.code(),
        _ => "topic_processing_failed",
    };
    record_topic_failure_code(
        runtime,
        topic,
        failed_at,
        poll_interval,
        failure_code,
        write_progress,
    )
    .await
}

pub(super) async fn record_topic_failure_code(
    runtime: &ForumReplyRuntime,
    topic: auction_application::forum::ForumReplyTopic,
    failed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
    failure_code: &str,
    write_progress: &ForumWriteProgress,
) -> Result<(), WorkerError> {
    if write_progress.is_cancelled() {
        return Ok(());
    }
    // 历史未知写入不能因后续 GET/数据库错误被覆盖成可重试状态，必须等待原有回读证明。
    let failure_code = if write_progress.has_unconfirmed_write()
        || topic.content_last_error_code.as_deref() == Some("write_unknown")
    {
        "write_unknown"
    } else {
        failure_code
    };
    let retry_at = failed_at
        .checked_add_signed(poll_interval)
        .ok_or(WorkerError::InvalidForumConfiguration)?;
    runtime.store.fail_topic_scan(topic, failed_at, retry_at, failure_code.to_owned()).await?;
    write_progress.confirm_write();
    Ok(())
}
