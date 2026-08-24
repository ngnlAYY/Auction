//! MoogleMail Worker 的租约处理、远程读取和业务重试。

use std::sync::Arc;

use auction_application::{
    Clock, MailMessage, MailPollBatch, MailPollClaim, MailProcessingDecision, MailProcessor,
};
use auction_observability::{WorkerResult, WorkerRetryKind};
use chrono::Duration as ChronoDuration;

use super::cycle::MailPollProcessContext;
use super::failure::{
    MailPollFailureContext, MailPollMetricContext, record_failure, retry_delay, safe_failure_code,
    safe_failure_detail,
};
use super::mapping::{intake_from_message, pending_decision, valid_page};
use super::{
    MailPollFetchRequest, MailPollGateway, MailPollGatewayError, MailPollPage,
    MailPollRuntimeConfig,
};

/// 在整封邮件处理期间维护数据库租约；续租失败时立即取消本地 future，并由 fence 阻止陈旧提交。
pub(super) async fn process_claim<C>(
    context: MailPollProcessContext<C>,
    claim: MailPollClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if !claim.validate() || claim.mailbox_key != context.mailbox_key {
        tracing::error!("MoogleMail Worker 收到无效租约");
        return WorkerResult::Error;
    }
    let Ok(lease_duration) = ChronoDuration::from_std(context.config.lease_duration) else {
        return WorkerResult::Error;
    };
    let store = Arc::clone(&context.store);
    let clock = Arc::clone(&context.clock);
    let renew_timeout = context.config.renew_interval;
    let mut renewal_claim = claim.clone();
    let work = process_claim_inner(context, claim);
    tokio::pin!(work);

    loop {
        let renewal_sleep = tokio::time::sleep(renew_timeout);
        tokio::pin!(renewal_sleep);
        tokio::select! {
            result = &mut work => return result,
            () = &mut renewal_sleep => {
                let now = clock.now();
                let Some(lease_until) = now.checked_add_signed(lease_duration) else {
                    tracing::error!(mailbox_key = %renewal_claim.mailbox_key, "MoogleMail 续租截止时间溢出");
                    return WorkerResult::Error;
                };
                match tokio::time::timeout(
                    renew_timeout,
                    store.renew(renewal_claim.clone(), now, lease_until),
                )
                .await
                {
                    Ok(Ok(updated)) => renewal_claim = updated,
                    Ok(Err(error)) => {
                        tracing::error!(
                            error = %error,
                            mailbox_key = %renewal_claim.mailbox_key,
                            "MoogleMail 租约续期失败，取消当前陈旧处理"
                        );
                        return WorkerResult::Error;
                    }
                    Err(_) => {
                        tracing::error!(
                            mailbox_key = %renewal_claim.mailbox_key,
                            "MoogleMail 租约续期超时，取消当前陈旧处理"
                        );
                        return WorkerResult::Error;
                    }
                }
            }
        }
    }
}

