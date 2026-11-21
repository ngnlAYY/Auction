//! 主题扫描失败的持久处置：失败码收敛、未知写入门禁与重试排期。

use auction_application::core::ApplicationError;
use auction_application::forum::{ForumReplyPhase, ForumReplyTopic, ForumWriteProgress};
use auction_time::Timestamp;

use super::ForumReplyRuntime;
use crate::worker::forum::reply::topic::schedule::retry_scan_at;
use crate::worker::runtime::WorkerError;

/// 按应用错误收敛失败码后持久化；返回 Final 收紧节奏下的重试时刻，供论坛循环提前唤醒。
pub(super) async fn record_topic_failure(
    runtime: &ForumReplyRuntime,
    topic: ForumReplyTopic,
    failed_at: Timestamp,
    poll_interval: auction_time::Duration,
    error: &ApplicationError,
    write_progress: &ForumWriteProgress,
) -> Result<Option<Timestamp>, WorkerError> {
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
        write_progress,
    )
    .await
}

/// 释放 claim 并记录失败码与重试时刻；返回值只在采用 Final 收紧节奏时为 `Some`。
pub(super) async fn record_topic_failure_code(
    runtime: &ForumReplyRuntime,
    topic: ForumReplyTopic,
    failed_at: Timestamp,
    poll_interval: auction_time::Duration,
    failure_code: &str,
    write_progress: &ForumWriteProgress,
) -> Result<Option<Timestamp>, WorkerError> {
    if write_progress.is_cancelled() {
        return Ok(None);
    }
    // 历史未知写入不能因后续 GET/数据库错误被覆盖成可重试状态，必须等待原有回读证明。
    let failure_code = if write_progress.has_unconfirmed_write()
        || topic.content_last_error_code.as_deref() == Some("write_unknown")
    {
        "write_unknown"
    } else {
        failure_code
    };
    // 不变量：只调整重试排期，失败码与未知写入门禁不变。未知写入维持常规轮询，
    // 只读对账不随 Final 放大；其余 Final 失败按收紧节奏重读，避免截止前回帖迟到。
    let final_cadence =
        topic.reply_phase == ForumReplyPhase::Final && failure_code != "write_unknown";
    let retry_at = retry_scan_at(runtime, &topic, failed_at, poll_interval, final_cadence)
        .await
        .ok_or(WorkerError::InvalidForumConfiguration)?;
    runtime.store.fail_topic_scan(topic, failed_at, retry_at, failure_code.to_owned()).await?;
    write_progress.confirm_write();
    Ok(final_cadence.then_some(retry_at))
}
