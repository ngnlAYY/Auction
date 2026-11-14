//! 论坛回帖一轮：领取、按主题处理与跨轮的 Final 唤醒排期；失败处置见 `failure.rs`。

use std::collections::{HashMap, HashSet};

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::forum::{ForumReplyPhase, ForumWriteProgress};
use auction_domain::AuctionId;
use chrono::{DateTime, Duration, Utc};

use super::failure::{record_topic_failure, record_topic_failure_code};
use super::topic::{FINAL_SCAN_MINIMUM_INTERVAL_SECONDS, process_topic, retry_scan_at};
use super::{ForumReplyRuntime, MAX_BATCH_SIZE};
use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError};

/// 领取调用失败后至少等待的时长，与 Final 阶段最短导入间隔一致。
const CLAIM_FAILURE_RETRY: std::time::Duration =
    std::time::Duration::from_secs(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS.unsigned_abs());

/// 一轮回帖导入的结果。Final 提示与成败分开返回：单个主题或领取失败不能吞掉其他主题已排定
/// 的提前唤醒，否则论坛循环退回常规轮询，截止前的回帖可能在截止后才导入而被拒。
pub(crate) struct ForumReplyCycleOutcome {
    /// 距最早 Final 排期的等待；只用于唤醒，持久 `next_scan_at` 仍是权威。
    pub(crate) earliest_final_scan: Option<std::time::Duration>,
    /// 失败只影响 control 健康状态、诊断与指标，不改变唤醒提示。
    pub(crate) result: Result<(), WorkerError>,
}

/// 本轮最后一次领取的结果，决定一轮结束时如何处置已到期的 Final 排期。
enum LastClaim {
    /// 领到了主题。每次只领取一场，此后无论因批量上限、同轮重复领取还是出错退出，其他
    /// 已到期主题都可能只是还没轮到。
    Topic,
    /// 领取成功但为空：在该时刻已到期的主题都不可领取。
    Empty(DateTime<Utc>),
    /// 领取调用失败、超时或被排空取消：无法判断主题是否仍可领取。
    Failed,
}

/// 论坛循环跨轮持有的 Final 排期：每个 Final 主题最近一次持久排定的下一次导入时刻。
///
/// 只决定何时唤醒，持久 `next_scan_at` 仍是唯一权威；进程重启后从空集合重新积累。跨轮保留
/// 是因为某一轮可能只处理到刚离开 Final 的主题，另一个仍在 Final、尚未到期的主题不能因本轮
/// 没有新提示而等满常规轮询。同一主题被重新排期时以新值覆盖，作废的较早时刻不能把循环
/// 唤醒成空轮。
#[derive(Default)]
pub(crate) struct FinalScanSchedule {
    next_scans: HashMap<AuctionId, DateTime<Utc>>,
    /// 本轮最后一次领取的结果；`None` 表示本轮没有发起领取（例如凭据不可用）。
    last_claim: Option<LastClaim>,
}

impl FinalScanSchedule {
    /// 记录主题的最新 Final 排期；`None` 表示该主题已回到常规节奏，移除旧提示。
    fn record(&mut self, auction_id: AuctionId, next_scan_at: Option<DateTime<Utc>>) {
        match next_scan_at {
            Some(next_scan_at) => {
                self.next_scans.insert(auction_id, next_scan_at);
            }
            None => {
                self.next_scans.remove(&auction_id);
            }
        }
    }

    /// 结束一轮并返回距最早 Final 排期的等待。
    ///
    /// 只有领取成功却为空才能证明已到期的主题不可领取（离开可扫描状态、凭据不可用或租约仍被
    /// 持有），此时丢弃不晚于那次领取的排期，以免循环按下限空转；本轮没有发起领取时以结束
    /// 时刻为界。最后一次领取领到了主题时，其他已到期主题可能只是没轮到，即使本轮随后出错
    /// 退出也全部保留，已过期的排期返回零，由论坛循环的下限尽快开始下一轮。领取调用失败时
    /// 同样保留，但至少等待 Final 最短间隔，数据库故障期间不按下限空转。
    fn finish_cycle(&mut self, now: DateTime<Utc>) -> Option<std::time::Duration> {
        let minimum_wait = match self.last_claim.take() {
            Some(LastClaim::Topic) => std::time::Duration::ZERO,
            Some(LastClaim::Failed) => CLAIM_FAILURE_RETRY,
            Some(LastClaim::Empty(claimed_at)) => {
                self.next_scans.retain(|_, next_scan_at| *next_scan_at > claimed_at);
                std::time::Duration::ZERO
            }
            None => {
                self.next_scans.retain(|_, next_scan_at| *next_scan_at > now);
                std::time::Duration::ZERO
            }
        };
        let earliest = self.next_scans.values().min()?;
        Some((*earliest - now).to_std().unwrap_or_default().max(minimum_wait))
    }
}

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumReplyRuntime,
    lease_duration: std::time::Duration,
    poll_interval: std::time::Duration,
    hot: Option<auction_application::admin::WorkerForumHotConfig>,
    final_scans: &mut FinalScanSchedule,
) -> ForumReplyCycleOutcome
where
    C: auction_application::core::Clock + 'static,
{
    let result = run_topics(worker, runtime, lease_duration, poll_interval, hot, final_scans).await;
    ForumReplyCycleOutcome {
        earliest_final_scan: final_scans.finish_cycle(worker.clock.now()),
        result,
    }
}

