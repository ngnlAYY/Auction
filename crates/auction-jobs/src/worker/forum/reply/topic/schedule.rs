//! 下一轮主题扫描时刻：Final 阶段按最早有效截止收紧读取间隔。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_id};

use super::super::ForumReplyRuntime;
use super::super::completion::repository_error;
use crate::worker::diagnostics;

/// Final 阶段的最短导入间隔；领取调用失败后的重试等待也以它为下限。
pub(in crate::worker::forum::reply) const FINAL_SCAN_MINIMUM_INTERVAL_SECONDS: i64 = 5;
/// Final 阶段按最早 lot 有效截止剩余时间收紧间隔的除数。
const FINAL_SCAN_REMAINING_DIVISOR: i32 = 6;

pub(super) async fn completed_scan_times(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::forum::ForumReplyTopic,
    clock: &dyn auction_application::core::Clock,
    poll_interval: chrono::Duration,
) -> Result<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>), ApplicationError> {
    let completed_at = clock.now();
    let next_scan_at = next_scan_at(runtime, topic, completed_at, poll_interval).await?;
    Ok((completed_at, next_scan_at))
}

/// 计算下一次导入时刻；非 Final 阶段或没有有效截止时使用常规轮询间隔。
pub(super) async fn next_scan_at(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::forum::ForumReplyTopic,
    completed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    let interval = if topic.reply_phase == auction_application::forum::ForumReplyPhase::Final {
        match runtime.store.earliest_live_lot_deadline(topic.auction_id).await? {
            Some(earliest_deadline) => {
                final_scan_interval(completed_at, earliest_deadline, poll_interval)
            }
            None => poll_interval,
        }
    } else {
        poll_interval
    };
    completed_at
        .checked_add_signed(interval)
        .ok_or_else(|| repository_error("论坛下一次扫描时间超出可表示范围"))
}

/// 失败重试或同轮重复领取后的下一次扫描时刻；`final_cadence` 由调用方按阶段与错误码判定。
///
/// Final 阶段沿用与成功扫描相同的收紧节奏：一次可重试失败（例如竞价序列化冲突留下的
/// pending 账本）或同轮重复领取，都不能把截止前的回帖推迟到常规轮询之后，否则导入时已过
/// 截止会按 `auction_closed` 拒绝。截止读取失败时退回常规轮询；这里只改变读取节奏，不改变
/// 失败码、未知写入门禁、命令身份或 pending 顺序。时刻不可表示时返回 `None`，调用方失败关闭。
pub(in crate::worker::forum::reply) async fn retry_scan_at(
    runtime: &ForumReplyRuntime,
    topic: &auction_application::forum::ForumReplyTopic,
    failed_at: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
    final_cadence: bool,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if !final_cadence {
        return failed_at.checked_add_signed(poll_interval);
    }
    match next_scan_at(runtime, topic, failed_at, poll_interval).await {
        Ok(next_scan_at) => Some(next_scan_at),
        Err(error) => {
            diagnostics::warn(
                "forum_reply",
                "read_final_retry_deadline",
                error.diagnostic_kind(),
                DiagnosticContext {
                    auction_id: diagnostic_id(&topic.auction_id),
                    ..Default::default()
                },
            );
            failed_at.checked_add_signed(poll_interval)
        }
    }
}

/// Final 阶段把导入间隔收紧到“最早 lot 有效截止剩余时间 / 6”，
/// 并夹在最短间隔与常规轮询间隔之间。截止已过时固定为最短间隔，
/// 直到拍卖离开 live/final；这里只改变读取节奏，不改变任何裁决时刻。
fn final_scan_interval(
    completed_at: chrono::DateTime<chrono::Utc>,
    earliest_deadline: chrono::DateTime<chrono::Utc>,
    poll_interval: chrono::Duration,
) -> chrono::Duration {
    let minimum = chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS);
    if poll_interval <= minimum {
        return poll_interval;
    }
    // 负数（已过截止）也按最短间隔处理，不能给远端负延迟。
    let remaining = earliest_deadline
        .signed_duration_since(completed_at)
        .checked_div(FINAL_SCAN_REMAINING_DIVISOR)
        .unwrap_or(minimum);
    if remaining < minimum {
        minimum
    } else if remaining > poll_interval {
        poll_interval
    } else {
        remaining
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_scan_interval_tightens_near_earliest_deadline() {
        let completed_at = chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00Z")
            .unwrap_or_else(|error| panic!("时间应合法: {error:?}"))
            .with_timezone(&chrono::Utc);
        let poll = chrono::Duration::minutes(5);

        // 剩余 60 分钟时仍受常规轮询间隔限制。
        let far = completed_at + chrono::Duration::minutes(60);
        assert_eq!(final_scan_interval(completed_at, far, poll), poll);

        // 剩余 60 秒时按 1/6 收紧到 10 秒。
        let near = completed_at + chrono::Duration::seconds(60);
        assert_eq!(final_scan_interval(completed_at, near, poll), chrono::Duration::seconds(10));

        // 剩余不足 30 秒时收敛到最短间隔，而不是继续线性缩小。
        let imminent = completed_at + chrono::Duration::seconds(12);
        assert_eq!(
            final_scan_interval(completed_at, imminent, poll),
            chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS)
        );

        // 截止已过仍保持最短间隔，等到拍卖离开 live/final。
        let past = completed_at - chrono::Duration::minutes(3);
        assert_eq!(
            final_scan_interval(completed_at, past, poll),
            chrono::Duration::seconds(FINAL_SCAN_MINIMUM_INTERVAL_SECONDS)
        );
    }

    #[test]
    fn final_scan_interval_never_exceeds_poll_interval() {
        let completed_at = chrono::DateTime::parse_from_rfc3339("2026-11-04T10:00:00Z")
            .unwrap_or_else(|error| panic!("时间应合法: {error:?}"))
            .with_timezone(&chrono::Utc);
        // 轮询间隔本身低于最短间隔时不放大读取频率。
        let short_poll = chrono::Duration::seconds(3);
        let near = completed_at + chrono::Duration::seconds(6);
        assert_eq!(final_scan_interval(completed_at, near, short_poll), short_poll);
    }
}
