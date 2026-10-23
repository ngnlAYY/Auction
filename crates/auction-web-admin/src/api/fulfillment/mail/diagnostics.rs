//! 邮件诊断扩展的闭集校验；不接受原始游标或跨邮箱的迟到观测。

use auction_contracts::{AdminMailPollProgressResponse, AdminMailPollingDiagnosticsResponse};

use super::parse_admin_time;

fn safe_mid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && matches!(value.as_bytes().first(), Some(b'1'..=b'9'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn mail_stage(stage: &str) -> bool {
    matches!(stage, "reading_detail" | "processing")
}

fn active_stage(stage: &str) -> bool {
    matches!(stage, "fetching" | "reading_detail" | "processing" | "committing")
}

fn safe_target(stage: &str, mid: Option<&str>) -> bool {
    mid.is_none_or(safe_mid) && (mail_stage(stage) || mid.is_none())
}

pub(super) fn valid_diagnostics(
    value: &AdminMailPollingDiagnosticsResponse,
    mailbox: &str,
) -> bool {
    let Some(observed_at) = parse_admin_time(&value.observed_at) else { return false };
    let cursor_valid = match value.cursor_status.as_str() {
        "available" => value.completed_external_mail_id.as_deref().is_some_and(safe_mid),
        "empty" | "hidden" => value.completed_external_mail_id.is_none(),
        _ => false,
    };
    if !cursor_valid {
        return false;
    }
    if value.worker_condition != "available" {
        return matches!(
            value.worker_condition.as_str(),
            "unavailable" | "timeout" | "incompatible" | "rejected"
        ) && value.worker_observed_at.is_none()
            && value.progress.is_none();
    }
    let Some(worker_at) = value.worker_observed_at.as_deref().and_then(parse_admin_time) else {
        return false;
    };
    if worker_at > observed_at {
        return false;
    }
    value.progress.as_ref().is_none_or(|progress| valid_progress(progress, mailbox, worker_at))
}

fn valid_progress(value: &AdminMailPollProgressResponse, mailbox: &str, observed_at: f64) -> bool {
    value.mailbox_key == mailbox
        && matches!(value.world_id.as_str(), "main" | "isekai")
        && (active_stage(&value.stage)
            || matches!(value.stage.as_str(), "idle" | "failed" | "canceled"))
        && safe_target(&value.stage, value.external_mail_id.as_deref())
        && parse_admin_time(&value.updated_at).is_some_and(|updated| updated <= observed_at)
        && value.last_failure.as_ref().is_none_or(|failure| {
            active_stage(&failure.stage)
                && safe_target(&failure.stage, failure.external_mail_id.as_deref())
                && parse_admin_time(&failure.occurred_at)
                    .is_some_and(|occurred| occurred <= observed_at)
        })
}
