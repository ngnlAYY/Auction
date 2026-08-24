//! 论坛回帖导入 Worker。
//!
//! 该循环只负责把已发布主题中的受限命令映射到现有竞价应用端口。身份、lot 映射和命令
//! 账本由存储端口拥有；Worker 不直接更新竞价表，也不复制竞价状态机。

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

async fn process_topic(
    runtime: &ForumReplyRuntime,
    topic: auction_application::ForumReplyTopic,
) -> Result<(), ApplicationError> {
    let preview = runtime.reader.fetch_topic(topic.world, topic.thread_id.clone()).await?;
    let targets = runtime.store.lot_targets(topic.auction_id).await?;
    let target_by_code = target_map(targets)?;

    for post in preview.posts {
        if post.ignored {
            continue;
        }
        let Some(actor) = post.post.author_id.clone().or_else(|| post.post.author.clone()) else {
            tracing::warn!(
                auction_id = %topic.auction_id,
                post_id = %post.post.id,
                "论坛帖子缺少可解析的身份，跳过命令"
            );
            continue;
        };
        let display_name = post.post.author.clone().unwrap_or_else(|| actor.clone());
        let bidder =
            match runtime.store.resolve_bidder(topic.world, actor.clone(), display_name).await {
                Ok(bidder) => bidder,
                Err(error) => {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                        post_id = %post.post.id,
                        error = %error,
                        "论坛帖子身份未通过登记映射"
                    );
                    continue;
                }
            };
        for command in post.commands {
            for lot_code in &command.lot_codes {
                let Some(&lot_id) = target_by_code.get(lot_code) else {
                    tracing::warn!(
                        auction_id = %topic.auction_id,
                    lot_code = %lot_code,
                        "论坛命令 lot 编码未找到本地映射"
                    );
                    continue;
                };
                process_command(
                    runtime,
                    &topic,
                    &bidder,
                    &post.post.id,
                    lot_code,
                    &command,
                    lot_id,
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn target_map(
    targets: Vec<auction_application::ForumLotTarget>,
) -> Result<HashMap<String, auction_domain::LotId>, ApplicationError> {
    let mut result = HashMap::with_capacity(targets.len());
    for target in targets {
        if target.code.is_empty()
            || target.lot_id.is_nil()
            || result.insert(target.code, target.lot_id).is_some()
        {
            return Err(repository_error("论坛 lot 映射重复或无效"));
        }
    }
    if result.is_empty() {
        return Err(repository_error("论坛 lot 映射为空"));
    }
    Ok(result)
}

async fn process_command(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::ForumReplyTopic,
    bidder: &auction_application::ForumBidderIdentity,
    post_id: &str,
    lot_code: &str,
    command: &auction_application::AdminForumCommand,
    lot_id: auction_domain::LotId,
) -> Result<(), ApplicationError> {
    let command_kind = if command.kind == "proxy" && command.proxy_lower {
        "proxy_lower"
    } else {
        command.kind.as_str()
    };
    if !matches!(command_kind, "bid" | "proxy" | "proxy_lower") {
        if command_kind == "notify" {
            process_notification(runtime, topic, bidder, post_id, lot_code, command, lot_id)
                .await?;
        }
        tracing::info!(
            auction_id = %topic.auction_id,
            post_id,
            command_kind,
            "论坛命令当前不改变竞价状态，已跳过"
        );
        return Ok(());
    }
    let Some(amount) = command.amount else {
        return Ok(());
    };
    let Some(line_number) = i32::try_from(command.line).ok() else {
        tracing::warn!(post_id, "论坛命令行号超出持久化范围");
        return Ok(());
    };
    let command_id = forum_command_id(
        topic.auction_id,
        &topic.thread_id,
        post_id,
        line_number,
        lot_code,
        command_kind,
    );
    let record = ForumBidCommandRecord {
        command_id,
        auction_id: topic.auction_id,
        world: topic.world,
        thread_id: topic.thread_id.clone(),
        post_id: post_id.to_owned(),
        line_number,
        lot_code: lot_code.to_owned(),
        actor: bidder.external_account_id.clone(),
        command_kind: command_kind.to_owned(),
        amount,
    };
    let start = match runtime.store.begin_command(record).await {
        Ok(start) => start,
        Err(error) => {
            tracing::error!(
                command_id = %command_id,
                error = %error,
                "论坛命令账本开始失败"
            );
            return Err(error);
        }
    };
    if !start.should_run {
        // 竞价命令已经完成时仍需重放安全的应用命令，以便恢复此前入账但通知失败的场景。
        // 应用端口以同一 command_id 返回既有结果，不会重复写入竞价事实。
        if start.status == ForumCommandStatus::Applied
            && command.notify.as_deref().is_some_and(|mode| matches!(mode, "mm" | "both"))
        {
            execute_command(
                runtime,
                command_kind,
                command,
                lot_id,
                bidder.user_id,
                command_id,
                amount,
            )
            .await?;
            process_notification(runtime, topic, bidder, post_id, lot_code, command, lot_id)
                .await?;
        }
        return Ok(());
    }

    let result =
        execute_command(runtime, command_kind, command, lot_id, bidder.user_id, command_id, amount)
            .await;
    let succeeded = result.is_ok();
    match result {
        Ok(()) => finish(runtime, command_id, "applied", "", "论坛竞价命令已应用").await?,
        Err(error) if is_retryable(&error) => {
            tracing::warn!(command_id = %command_id, error = %error, "论坛竞价命令暂时未完成，将重试");
            return Err(error);
        }
        Err(error) => {
            let detail = bounded_detail(error.to_string());
            finish(runtime, command_id, "rejected", "command_rejected", &detail).await?;
        }
    }
    if succeeded && command.notify.as_deref().is_some_and(|mode| matches!(mode, "mm" | "both")) {
        process_notification(runtime, topic, bidder, post_id, lot_code, command, lot_id).await?;
    }
    Ok(())
}

async fn process_notification(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::ForumReplyTopic,
    bidder: &auction_application::ForumBidderIdentity,
    post_id: &str,
    lot_code: &str,
    command: &auction_application::AdminForumCommand,
    lot_id: auction_domain::LotId,
) -> Result<(), ApplicationError> {
    let mode = command.notify.as_deref().unwrap_or("both");
    if mode == "pm" {
        tracing::info!(post_id, "论坛 PM 通知 owner 尚未装配");
        return Ok(());
    }
    if !matches!(mode, "mm" | "both") {
        tracing::warn!(post_id, "论坛通知模式不在闭集内");
        return Ok(());
    }
    let Some(line) = i32::try_from(command.line).ok() else {
        tracing::warn!(post_id, "论坛通知行号超出持久化范围");
        return Ok(());
    };
    let command_id =
        forum_command_id(topic.auction_id, &topic.thread_id, post_id, line, lot_code, "notify");
    let record = ForumBidCommandRecord {
        command_id,
        auction_id: topic.auction_id,
        world: topic.world,
        thread_id: topic.thread_id.clone(),
        post_id: post_id.to_owned(),
        line_number: line,
        lot_code: lot_code.to_owned(),
        actor: bidder.external_account_id.clone(),
        command_kind: "notify".to_owned(),
        amount: 0,
    };
    let start = match runtime.store.begin_command(record).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(command_id = %command_id, error = %error, "论坛通知账本开始失败");
            return Err(error);
        }
    };
    if !start.should_run {
        return Ok(());
    }
    let Some(notifier) = runtime.notifier.as_ref() else {
        finish(
            runtime,
            command_id,
            "rejected",
            "notifier_unavailable",
            "论坛 MoogleMail 通知 owner 未装配",
        )
        .await?;
        return Ok(());
    };
    let request = ForumNotificationRequest {
        auction_id: topic.auction_id.as_uuid(),
        lot_id: lot_id.as_uuid(),
        lot_code,
        mode,
        thread_id: &topic.thread_id,
        post_id,
        line: command.line,
    };
    let request_hash = match RequestHash::from_request("forum_notify", lot_id.as_uuid(), &request) {
        Ok(hash) => hash,
        Err(error) => {
            let detail = bounded_detail(error.to_string());
            finish(runtime, command_id, "rejected", "request_hash_failed", &detail).await?;
            return Ok(());
        }
    };
    let external_mail_id =
        format!("forum:{}:{}:{}:{}:{}", topic.auction_id, topic.thread_id, post_id, line, lot_code);
    let result = notifier
        .enqueue(MailDeliveryCommand {
            command_id: CommandId::from_uuid(command_id),
            request_hash,
            origin: MailDeliveryOrigin::MailIntake,
            origin_id: external_mail_id.clone(),
            operation: MailDeliveryOperation::Send,
            world_id: forum_world_name(topic.world).to_owned(),
            external_mail_id,
            recipient: bidder.external_account_id.clone(),
            template: Some(MailNotificationTemplate::ForumAuctionNotice),
            result_code: Some("forum_auction_notice".to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: None,
        })
        .await;
    match result {
        Ok(result) if !result.delivery_id.is_nil() => {
            finish(runtime, command_id, "applied", "", "论坛 MoogleMail 通知已入队").await?;
        }
        Ok(_) => {
            finish(
                runtime,
                command_id,
                "rejected",
                "owner_result_invalid",
                "论坛通知 owner 返回无效结果",
            )
            .await?;
        }
        Err(error) if is_retryable(&error) => {
            tracing::warn!(command_id = %command_id, error = %error, "论坛 MoogleMail 通知暂时未入队，将重试");
            return Err(error);
        }
        Err(error) => {
            let detail = bounded_detail(error.to_string());
            finish(runtime, command_id, "rejected", "notification_rejected", &detail).await?;
        }
    }
    Ok(())
}

async fn execute_command(
    runtime: &ForumReplyRuntime,
    command_kind: &str,
    command: &auction_application::AdminForumCommand,
    lot_id: auction_domain::LotId,
    bidder_id: auction_domain::UserId,
    command_id: Uuid,
    amount: i64,
) -> Result<(), ApplicationError> {
    let request = ForumCommandRequest {
        command_kind,
        amount,
        mode: command.proxy.as_deref(),
        authorization_id: None,
    };
    match command_kind {
        "bid" => {
            let amount = BidAmount::new(amount)?;
            let request_hash = RequestHash::from_request("place_bid", lot_id.as_uuid(), &request)?;
            runtime
                .commands
                .place_bid(PlaceBidCommand {
                    command_id: CommandId::from_uuid(command_id),
                    lot_id,
                    bidder_id,
                    amount,
                    request_hash,
                })
                .await
                .map(|_| ())
        }
        "proxy" => {
            let amount = BidAmount::new(amount)?;
            let mode = match command.proxy.as_deref() {
                Some("fast") | None => ProxyMode::Fast,
                Some("slow") => ProxyMode::Slow,
                Some(_) => return Err(repository_error("论坛代理模式不在闭集内")),
            };
            let request = ForumCommandRequest { mode: Some(proxy_mode_name(mode)), ..request };
            let request_hash =
                RequestHash::from_request("create_proxy", lot_id.as_uuid(), &request)?;
            runtime
                .commands
                .create_proxy(CreateProxyCommand {
                    command_id: CommandId::from_uuid(command_id),
                    lot_id,
                    bidder_id,
                    maximum_amount: amount,
                    mode: Some(mode),
                    request_hash,
                })
                .await
                .map(|_| ())
        }
        "proxy_lower" => {
            let amount = BidAmount::new(amount)?;
            let Some(proxy) = runtime.store.active_proxy(lot_id, bidder_id).await? else {
                return Err(repository_error("论坛用户没有可降低的活动代理"));
            };
            let request = ForumCommandRequest {
                authorization_id: Some(proxy.authorization_id.as_uuid()),
                ..request
            };
            let request_hash =
                RequestHash::from_request("lower_proxy", lot_id.as_uuid(), &request)?;
            runtime
                .commands
                .lower_proxy(LowerProxyCommand {
                    command_id: CommandId::from_uuid(command_id),
                    lot_id,
                    bidder_id,
                    authorization_id: proxy.authorization_id,
                    new_maximum: amount,
                    request_hash,
                })
                .await
                .map(|_| ())
        }
        _ => Err(repository_error("论坛命令类型不在闭集内")),
    }
}

async fn finish(
    runtime: &ForumReplyRuntime,
    command_id: Uuid,
    status: &str,
    error_code: &str,
    detail: &str,
) -> Result<(), ApplicationError> {
    runtime
        .store
        .finish_command(command_id, status.to_owned(), error_code.to_owned(), detail.to_owned())
        .await
}

#[derive(Debug, Serialize)]
struct ForumCommandRequest<'a> {
    command_kind: &'a str,
    amount: i64,
    mode: Option<&'a str>,
    authorization_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct ForumNotificationRequest<'a> {
    auction_id: Uuid,
    lot_id: Uuid,
    lot_code: &'a str,
    mode: &'a str,
    thread_id: &'a str,
    post_id: &'a str,
    line: usize,
}

fn forum_command_id(
    auction_id: auction_domain::AuctionId,
    thread_id: &str,
    post_id: &str,
    line_number: i32,
    lot_code: &str,
    command_kind: &str,
) -> Uuid {
    let name = format!(
        "auction/forum-command/v2/{auction_id}/{thread_id}/{post_id}/{line_number}/{lot_code}/{command_kind}"
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes())
}

const fn proxy_mode_name(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

fn is_retryable(error: &ApplicationError) -> bool {
    matches!(
        error,
        ApplicationError::Repository(_)
            | ApplicationError::CompetitionTransaction(
                CompetitionTransactionError::Failed(_)
                    | CompetitionTransactionError::Conflict
                    | CompetitionTransactionError::SerializationConflict,
            )
    )
}

fn bounded_detail(value: String) -> String {
    let detail: String = value.chars().take(512).collect();
    if detail.is_empty() { "论坛竞价命令被拒绝".to_owned() } else { detail }
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}
