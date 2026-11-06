//! 将数据库已完成 MID 与 Worker 运行观测组合成可选安全投影；不增加任何写入。

use auction_application::admin::AdminMailPollState;
use auction_application::mail::{MailPollProgressSnapshot, is_safe_mail_poll_id};
use auction_application::operations::{WorkerControlCondition, WorkerControlMailProgress};
use auction_contracts::{
    AdminMailPollProgressFailureResponse, AdminMailPollProgressResponse,
    AdminMailPollingDiagnosticsResponse,
};
use chrono::Utc;

use super::{AdminApiState, timestamp};

/// 只在 Admin 显式请求诊断时读取 control；不可用时仍返回数据库侧的安全已完成 MID。
pub(super) async fn read_polling_diagnostics(
    state: &AdminApiState,
    polling: &AdminMailPollState,
) -> AdminMailPollingDiagnosticsResponse {
    let mut worker = match state.worker_control() {
        Some(port) => port.mail_progress().await.unwrap_or_else(|_| {
            WorkerControlMailProgress::unavailable(WorkerControlCondition::Unavailable)
        }),
        None => WorkerControlMailProgress::unavailable(WorkerControlCondition::Unavailable),
    };
    if !worker.validate() {
        worker = WorkerControlMailProgress::unavailable(WorkerControlCondition::Incompatible);
    }
    // 原始游标继续留在服务端；只有严格的正十进制 MID 可以成为浏览器诊断值。
    let (cursor_status, completed_external_mail_id) = match polling.cursor_value.as_deref() {
        None | Some("") => ("empty", None),
        Some(value) if is_safe_mail_poll_id(value) => ("available", Some(value.to_owned())),
        Some(_) => ("hidden", None),
    };
    let progress = worker
        .pollers
        .into_iter()
        .find(|progress| progress.mailbox_key == polling.mailbox_key)
        .map(map_progress);
    AdminMailPollingDiagnosticsResponse {
        observed_at: timestamp(Utc::now()),
        cursor_status: cursor_status.to_owned(),
        completed_external_mail_id,
        worker_condition: worker.condition.as_str().to_owned(),
        worker_observed_at: worker.observed_at.map(timestamp),
        progress,
    }
}

fn map_progress(progress: MailPollProgressSnapshot) -> AdminMailPollProgressResponse {
    AdminMailPollProgressResponse {
        mailbox_key: progress.mailbox_key,
        world_id: progress.world_id,
        stage: progress.stage.as_str().to_owned(),
        external_mail_id: progress.external_mail_id,
        updated_at: timestamp(progress.updated_at),
        last_failure: progress.last_failure.map(|failure| AdminMailPollProgressFailureResponse {
            stage: failure.stage.as_str().to_owned(),
            external_mail_id: failure.external_mail_id,
            occurred_at: timestamp(failure.occurred_at),
        }),
    }
}
