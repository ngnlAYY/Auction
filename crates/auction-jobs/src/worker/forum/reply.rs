//! 论坛回帖导入 Worker 的模块装配；runtime 配置与 `run_cycle` 入口在此，游标推进和
//! warning-only 边界不变。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use auction_application::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::{
    ApplicationError, CompetitionCommandPort, CompetitionTransactionError, CreateProxyCommand,
    ForumBidCommandRecord, ForumCommandStatus, ForumCredentialAvailability, ForumPostWriter,
    ForumRenderer, ForumReplyStore, ForumTopicReader, ForumWriteProgress, LowerProxyCommand,
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate, PlaceBidCommand, PublicEquipmentLinkResolver, RequestHash,
};
use auction_domain::{BidAmount, CommandId, ProxyMode};
use chrono::Duration;
use serde::Serialize;
use uuid::Uuid;

use super::{PersistentWorker, WorkerError};
use crate::worker::diagnostics;

#[path = "reply/command.rs"]
mod command;
#[path = "reply/common.rs"]
mod common;
#[path = "reply/notification.rs"]
mod notification;
#[path = "reply/rejection.rs"]
mod rejection;
mod reply_delay;
#[path = "reply/topic.rs"]
mod topic;

use command::process_command;
use common::{bounded_detail, finish, forum_command_id, is_retryable, repository_error};
use notification::process_notification;
use reply_delay::ReplyDelayEstimator;
use topic::process_topic;

const MAX_BATCH_SIZE: u16 = 100;

/// 单条论坛命令已经证明的帖子、身份与 lot 上下文。
struct ForumCommandContext<'a> {
    topic: &'a auction_application::ForumReplyTopic,
    bidder: &'a auction_application::ForumBidderIdentity,
    post_id: &'a str,
    post_number: usize,
    lot_code: &'a str,
    command: &'a auction_application::AdminForumCommand,
    lot_id: auction_domain::LotId,
}

/// 论坛回帖导入运行时依赖。
pub struct ForumReplyPorts {
    pub store: Arc<dyn ForumReplyStore>,
    pub credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub reader: Arc<dyn ForumTopicReader>,
    pub writer: Arc<dyn ForumPostWriter>,
    pub renderer: Arc<dyn ForumRenderer>,
    pub equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub commands: Arc<dyn CompetitionCommandPort>,
    pub notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
}

pub struct ForumReplyRuntime {
    pub(crate) store: Arc<dyn ForumReplyStore>,
    pub(crate) credential_availability: Arc<dyn ForumCredentialAvailability>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) writer: Arc<dyn ForumPostWriter>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub(crate) commands: Arc<dyn CompetitionCommandPort>,
    pub(crate) notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
    pub(crate) verify_post_body: bool,
    pub(crate) external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
    reply_delay: ReplyDelayEstimator,
}

impl ForumReplyRuntime {
    /// 建立论坛回帖导入运行时，并限制单轮主题读取数量。
    pub fn new(
        ports: ForumReplyPorts,
        batch_size: u16,
        item_timeout: std::time::Duration,
        public_base_url: Option<String>,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0
            || batch_size > MAX_BATCH_SIZE
            || item_timeout.is_zero()
            || public_base_url
                .as_deref()
                .is_some_and(|value| !super::forum_sync::valid_public_base_url(value))
        {
            return Err(WorkerError::InvalidForumReplyConfiguration);
        }
        let ForumReplyPorts {
            store,
            credential_availability,
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
            notifier,
        } = ports;
        Ok(Self {
            store,
            credential_availability,
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
            notifier,
            batch_size,
            item_timeout,
            public_base_url,
            verify_post_body: false,
            external_metrics: auction_application::noop_external_metrics_observer(),
            reply_delay: ReplyDelayEstimator::new(
                auction_application::noop_external_metrics_observer(),
            ),
        })
    }

    /// 配置正常固定帖刷新的额外正文校验；未知写入始终只读核对。
    pub fn with_post_body_verification(mut self, enabled: bool) -> Self {
        self.verify_post_body = enabled;
        self
    }

    /// 注入同步外部客户端指标；延迟估计只读内存，不改变扫描或命令控制流。
    pub fn with_external_metrics(
        mut self,
        observer: Arc<dyn auction_application::ExternalMetricsObserver>,
    ) -> Self {
        self.reply_delay = ReplyDelayEstimator::new(Arc::clone(&observer));
        self.external_metrics = observer;
        self
    }

    /// 记录论坛单项预算耗尽；只读同步计数，不改变截止点。
    pub(crate) fn observe_budget(
        &self,
        world: auction_domain::SettlementWorld,
        phase: auction_application::ForumBudgetPhase,
    ) {
        let world = match world {
            auction_domain::SettlementWorld::Main => auction_application::ForumWorld::Main,
            auction_domain::SettlementWorld::Isekai => auction_application::ForumWorld::Isekai,
        };
        self.external_metrics.observe(
            auction_application::ExternalMetricsEvent::ForumBudgetExhausted { world, phase },
        );
    }
}

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumReplyRuntime,
    lease_duration: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<(), WorkerError>
where
    C: auction_application::Clock + 'static,
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
    let mut seen_auctions = HashSet::new();
    let mut processed = 0_u16;
    let mut topic_failed = false;
    while processed < runtime.batch_size && !worker.control.is_draining() {
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
            .checked_add(runtime.item_timeout)
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
            runtime.observe_budget(topic.world, auction_application::ForumBudgetPhase::TopicItem);
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
        let activity =
            worker.write_lifecycle.activity(auction_application::CriticalWriteKind::Forum);
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
                Ok(Ok(())) => {}
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
                        auction_application::ForumBudgetPhase::TopicItem,
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
    if topic_failed { Err(WorkerError::ForumTopicProcessingFailed) } else { Ok(()) }
}

async fn record_topic_failure(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
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

async fn record_topic_failure_code(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
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
