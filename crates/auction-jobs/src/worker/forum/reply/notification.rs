//! W14.4：论坛 `notify` 改为订阅该 lot 的被超提醒，不再寄导入回执邮件。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::forum::NotifySubscription;

use super::*;
use crate::worker::diagnostics;

pub(super) async fn process_notification(
    runtime: &ForumReplyRuntime,
    context: &ForumCommandContext<'_>,
    post_number: i32,
) -> Result<(), ApplicationError> {
    let topic = context.topic;
    let bidder = context.bidder;
    let post_id = context.post_id;
    let lot_code = context.lot_code;
    let command = context.command;
    let lot_id = context.lot_id;
    // 模式仍参与冻结请求身份，但不决定渠道：订阅按账号提醒渠道发送。
    let mode = command.notify.as_deref().unwrap_or("both");
    if !matches!(mode, "mm" | "pm" | "both") {
        tracing::warn!(post_id, "论坛通知模式不在闭集内");
        return Ok(());
    }
    let Some(line) = i32::try_from(command.line).ok() else {
        tracing::warn!(post_id, "论坛通知行号超出持久化范围");
        return Ok(());
    };
    let proposed_command_id =
        forum_command_id(topic.auction_id, &topic.thread_id, post_id, line, lot_code, "notify");
    let record = ForumBidCommandRecord {
        command_id: proposed_command_id,
        auction_id: topic.auction_id,
        world: topic.world,
        thread_id: topic.thread_id.clone(),
        post_id: post_id.to_owned(),
        post_number,
        line_number: line,
        lot_code: lot_code.to_owned(),
        actor: bidder.external_account_id.clone(),
        command_kind: "notify".to_owned(),
        amount: 0,
    };
    let start = match runtime.store.begin_command(record).await {
        Ok(value) => value,
        Err(error) => {
            diagnostics::error(
                "forum_notification",
                "begin_command",
                error.diagnostic_kind(),
                DiagnosticContext {
                    command_id: auction_application::core::diagnostics::diagnostic_id(
                        &proposed_command_id,
                    ),
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
        return Ok(());
    }
    // 同一命令身份下重复扫描不会重复标记；结果只写命令账本与系统日志楼。
    match runtime
        .store
        .subscribe_notify(
            command_id,
            topic.auction_id,
            lot_id,
            bidder.user_id,
            post_id.to_owned(),
            lot_code.to_owned(),
        )
        .await
    {
        Ok(NotifySubscription::Subscribed) => {
            finish(runtime, command_id, "applied", "", "已订阅该 lot 的被超提醒").await?;
        }
        Ok(NotifySubscription::AlreadySubscribed) => {
            finish(runtime, command_id, "applied", "", "该 lot 已在关注列表中").await?;
        }
        Ok(NotifySubscription::Capped(reason)) => {
            let detail = match reason {
                "post_limit" => "本楼 notify 订阅数已达上限",
                _ => "本场关注列表已达上限",
            };
            finish(runtime, command_id, "rejected", "subscription_cap", detail).await?;
        }
        Err(error) if is_retryable(&error) => {
            diagnostics::warn(
                "forum_notification",
                "retry_command",
                error.diagnostic_kind(),
                DiagnosticContext {
                    command_id: auction_application::core::diagnostics::diagnostic_id(&command_id),
                    ..Default::default()
                },
            );
            return Err(error);
        }
        Err(error) => {
            let detail = bounded_detail(error.to_string());
            finish(runtime, command_id, "rejected", "subscription_failed", &detail).await?;
        }
    }
    Ok(())
}
