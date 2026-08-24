//! 论坛竞价命令账本、应用命令和完成状态收口。

use super::*;

pub(super) async fn process_command(
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
                    request_id: None,
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
                    request_id: None,
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
                    request_id: None,
                })
                .await
                .map(|_| ())
        }
        _ => Err(repository_error("论坛命令类型不在闭集内")),
    }
}

#[derive(Debug, Serialize)]
struct ForumCommandRequest<'a> {
    command_kind: &'a str,
    amount: i64,
    mode: Option<&'a str>,
    authorization_id: Option<Uuid>,
}
