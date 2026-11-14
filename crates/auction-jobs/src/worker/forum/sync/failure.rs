//! 失败码、退避与人工对账升级规则。

use std::sync::Arc;

use auction_application::core::{ApplicationError, Clock, RepositoryError};
use auction_application::forum::{
    ForumRenderError, ForumSyncClaim, ForumSyncStore, ForumWriteProgress,
};
use chrono::Duration;

const RETRY_DELAY: Duration = Duration::minutes(1);
const MANUAL_RECONCILIATION_DELAY: Duration = Duration::days(365);

pub(super) async fn record_failure<C>(
    write_progress: &ForumWriteProgress,
    store: Arc<dyn ForumSyncStore>,
    clock: Arc<C>,
    claim: ForumSyncClaim,
    failure_code: String,
    error: ApplicationError,
) -> Result<(), ApplicationError>
where
    C: Clock + 'static,
{
    if write_progress.is_cancelled() {
        return Ok(());
    }
    let now = clock.now();
    let unscoped_code = failure_code.strip_prefix("reconcile_").unwrap_or(&failure_code);
    let delay = if matches!(unscoped_code, "write_unknown" | "publish_proof_failed") {
        // 带 reconcile_ 的未知结果同样只能 GET 对账，不能误用一年退避使已成功的编辑永久搁置。
        RETRY_DELAY
    } else if is_manual_failure(&error, &failure_code) {
        MANUAL_RECONCILIATION_DELAY
    } else {
        RETRY_DELAY
    };
    let Some(retry_at) = now.checked_add_signed(delay) else {
        // 不把人工对账或重试时间折回当前时刻，避免未知远端结果被自动再次提交。
        return Err(ApplicationError::Repository(RepositoryError::Failed(
            "论坛同步重试时间溢出".to_owned(),
        )));
    };
    let safe_detail = bounded_detail(error.to_string());
    store.fail(claim, retry_at, failure_code, safe_detail).await?;
    write_progress.confirm_write();
    Ok(())
}

pub(super) fn forum_failure_code(error: &ApplicationError) -> &'static str {
    match error {
        ApplicationError::ForumRemote(remote) => remote.code(),
        ApplicationError::CapabilityUnavailable(_) => "capability_unavailable",
        _ => "remote_failed",
    }
}

/// 创建主题失败的错误码：远端写入已经开始即结果未知，后续只能先做精确标题只读对账。
pub(super) fn create_failure_code(error: &ApplicationError) -> &'static str {
    if matches!(error, ApplicationError::ForumRemote(remote) if remote.write_started()) {
        "write_unknown"
    } else {
        forum_failure_code(error)
    }
}

/// 本地渲染失败的错误码；两类都是人工处置，不会自动重试远端写入。
pub(super) fn render_failure_code(error: ForumRenderError) -> &'static str {
    match error {
        ForumRenderError::InvalidSnapshot => "invalid_snapshot",
        ForumRenderError::BodyTooLarge => "body_too_large",
    }
}

fn is_manual_failure(error: &ApplicationError, failure_code: &str) -> bool {
    matches!(
        error,
        ApplicationError::ForumRemote(remote) if remote.write_started() || !remote.retryable()
    ) || matches!(
        failure_code.strip_prefix("reconcile_").unwrap_or(failure_code),
        "invalid_snapshot"
            | "body_too_large"
            | "equipment_links_failed"
            | "configuration"
            | "credential_unavailable"
            | "protocol_drift"
            | "fixed_layout_conflict"
            | "write_authorization_required"
            | "capability_unavailable"
    )
}

pub(super) fn requires_reconciliation(claim: &ForumSyncClaim) -> bool {
    claim.previous_failure_code.as_deref().is_some_and(|code| {
        matches!(code, "write_unknown" | "publish_proof_failed" | "write_retry_authorized")
            || code.starts_with("reconcile_")
    })
}

pub(super) fn scoped_failure_code(reconciliation_only: bool, failure_code: &str) -> String {
    if reconciliation_only && !failure_code.starts_with("reconcile_") {
        format!("reconcile_{failure_code}")
    } else {
        failure_code.to_owned()
    }
}

fn bounded_detail(value: String) -> String {
    let detail: String = value.chars().take(512).collect();
    if detail.is_empty() { "论坛同步失败".to_owned() } else { detail }
}

#[cfg(test)]
mod tests {
    use auction_application::core::ApplicationError;
    use auction_application::forum::{ForumRemoteError, ForumRenderError};

    use super::{create_failure_code, render_failure_code};

    #[test]
    fn render_failure_code_maps_each_render_error() {
        assert_eq!(render_failure_code(ForumRenderError::InvalidSnapshot), "invalid_snapshot");
        assert_eq!(render_failure_code(ForumRenderError::BodyTooLarge), "body_too_large");
    }

    #[test]
    fn create_failure_code_marks_started_writes_unknown() {
        let started = ApplicationError::from(ForumRemoteError::new("http_status", true, true));
        assert_eq!(create_failure_code(&started), "write_unknown");

        let before_write =
            ApplicationError::from(ForumRemoteError::new("http_status", true, false));
        assert_eq!(create_failure_code(&before_write), "http_status");
        assert_eq!(
            create_failure_code(&ApplicationError::CapabilityUnavailable("forum")),
            "capability_unavailable"
        );
        assert_eq!(
            create_failure_code(&ApplicationError::Repository(
                auction_application::core::RepositoryError::Failed("db".to_owned())
            )),
            "remote_failed"
        );
    }
}
