//! MoogleMail 远程读取的有界超时、临时错误重试和指标记录。

use auction_application::core::diagnostics::DiagnosticContext;
use auction_observability::WorkerRetryKind;

use crate::worker::diagnostics;
use crate::worker::mail::poll::failure::{MailPollMetricContext, retry_delay};
use crate::worker::mail::poll::{
    MailPollFetchRequest, MailPollGateway, MailPollGatewayError, MailPollPage,
    MailPollRuntimeConfig,
};
use auction_application::mail::MailPollClaim;

pub(super) async fn fetch_with_retry(
    gateway: &dyn MailPollGateway,
    request: MailPollFetchRequest,
    claim: MailPollClaim,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> Result<MailPollPage, MailPollGatewayError> {
    let world_id = request.world_id.clone();
    for attempt in 1..=config.fetch_attempts {
        let deadline =
            std::time::Instant::now().checked_add(config.fetch_timeout).ok_or_else(|| {
                MailPollGatewayError {
                    failure_code: "fetch_timeout".to_owned(),
                    safe_detail: "MoogleMail 远程读取预算无效".to_owned(),
                    temporary: true,
                }
            })?;
        let request = MailPollFetchRequest { deadline: Some(deadline), ..request.clone() };
        let result = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            gateway.fetch_claimed(request, claim.clone()),
        )
        .await;
        match result {
            Ok(Ok(page)) => return Ok(page),
            Ok(Err(error)) if error.temporary && attempt < config.fetch_attempts => {
                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Ok(Err(error)) => return Err(error),
            Err(error) if attempt < config.fetch_attempts => {
                super::super::types::observe_budget(
                    &world_id,
                    metric_context.external_metrics.as_ref(),
                    auction_application::external_metrics::MailPhase::PollFetch,
                    auction_application::external_metrics::MailBudget::Time,
                );
                diagnostics::warn(
                    "mail_poll",
                    "retry_fetch_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Err(error) => {
                super::super::types::observe_budget(
                    &world_id,
                    metric_context.external_metrics.as_ref(),
                    auction_application::external_metrics::MailPhase::PollFetch,
                    auction_application::external_metrics::MailBudget::Time,
                );
                diagnostics::warn(
                    "mail_poll",
                    "fetch_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                return Err(MailPollGatewayError {
                    failure_code: "fetch_timeout".to_owned(),
                    safe_detail: "MoogleMail 远程读取在总时限内未完成".to_owned(),
                    temporary: true,
                });
            }
        }
    }
    Err(MailPollGatewayError {
        failure_code: "fetch_failed".to_owned(),
        safe_detail: "MoogleMail 远程读取未完成".to_owned(),
        temporary: true,
    })
}