async fn run_topics<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumReplyRuntime,
    lease_duration: std::time::Duration,
    poll_interval: std::time::Duration,
    hot: Option<auction_application::admin::WorkerForumHotConfig>,
    final_scans: &mut FinalScanSchedule,
) -> Result<(), WorkerError>
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
        // 领取有结果之前按失败处理：出错、超时或被排空取消都不能证明主题已不可领取。
        final_scans.last_claim = Some(LastClaim::Failed);
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
            // 领取成功但为空：不晚于此刻的排期对应的主题已不可领取。
            final_scans.last_claim = Some(LastClaim::Empty(now));
            break;
        };
        // 领到主题后，本轮无论因批量上限、重复领取还是出错结束，其他已到期主题都可能还没
        // 轮到，排期必须保留。
        final_scans.last_claim = Some(LastClaim::Topic);
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
            // 处理其他主题耗时超过 Final 间隔时会重领到它，释放时仍须沿用 Final 节奏，
            // 不能推到常规轮询之后；未知写入维持常规轮询。
            let scanned_at = worker.clock.now();
            let final_cadence = topic.reply_phase == ForumReplyPhase::Final
                && topic.content_last_error_code.as_deref() != Some("write_unknown");
            let next_scan_at =
                retry_scan_at(runtime, &topic, scanned_at, poll_interval, final_cadence)
                    .await
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
            final_scans.record(diagnostic_auction_id, final_cadence.then_some(next_scan_at));
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
                    // 只有 Final 阶段收紧的排期才提前唤醒；离开 Final 的主题回到常规轮询节奏，
                    // 与失败、重复领取路径一样移除旧提示，作废排期不会让循环多跑空轮。
                    let final_scan =
                        next_scan_at.filter(|_| topic.reply_phase == ForumReplyPhase::Final);
                    final_scans.record(diagnostic_auction_id, final_scan);
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
                    let retry_at = record_topic_failure(
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
                    final_scans.record(diagnostic_auction_id, retry_at);
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

                    let retry_at = record_topic_failure_code(
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
                    final_scans.record(diagnostic_auction_id, retry_at);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auction_domain::AuctionId;
    use chrono::{DateTime, Utc};

    use super::{CLAIM_FAILURE_RETRY, FinalScanSchedule, LastClaim};
    use crate::worker::runtime::test_fixture::base_time;

    fn at(seconds: i64) -> DateTime<Utc> {
        base_time() + chrono::Duration::seconds(seconds)
    }

    #[test]
    fn rescheduled_topic_overwrites_its_earlier_final_scan() {
        let mut schedule = FinalScanSchedule::default();
        let auction_id = AuctionId::new();
        schedule.record(auction_id, Some(at(10)));
        // 同轮重领后的新排期覆盖作废的较早时刻，不能把循环唤醒成空轮。
        schedule.record(auction_id, Some(at(25)));
        assert_eq!(schedule.finish_cycle(at(15)), Some(Duration::from_secs(10)));
        // 回到常规节奏的主题移除提示。
        schedule.record(auction_id, None);
        assert_eq!(schedule.finish_cycle(at(15)), None);
    }

    #[test]
    fn only_an_empty_claim_drops_due_scans() {
        let mut schedule = FinalScanSchedule::default();
        schedule.record(AuctionId::new(), Some(at(5)));
        schedule.record(AuctionId::new(), Some(at(11)));
        schedule.record(AuctionId::new(), Some(at(30)));

        // 最后一次领取领到了主题（批量上限、重复领取或领取后出错退出）：其他已到期主题可能
        // 只是没轮到，全部保留，下一轮尽快开始。
        schedule.last_claim = Some(LastClaim::Topic);
        assert_eq!(schedule.finish_cycle(at(12)), Some(Duration::ZERO));

        // 领取调用失败：同样保留，但至少等待 Final 最短间隔，不按下限空转。
        schedule.last_claim = Some(LastClaim::Failed);
        assert_eq!(schedule.finish_cycle(at(12)), Some(CLAIM_FAILURE_RETRY));
        assert_eq!(CLAIM_FAILURE_RETRY, Duration::from_secs(5));

        // 领取成功但为空：那次领取时已到期的排期作废；领取之后才到期的保留。
        schedule.last_claim = Some(LastClaim::Empty(at(10)));
        assert_eq!(schedule.finish_cycle(at(12)), Some(Duration::ZERO));

        // 本轮没有发起领取（如凭据不可用）时以结束时刻为界，避免按下限空转。
        assert_eq!(schedule.finish_cycle(at(12)), Some(Duration::from_secs(18)));

        // 最短等待只作用于已到期或即将到期的排期，更晚的排期仍按原时刻唤醒。
        schedule.last_claim = Some(LastClaim::Failed);
        assert_eq!(schedule.finish_cycle(at(12)), Some(Duration::from_secs(18)));
    }
}
