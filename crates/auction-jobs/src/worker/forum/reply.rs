//! 论坛回帖导入 Worker 的模块装配。
//!
//! 主题处理、竞价命令、MoogleMail 通知和共享幂等辅助分别由子模块负责；本文件保留
//! runtime 配置与 `run_cycle` 入口，不改变游标推进和 warning-only 边界。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use auction_application::{
    ApplicationError, CompetitionCommandPort, CompetitionTransactionError, CreateProxyCommand,
    ForumBidCommandRecord, ForumCommandStatus, ForumPostWriter, ForumRenderer, ForumReplyStore,
    ForumTopicReader, LowerProxyCommand, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate, PlaceBidCommand,
    PublicEquipmentLinkResolver, RequestHash, forum_world_name,
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
#[path = "reply/topic.rs"]
mod topic;

use command::process_command;
use common::{bounded_detail, finish, forum_command_id, is_retryable, repository_error};
use notification::process_notification;
use topic::process_topic;

const MAX_BATCH_SIZE: u16 = 100;

/// 论坛回帖导入运行时依赖。
pub struct ForumReplyPorts {
    pub store: Arc<dyn ForumReplyStore>,
    pub reader: Arc<dyn ForumTopicReader>,
    pub writer: Arc<dyn ForumPostWriter>,
    pub renderer: Arc<dyn ForumRenderer>,
    pub equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub commands: Arc<dyn CompetitionCommandPort>,
    pub notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
}

/// 论坛回帖导入运行时依赖。
pub struct ForumReplyRuntime {
    pub(crate) store: Arc<dyn ForumReplyStore>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) writer: Arc<dyn ForumPostWriter>,
    pub(crate) renderer: Arc<dyn ForumRenderer>,
    pub(crate) equipment_links: Arc<dyn PublicEquipmentLinkResolver>,
    pub(crate) commands: Arc<dyn CompetitionCommandPort>,
    pub(crate) notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
    pub(crate) public_base_url: Option<String>,
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
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
            notifier,
        } = ports;
        Ok(Self {
            store,
            reader,
            writer,
            renderer,
            equipment_links,
            commands,
            notifier,
            batch_size,
            item_timeout,
            public_base_url,
        })
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
    let mut worlds = Vec::new();
    let mut failed_worlds = Vec::new();
    let mut seen_auctions = HashSet::new();
    let mut processed = 0_u16;
    while processed < runtime.batch_size {
        let now = worker.clock.now();
        let lease_until =
            now.checked_add_signed(lease_duration).ok_or(WorkerError::InvalidForumConfiguration)?;
        // 每次只领取即将处理的一场拍卖，避免批量候选在等待前序主题时耗尽内容租约。
        let mut topics = runtime
            .store
            .claim_published_topics(worker.worker_id.clone(), now, lease_until, 1)
            .await?;
        let Some(topic) = topics.pop() else {
            break;
        };
        if !seen_auctions.insert(topic.auction_id) {
            // 同一轮不重复读取远端主题；释放刚领取的重复 claim 后等待下一次 poll。
            let scanned_at = worker.clock.now();
            let next_scan_at = scanned_at
                .checked_add_signed(poll_interval)
                .ok_or(WorkerError::InvalidForumConfiguration)?;
            runtime.store.finish_topic_scan(topic, scanned_at, next_scan_at).await?;
            break;
        }
        processed += 1;
        if !worlds.contains(&topic.world) {
            worlds.push(topic.world);
        }
        match tokio::time::timeout(
            runtime.item_timeout,
            process_topic(runtime, topic.clone(), worker.clock.as_ref(), poll_interval),
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
                if !failed_worlds.contains(&topic.world) {
                    failed_worlds.push(topic.world);
                }
            }
            Err(_) => {
                tracing::warn!(
                    auction_id = %topic.auction_id,
                    thread_id = %topic.thread_id,
                    "论坛回帖主题导入超时，保留该世界游标等待重试"
                );
                if !failed_worlds.contains(&topic.world) {
                    failed_worlds.push(topic.world);
                }
            }
        }
    }

    let mut scan_error = None;
    for world in worlds {
        // 一个世界可能在本批次包含多个主题；只有全部主题都成功，才允许推进该世界游标。
        if failed_worlds.contains(&world) {
            continue;
        }
        if let Err(error) = runtime.store.mark_scanned(world, worker.clock.now()).await {
            tracing::error!(
                world = ?world,
                error = %error,
                "论坛回帖扫描游标写入失败"
            );
            if scan_error.is_none() {
                scan_error = Some(error);
            }
        }
    }
    if let Some(error) = scan_error {
        return Err(error.into());
    }
    Ok(())
}
