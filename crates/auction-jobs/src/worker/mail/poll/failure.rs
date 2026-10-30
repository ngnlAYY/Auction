//! MoogleMail Worker 的失败分类、退避和指标上下文。

use std::sync::Arc;

use auction_application::{Clock, MailPollBatch, MailPollClaim, MailPollStore, RecurringSchedule};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerRetryKind};
use chrono::Duration as ChronoDuration;

use super::MailPollRuntimeConfig;

pub(super) async fn record_failure<C>(
    store: Arc<dyn MailPollStore>,
    clock: Arc<C>,
    claim: MailPollClaim,
    failure_code: String,
    safe_detail: String,
    context: MailPollFailureContext,
) where
    C: Clock + 'static,
{
    record_failure_with_prefix(store, clock, claim, failure_code, safe_detail, context, None).await;
}

/// 失败后缀不能抹掉已完成前缀；同一 fenced 事务持久化前缀与退避。
pub(super) async fn record_failure_with_prefix<C>(
    store: Arc<dyn MailPollStore>,
    clock: Arc<C>,
    claim: MailPollClaim,
    failure_code: String,
    safe_detail: String,
    context: MailPollFailureContext,
    prefix: Option<MailPollBatch>,
) where
    C: Clock + 'static,
{
    let MailPollFailureContext { schedule, config, metric_context } = context;
    metric_context.metrics.observe_retry(metric_context.loop_kind, WorkerRetryKind::Persisted);
    let now = clock.now();
    let delay = retry_delay(config.retry_base, config.retry_maximum, claim.attempt_count);
    let Ok(delay) = ChronoDuration::from_std(delay) else {
        tracing::error!("MoogleMail Worker 失败退避时长超出时间表示范围，保留当前 claim");
        return;
    };
    let Some(backoff_at) = now.checked_add_signed(delay) else {
        tracing::error!("MoogleMail Worker 失败退避时间溢出，保留当前 claim");
        return;
    };
    let retry_at =
        schedule.next_after(now).map_or(backoff_at, |scheduled_at| scheduled_at.max(backoff_at));
    tracing::warn!(
        mailbox_key = %claim.mailbox_key,
        attempt_count = claim.attempt_count,
        failure_code = %failure_code,
        retry_at = %retry_at,
        "MoogleMail mailbox processing failed"
    );
    let result = match prefix {
        Some(batch) => {
            store.fail_after_prefix(claim, batch, retry_at, failure_code, safe_detail).await
        }
        None => store.fail(claim, retry_at, failure_code, safe_detail).await,
    };
    if let Err(error) = result {
        tracing::error!(error = %error, "MoogleMail Worker 保存失败退避状态失败");
    }
}

pub(super) fn retry_delay(
    base: std::time::Duration,
    maximum: std::time::Duration,
    failed_attempt: u32,
) -> std::time::Duration {
    let exponent = failed_attempt.saturating_sub(1).min(31);
    let factor = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(maximum)
}

pub(super) fn safe_failure_code(value: &str) -> String {
    if auction_domain::valid_text(value, 128) {
        value.to_owned()
    } else {
        "gateway_failed".to_owned()
    }
}

pub(super) fn safe_failure_detail(value: &str) -> String {
    if auction_domain::valid_text(value, 512) {
        value.to_owned()
    } else {
        "MoogleMail gateway 返回了无效的安全错误分类".to_owned()
    }
}

#[derive(Clone)]
pub(super) struct MailPollMetricContext {
    pub(super) metrics: Arc<dyn WorkerRecorder>,
    pub(super) loop_kind: WorkerLoop,
}

#[derive(Clone)]
pub(super) struct MailPollFailureContext {
    pub(super) schedule: Arc<dyn RecurringSchedule>,
    pub(super) config: MailPollRuntimeConfig,
    pub(super) metric_context: MailPollMetricContext,
}