async fn process_claim_inner<C>(
    context: MailPollProcessContext<C>,
    claim: MailPollClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let MailPollProcessContext {
        store,
        gateway,
        processor,
        clock,
        world_id,
        mailbox_key: _,
        config,
        schedule,
        metrics,
        loop_kind,
    } = context;
    let metric_context = MailPollMetricContext { metrics: Arc::clone(&metrics), loop_kind };
    let failure_context = MailPollFailureContext {
        schedule: Arc::clone(&schedule),
        config,
        metric_context: metric_context.clone(),
    };
    let request = MailPollFetchRequest {
        mailbox_key: claim.mailbox_key.clone(),
        world_id: world_id.clone(),
        cursor_value: claim.cursor_value.clone(),
        limit: config.batch_size,
    };
    let polled_at = clock.now();
    let page =
        match fetch_with_retry(gateway.as_ref(), request, config, metric_context.clone()).await {
            Ok(page) => page,
            Err(error) => {
                record_failure(
                    store,
                    clock,
                    claim,
                    safe_failure_code(&error.failure_code),
                    safe_failure_detail(&error.safe_detail),
                    failure_context,
                )
                .await;
                return WorkerResult::Error;
            }
        };
    if !valid_page(&page, &claim, &world_id, config.batch_size) {
        record_failure(
            store,
            clock,
            claim,
            "invalid_gateway_page".to_owned(),
            "MoogleMail gateway 返回的批次不符合安全边界".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    }
    metrics.observe_batch(loop_kind, page.messages.len(), usize::from(config.batch_size));
    let Some(next_poll_at) = schedule.next_after(polled_at) else {
        record_failure(
            Arc::clone(&store),
            Arc::clone(&clock),
            claim,
            "schedule_failed".to_owned(),
            "MoogleMail cron 无法计算下一次触发时刻".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    };
    let mut items = Vec::with_capacity(page.messages.len());
    for mut message in page.messages {
        let decision = match (processor.as_ref(), message.business.take()) {
            (Some(processor), Some(business)) => {
                let decision = match process_with_retry(
                    processor.as_ref(),
                    business,
                    config,
                    metric_context.clone(),
                )
                .await
                {
                    Ok(decision) => decision,
                    Err(failure) => {
                        record_failure(
                            Arc::clone(&store),
                            Arc::clone(&clock),
                            claim,
                            failure.failure_code.to_owned(),
                            failure.safe_detail.to_owned(),
                            failure_context.clone(),
                        )
                        .await;
                        return WorkerResult::Error;
                    }
                };
                if decision.validate().is_err() {
                    record_failure(
                        Arc::clone(&store),
                        Arc::clone(&clock),
                        claim,
                        "invalid_business_decision".to_owned(),
                        "MoogleMail 业务处理器返回了无效决定".to_owned(),
                        failure_context.clone(),
                    )
                    .await;
                    return WorkerResult::Error;
                }
                decision
            }
            (Some(_), None) => {
                record_failure(
                    Arc::clone(&store),
                    Arc::clone(&clock),
                    claim,
                    "business_payload_missing".to_owned(),
                    "MoogleMail 网关未提供业务 envelope，入站游标保持不变".to_owned(),
                    failure_context.clone(),
                )
                .await;
                return WorkerResult::Error;
            }
            (None, Some(_)) | (None, None) => pending_decision(&message),
        };
        items.push(intake_from_message(&claim.mailbox_key, message, decision, polled_at));
    }
    let batch = MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        next_cursor: page.next_cursor,
        polled_at,
        next_poll_at,
        items,
    };
    if let Err(error) = store.commit(claim.clone(), batch).await {
        tracing::error!(
            error = %error,
            mailbox_key = %claim.mailbox_key,
            "MoogleMail Worker 提交入站批次失败"
        );
        record_failure(
            store,
            clock,
            claim,
            "commit_failed".to_owned(),
            "MoogleMail 入站批次未能确认提交".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    }
    WorkerResult::Success
}

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

pub(super) struct MailProcessFailure {
    failure_code: &'static str,
    safe_detail: &'static str,
}

pub(super) async fn process_with_retry(
    processor: &dyn MailProcessor,
    business: MailMessage,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
) -> Result<MailProcessingDecision, MailProcessFailure> {
    let mut original = Some(business);
    for attempt in 1..=config.process_attempts {
        // 非最终尝试只建立一个显式短生命周期副本；成功或失败返回时，未使用的原 envelope 会清零。
        let message = if attempt == config.process_attempts {
            let Some(message) = original.take() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message
        } else {
            let Some(message) = original.as_ref() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message.duplicate_for_retry()
        };
        let result = tokio::time::timeout(config.item_timeout, processor.process(message)).await;
        match result {
            Ok(Ok(decision)) => return Ok(decision),
            Ok(Err(_)) if attempt < config.process_attempts => {
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
            Ok(Err(_)) => {
                return Err(MailProcessFailure {
                    failure_code: "business_processing_failed",
                    safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
                });
            }
            Err(_) if attempt < config.process_attempts => {
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
                return Err(MailProcessFailure {
                    failure_code: "business_processing_timeout",
                    safe_detail: "MoogleMail 单封邮件处理在总时限内未完成",
                });
            }
        }
    }
    Err(MailProcessFailure {
        failure_code: "business_processing_failed",
        safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
    })
}
