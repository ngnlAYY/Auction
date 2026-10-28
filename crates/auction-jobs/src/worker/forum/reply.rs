//! 论坛回帖导入 Worker 的模块装配；runtime 配置与 `run_cycle` 入口在此，游标推进和
//! warning-only 边界不变。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
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

#[path = "reply/command.rs"]
mod command;
#[path = "reply/common.rs"]
mod common;
#[path = "reply/notification.rs"]
mod notification;
#[path = "reply/rejection.rs"]
mod rejection;
#[path = "reply/topic.rs"]
mod topic;

use command::process_command;
use common::{bounded_detail, finish, forum_command_id, is_retryable, repository_error};
use notification::process_notification;
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
        })
    }

    /// 配置正常固定帖刷新的额外正文校验；未知写入始终只读核对。
    pub fn with_post_body_verification(mut self, enabled: bool) -> Self {
        self.verify_post_body = enabled;
        self
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
    let lease_duration =
        Duration::from_std(lease_duration).map_err(|_| WorkerError::InvalidForumConfiguration)?;
    let poll_interval =
        Duration::from_std(poll_interval).map_err(|_| WorkerError::InvalidForumConfiguration)?;
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
            topic_failed = true;
            break;
        }
        // 内容租约必须覆盖领取和远端编辑的总预算，领取响应不重置单项时限。
        let item_deadline = tokio::time::Instant::now()
            .checked_add(runtime.item_timeout)
            .ok_or(WorkerError::InvalidForumConfiguration)?;
        let now = worker.clock.now();
        let lease_until =
            now.checked_add_signed(lease_duration).ok_or(WorkerError::InvalidForumConfiguration)?;
        // 每次只领取即将处理的一场拍卖，避免批量候选在等待前序主题时耗尽内容租约。
        let mut topics = tokio::time::timeout_at(
            item_deadline,
            runtime.store.claim_published_topics(
                worker.worker_id.clone(),
                now,
                lease_until,
                available_worlds,
                1,
            ),
        )
        .await
        .map_err(|_| WorkerError::ForumTopicProcessingFailed)??;
        let Some(topic) = topics.pop() else {
            break;
        };
        if tokio::time::Instant::now() >= item_deadline {
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
                .ok_or(WorkerError::InvalidForumConfiguration)?;
            let processed_through_post_number = topic.processed_through_post_number;
            if let Some(failure_code) = topic.content_last_error_code.clone() {
                // 重复领取没有读取远端证明，不能借 finish 清掉上一轮的未知写入门禁。
                runtime
                    .store
                    .fail_topic_scan(topic, scanned_at, next_scan_at, failure_code)
                    .await?;
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
                    .await?;
            }
            break;
        }
        processed += 1;
        let write_progress = ForumWriteProgress::default();
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
                tracing::warn!(
                    auction_id = %topic.auction_id,
                    thread_id = %topic.thread_id,
                    error = %error,
                    "论坛回帖主题导入未完成"
                );
                record_topic_failure(
                    runtime,
                    topic,
                    worker.clock.now(),
                    poll_interval,
                    &error,
                    &write_progress,
                )
                .await?;
                topic_failed = true;
            }
            Err(_) => {
                tracing::warn!(
                    auction_id = %topic.auction_id,
                    thread_id = %topic.thread_id,
                    "论坛回帖主题导入超时，保留该主题等待持久化调度重试"
                );
                record_topic_failure_code(
                    runtime,
                    topic,
                    worker.clock.now(),
                    poll_interval,
                    "topic_processing_timeout",
                    write_progress.has_unconfirmed_write(),
                )
                .await?;
                topic_failed = true;
            }
        }
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
        write_progress.has_unconfirmed_write(),
    )
    .await
}

async fn record_topic_failure_code(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
    failed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
    failure_code: &str,
    unconfirmed_write: bool,
) -> Result<(), WorkerError> {
    // 历史未知写入不能因后续 GET/数据库错误被覆盖成可重试状态，必须等待原有回读证明。
    let failure_code =
        if unconfirmed_write || topic.content_last_error_code.as_deref() == Some("write_unknown") {
            "write_unknown"
        } else {
            failure_code
        };
    let retry_at = failed_at
        .checked_add_signed(poll_interval)
        .ok_or(WorkerError::InvalidForumConfiguration)?;
    runtime
        .store
        .fail_topic_scan(topic, failed_at, retry_at, failure_code.to_owned())
        .await
        .map_err(Into::into)
}
