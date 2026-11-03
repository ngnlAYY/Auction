//! Sent/Take 恢复与 compose 清理的单次预算边界；复用原应用服务及持久围栏。

use auction_observability::WorkerResult;

use super::super::MailPollRuntime;

/// 一次维护只建立一个截止点；超时或取消保留未知外写围栏，不授权自动重发。
pub(in crate::worker::runtime::mail_poll) async fn run_maintenance(
    runtime: &MailPollRuntime,
) -> WorkerResult {
    let Some(maintenance) = &runtime.maintenance else { return WorkerResult::Success };
    // DB、凭据排队、列表/详情及 compose/discard 全部消耗原截止点，不能逐操作重置。
    let deadline = tokio::time::Instant::now() + runtime.config.fetch_timeout;
    let result = tokio::time::timeout_at(
        deadline,
        maintenance.sweep(&runtime.world_id, deadline.into_std()),
    )
    .await;
    match result {
        Ok(Ok(report)) => {
            tracing::info!(
                world_id = %runtime.world_id,
                compose_clean = report.compose_clean,
                compose_error = ?report.compose_error,
                compose_matches = report.compose_matches,
                discarded = report.discarded,
                matched_delivery = ?report.matched_delivery,
                sent_candidates = report.sent_candidates,
                sent_matched = report.sent_matched,
                sent_state_failed = report.sent_state_failed,
                sent_state_error = ?report.sent_state_error,
                sent_observed = report.sent_observed,
                sent_recovered = report.sent_recovered,
                sweep_requeued = report.sweep_requeued,
                "MoogleMail 共享读取维护完成"
            );
            // 服务可能返回含局部错误的报告；阶段指标据此记错，不把部分失败计成成功。
            if report.compose_error.is_some() || report.sent_state_error.is_some() {
                WorkerResult::Error
            } else {
                WorkerResult::Success
            }
        }
        Err(_) => {
            tracing::warn!(
                world_id = %runtime.world_id,
                "MoogleMail 维护总预算耗尽；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
            WorkerResult::Error
        }
        Ok(Err(_)) if tokio::time::Instant::now() >= deadline => {
            tracing::warn!(
                world_id = %runtime.world_id,
                "MoogleMail 维护总预算耗尽；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
            WorkerResult::Error
        }
        Ok(Err(error)) => {
            // 维护失败不撤销已获证据；原游标、租约和未知状态仍由持久端口恢复。
            tracing::warn!(
                error = %error,
                world_id = %runtime.world_id,
                "MoogleMail 共享读取维护失败；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
            WorkerResult::Error
        }
    }
}
