//! 论坛回帖导入 Worker 的模块装配。
//!
//! 主题处理、竞价命令、MoogleMail 通知和共享幂等辅助分别由子模块负责；本文件保留
//! runtime 配置与 `run_cycle` 入口，不改变游标推进和 warning-only 边界。

use std::{collections::HashMap, sync::Arc};

use auction_application::{
    ApplicationError, CompetitionCommandPort, CompetitionTransactionError, CreateProxyCommand,
    ForumBidCommandRecord, ForumCommandStatus, ForumReplyStore, ForumTopicReader,
    LowerProxyCommand, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailNotificationTemplate, PlaceBidCommand, RequestHash, forum_world_name,
};
use auction_domain::{BidAmount, CommandId, ProxyMode};
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
use common::{
    bounded_detail, finish, forum_command_id, is_retryable, proxy_mode_name, repository_error,
};
use notification::process_notification;
use topic::process_topic;

const MAX_BATCH_SIZE: u16 = 100;

/// 论坛回帖导入运行时依赖。
pub struct ForumReplyRuntime {
    pub(crate) store: Arc<dyn ForumReplyStore>,
    pub(crate) reader: Arc<dyn ForumTopicReader>,
    pub(crate) commands: Arc<dyn CompetitionCommandPort>,
    pub(crate) notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub(crate) batch_size: u16,
    pub(crate) item_timeout: std::time::Duration,
}

impl ForumReplyRuntime {
    /// 建立论坛回帖导入运行时，并限制单轮主题读取数量。
    pub fn new(
        store: Arc<dyn ForumReplyStore>,
        reader: Arc<dyn ForumTopicReader>,
        commands: Arc<dyn CompetitionCommandPort>,
        notifier: Option<Arc<dyn MailDeliveryEnqueuer>>,
        batch_size: u16,
        item_timeout: std::time::Duration,
    ) -> Result<Self, WorkerError> {
        if batch_size == 0 || batch_size > MAX_BATCH_SIZE || item_timeout.is_zero() {
            return Err(WorkerError::InvalidForumReplyConfiguration);
        }
        Ok(Self { store, reader, commands, notifier, batch_size, item_timeout })
    }
}

pub(crate) async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &ForumReplyRuntime,
) -> Result<(), WorkerError>
where
    C: auction_application::Clock + 'static,
{
    let topics = runtime.store.list_published_topics(runtime.batch_size).await?;
    let mut worlds = Vec::new();
    let mut failed_worlds = Vec::new();
    for topic in topics {
        if !worlds.contains(&topic.world) {
            worlds.push(topic.world);
        }
        match tokio::time::timeout(runtime.item_timeout, process_topic(runtime, topic.clone()))
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
