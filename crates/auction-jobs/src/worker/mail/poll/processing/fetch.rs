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
        let result =
            tokio::time::timeout(config.fetch_timeout, gateway.fetch(request.clone())).await;
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
