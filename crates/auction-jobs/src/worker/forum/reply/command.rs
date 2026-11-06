//! 论坛竞价命令账本、应用命令和完成状态收口。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};

use super::*;
use crate::worker::diagnostics;

pub(super) async fn process_command(
    runtime: &ForumReplyRuntime,
    context: &ForumCommandContext<'_>,
) -> Result<(), ApplicationError> {
    let topic = context.topic;
    let bidder = context.bidder;
    let post_id = context.post_id;
    let lot_code = context.lot_code;
    let command = context.command;
    let lot_id = context.lot_id;
    let command_kind = if command.kind == "proxy" && command.proxy_lower {
        "proxy_lower"
    } else {
        command.kind.as_str()
    };
    let Some(post_number) = i32::try_from(context.post_number).ok() else {
        tracing::warn!(post_id, "论坛楼层号超出持久化范围");
        return Ok(());
    };
    if !matches!(command_kind, "bid" | "proxy" | "proxy_lower") {
        if command_kind == "notify" {
            process_notification(runtime, context, post_number).await?;
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
    let proposed_command_id = forum_command_id(
        topic.auction_id,
        &topic.thread_id,
        post_id,
        line_number,
        lot_code,
        command_kind,
    );
    let record = ForumBidCommandRecord {
        command_id: proposed_command_id,
        auction_id: topic.auction_id,
        world: topic.world,
        thread_id: topic.thread_id.clone(),
        post_id: post_id.to_owned(),
        post_number,
        line_number,
        lot_code: lot_code.to_owned(),
        actor: bidder.external_account_id.clone(),
        command_kind: command_kind.to_owned(),
        amount,
    };
    let start = match runtime.store.begin_command(record).await {
        Ok(start) => start,
        Err(error) => {
            diagnostics::error(
                "forum_reply",
                "begin_command",
                error.diagnostic_kind(),
                DiagnosticContext {
                    command_id: diagnostic_id(&proposed_command_id),
                    ..Default::default()
                },
            );
            return Err(error);
        }
    };
    if start.is_new {
        runtime.reply_delay.observe_import(
            topic.auction_id,
            topic.world,
            topic.reply_phase,
            post_id,
            std::time::Instant::now(),
        );
    }
    let command_id = start.command_id;
    if !start.should_run {
        // 竞价命令已经完成时仍需重放安全的应用命令，以便恢复此前入账但通知失败的场景。
        // 应用端口以同一 command_id 返回既有结果，不会重复写入竞价事实。
        if start.status == ForumCommandStatus::Applied
            && command.notify.as_deref().is_some_and(|mode| !mode.is_empty())
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
            process_notification(runtime, context, post_number).await?;
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
            diagnostics::warn(
                "forum_reply",
                "retry_command",
                error.diagnostic_kind(),
                DiagnosticContext { command_id: diagnostic_id(&command_id), ..Default::default() },
            );
            return Err(error);
        }
        Err(error) => {
            let detail = bounded_detail(error.to_string());
            let code = super::rejection::rejection_code(&error);
            finish(runtime, command_id, "rejected", code, &detail).await?;
        }
    }
    if succeeded && command.notify.as_deref().is_some_and(|mode| !mode.is_empty()) {
        process_notification(runtime, context, post_number).await?;
    }
    Ok(())
}

async fn execute_command(
    runtime: &ForumReplyRuntime,
    command_kind: &str,
    command: &auction_application::admin::AdminForumCommand,
    lot_id: auction_domain::LotId,
    bidder_id: auction_domain::UserId,
    command_id: Uuid,
    amount: i64,
) -> Result<(), ApplicationError> {
    match command_kind {
        "bid" => {
            let amount = BidAmount::new(amount)?;
            let request_hash = PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount)?;
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
            let request_hash =
                CreateProxyCommand::legacy_request_hash(lot_id, bidder_id, mode, amount)?;
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
            let request_hash = LowerProxyCommand::legacy_request_hash(
                lot_id,
                bidder_id,
                proxy.authorization_id,
                amount,
            )?;
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
