//! MoogleMail 远程读取的有界超时、临时错误重试和指标记录。

use super::*;
use auction_observability::WorkerRetryKind;

pub(super) async fn fetch_with_retry(
    gateway: &dyn MailPollGateway,
    request: MailPollFetchRequest,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> Result<MailPollPage, MailPollGatewayError> {
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
            gateway.fetch(request),
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
            Err(_) if attempt < config.fetch_attempts => {
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
            Err(_) => {
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
