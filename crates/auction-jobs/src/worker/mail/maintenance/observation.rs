//! 完整 Read Mail 的纯计数观察；恢复定位另由原 reader 路径按需执行。

use auction_observability::WorkerResult;

use super::super::MailPollRuntime;

/// 只返回完整已读列表的数量；不创建历史入站命令，也不把未找到升级为业务证明。
pub(in crate::worker::runtime::mail_poll) async fn run_read_observation(
    runtime: &MailPollRuntime,
) -> WorkerResult {
    let Some(maintenance) = &runtime.maintenance else { return WorkerResult::Success };
    let deadline = tokio::time::Instant::now() + runtime.config.fetch_timeout;
    match tokio::time::timeout_at(
        deadline,
        maintenance.observe_read_mail(&runtime.world_id, deadline.into_std()),
    )
    .await
    {
        Ok(Ok(observed)) => {
            tracing::info!(
                world_id = %runtime.world_id,
                read_observed = observed,
                "MoogleMail Read Mail 共享观察完成"
            );
            WorkerResult::Success
        }
        Ok(Err(error)) => {
            tracing::warn!(
                world_id = %runtime.world_id,
                error = %error,
                "MoogleMail Read Mail 观察失败；不撤销其它页面处理"
            );
            WorkerResult::Error
        }
        Err(_) => {
            tracing::warn!(
                world_id = %runtime.world_id,
                "MoogleMail Read Mail 观察预算耗尽；不撤销其它页面处理"
            );
            WorkerResult::Error
        }
    }
}
