//! 论坛命令的 MoogleMail 通知入队。

use super::*;

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
    let mode = command.notify.as_deref().unwrap_or("both");
    if mode == "pm" {
        tracing::warn!(post_id, "论坛 PM 通知 owner 未装配");
        return Ok(());
    }
    if mode == "both" {
        tracing::warn!(post_id, "论坛 PM 通知 owner 未装配，将继续处理 MoogleMail 通知");
    }
    if !matches!(mode, "mm" | "both") {
        tracing::warn!(post_id, "论坛通知模式不在闭集内");
        return Ok(());
    }
    let Some(notifier) = runtime.notifier.as_ref() else {
        // 旧版在 owner 未装配时直接跳过；此处不能先占用稳定 command_id 并留下永久拒绝事实。
        tracing::warn!(post_id, "论坛 MoogleMail 通知 owner 未装配");
        return Ok(());
    };
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
            tracing::error!(command_id = %proposed_command_id, error = %error, "论坛通知账本开始失败");
            return Err(error);
        }
    };
    let command_id = start.command_id;
    if !start.should_run {
        return Ok(());
    }
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
            world_id: topic.world.as_str().to_owned(),
            external_mail_id,
            recipient: bidder.player_name.clone(),
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
