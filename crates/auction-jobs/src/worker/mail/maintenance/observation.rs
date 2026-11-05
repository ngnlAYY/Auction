//! Read Mail 的纯计数观察；增量轮只统计水位之后的行，完整校验轮统计全部历史。
//! 计数不是已读或收取证明，恢复定位另由 reader 的已知 MID 路径按需执行。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError, diagnostic_world};
use auction_observability::WorkerResult;

use super::super::MailPollRuntime;
use crate::worker::diagnostics;

/// 返回本轮扫描到的已读行数；不创建历史入站命令，也不把未找到升级为业务证明。
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
                "MoogleMail Read Mail 共享观察完成（增量轮只含水位之后的行）"
            );
            WorkerResult::Success
        }
        Ok(Err(error)) if tokio::time::Instant::now() >= deadline => {
            runtime.observe_budget(
                auction_application::MailPhase::ReadObservation,
                auction_application::MailBudget::Time,
            );
            diagnostics::unpersisted_error(
                "mail_observation",
                "read_mail_deadline",
                error.diagnostic_kind(),
                DiagnosticContext {
                    world: diagnostic_world(&runtime.world_id),
                    ..Default::default()
                },
            );

            WorkerResult::Error
        }
        Ok(Err(error)) => {
            diagnostics::unpersisted_error(
                "mail_observation",
                "read_mail",
                error.diagnostic_kind(),
                DiagnosticContext {
                    world: diagnostic_world(&runtime.world_id),
                    ..Default::default()
                },
            );
            WorkerResult::Error
        }
        Err(error) => {
            runtime.observe_budget(
                auction_application::MailPhase::ReadObservation,
                auction_application::MailBudget::Time,
            );
            diagnostics::unpersisted_error(
                "mail_observation",
                "read_mail_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext {
                    world: diagnostic_world(&runtime.world_id),
                    ..Default::default()
                },
            );

            WorkerResult::Error
        }
    }
}
