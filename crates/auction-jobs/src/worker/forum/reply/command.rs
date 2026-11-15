//! 论坛竞价命令账本、应用命令和完成状态收口。

use auction_application::auction::PlaceBidCommand;
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};
use auction_application::forum::{ForumBidCommandRecord, ForumCommandStatus};
use auction_domain::{BidAmount, CommandId};
use uuid::Uuid;

use super::completion::{bounded_detail, finish, forum_command_id, is_retryable, repository_error};
use super::notification::process_notification;
use super::{ForumCommandContext, ForumReplyRuntime};
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
    let opens_at_start = command.kind == "start";
    let (command_kind, amount) = ledger_command(command, context.start_price)?;
    let Some(post_number) = i32::try_from(context.post_number).ok() else {
        tracing::warn!(post_id, "论坛楼层号超出持久化范围");
        return Ok(());
    };
    // 论坛只接受普通出价（解析器丢弃代理行）；其余种类不改变竞价状态。
    if command_kind != "bid" {
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
    let Some(amount) = amount else {
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
            execute_command(runtime, command_kind, lot_id, bidder.user_id, command_id, amount)
                .await?;
            process_notification(runtime, context, post_number).await?;
        }
        return Ok(());
    }

    let result =
        execute_command(runtime, command_kind, lot_id, bidder.user_id, command_id, amount).await;
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
            let code = if opens_at_start { start_rejection_code(code) } else { code };
            finish(runtime, command_id, "rejected", code, &detail).await?;
        }
    }
    if succeeded && command.notify.as_deref().is_some_and(|mode| !mode.is_empty()) {
        process_notification(runtime, context, post_number).await?;
    }
    Ok(())
}

/// 把解析出的论坛命令映射为账本类型与金额；`None` 金额表示不改变竞价状态。
///
/// `start` 是按首口出价（起拍价向上取整到信用网格）的普通出价：账本、命令身份与竞价都按 `bid`
/// 记录，同一行只有一个动作，不会与真正的出价行冲突。“只有未被出价的 lot 能 start”由竞价事务内
/// 的最低价判定保证，这里不预读 lot 状态。
fn ledger_command(
    command: &auction_application::admin::AdminForumCommand,
    start_price: BidAmount,
) -> Result<(&str, Option<i64>), ApplicationError> {
    Ok(match command.kind.as_str() {
        "start" => ("bid", Some(BidAmount::opening_bid(start_price)?.value())),
        kind => (kind, command.amount),
    })
}

/// `start` 被最低价拒绝只说明 lot 已有出价，改用专门分类；其余拒绝分类原样保留。
fn start_rejection_code(code: &'static str) -> &'static str {
    if code == "bid_too_low" { "start_unavailable" } else { code }
}

async fn execute_command(
    runtime: &ForumReplyRuntime,
    command_kind: &str,
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
        _ => Err(repository_error("论坛命令类型不在闭集内")),
    }
}

#[cfg(test)]
mod tests {
    use auction_application::admin::AdminForumCommand;
    use auction_domain::BidAmount;

    use super::super::rejection::rejection_message;
    use super::{ledger_command, start_rejection_code};

    fn command(kind: &str, amount: Option<i64>) -> AdminForumCommand {
        AdminForumCommand {
            line: 1,
            text: String::new(),
            actor: None,
            lot_codes: vec!["One01".to_owned()],
            kind: kind.to_owned(),
            amount,
            notify: None,
        }
    }

    fn persisted(value: i64) -> BidAmount {
        BidAmount::from_persisted_positive(value).unwrap()
    }

    /// `start` 按首口出价记为普通出价（非整千历史起拍价向上取整）；其余命令保持原类型与金额。
    #[test]
    fn start_is_recorded_as_an_opening_bid() {
        let start = command("start", None);
        assert_eq!(ledger_command(&start, persisted(50_000)).unwrap(), ("bid", Some(50_000)));
        assert_eq!(ledger_command(&start, persisted(1_500)).unwrap(), ("bid", Some(2_000)));

        let bid = command("bid", Some(200_000));
        assert_eq!(ledger_command(&bid, persisted(50_000)).unwrap(), ("bid", Some(200_000)));
        let notify = command("notify", None);
        assert_eq!(ledger_command(&notify, persisted(50_000)).unwrap(), ("notify", None));
    }

    /// lot 已有出价时 `start` 被最低价拒绝，系统日志改用专门分类；其他拒绝原样保留。
    #[test]
    fn start_below_minimum_reports_that_the_lot_already_has_bids() {
        assert_eq!(start_rejection_code("bid_too_low"), "start_unavailable");
        // 新分类在系统日志白名单内，不回退为通用拒绝。
        assert_eq!(rejection_message(Some("start_unavailable"), None).0, "start_unavailable");
        assert_eq!(start_rejection_code("auction_closed"), "auction_closed");
        assert_eq!(start_rejection_code("already_leading"), "already_leading");
    }
}
