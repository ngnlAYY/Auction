//! MoogleMail Worker 的 claim/续租、批次处理和提交编排；lease fencing 与
//! cursor/事实批次提交顺序由本模块保持。

use std::sync::Arc;

use auction_application::diagnostics::{DiagnosticContext, DiagnosticDisposition, DiagnosticError};
use auction_application::{
    Clock, MailMessage, MailPollBatch, MailPollClaim, MailPollCompletion, MailPollProgressStage,
    MailProcessingDecision, MailProcessor,
};
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::cycle::MailPollProcessContext;
use super::failure::{
    MailPollFailureContext, MailPollMetricContext, record_failure, record_failure_with_prefix,
    retry_delay, safe_failure_code, safe_failure_detail,
};
use super::mapping::{intake_from_message, pending_decision, valid_page};
use super::{
    MailPollFetchRequest, MailPollGateway, MailPollGatewayError, MailPollPage,
    MailPollRuntimeConfig,
};
use crate::worker::diagnostics as worker_diagnostics;

mod business;
mod diagnostics;
mod fetch;
mod progress;

use business::process_with_retry;
use fetch::fetch_with_retry;
use progress::MailPollProgressGuard;

/// 在整封邮件处理期间维护数据库租约；续租失败时立即取消本地 future，并由 fence 阻止陈旧提交。
pub(super) async fn process_claim<C>(
    context: MailPollProcessContext<C>,
    claim: MailPollClaim,
    mut lease_deadline: tokio::time::Instant,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if !claim.validate() || claim.mailbox_key != context.mailbox_key {
        tracing::error!("MoogleMail Worker 收到无效租约");
        return WorkerResult::Error;
    }
    // 调用方在领取前冻结单调截止；返回租约只能收紧它，不能补回数据库/网络耗时。
    let observed_at = tokio::time::Instant::now();
    let Some(observed_deadline) = (claim.lease_until - context.clock.now())
        .to_std()
        .ok()
        .and_then(|remaining| observed_at.checked_add(remaining))
    else {
        tracing::error!("MoogleMail Worker 收到已过期或不可表示的租约");
        return WorkerResult::Error;
    };
    lease_deadline = lease_deadline.min(observed_deadline);
    let lease_span = context.config.lease_duration;
    let Ok(lease_duration) = ChronoDuration::from_std(context.config.lease_duration) else {
        return WorkerResult::Error;
    };
    let store = Arc::clone(&context.store);
    let clock = Arc::clone(&context.clock);
    let renew_timeout = context.config.renew_interval;
    let mut renewal_claim = claim.clone();
    let mut progress = MailPollProgressGuard::new(
        context.progress_observer.clone(),
        claim.clone(),
        &context.world_id,
    );
    let work = process_claim_inner(context, claim);
    tokio::pin!(work);

    loop {
        let renewal_sleep = tokio::time::sleep(renew_timeout);
        tokio::pin!(renewal_sleep);
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(lease_deadline) => {
                tracing::error!(
                    mailbox_key = %renewal_claim.mailbox_key,
                    "MoogleMail 租约已过期，取消当前陈旧处理"
                );
                return WorkerResult::Error;
            }
            result = std::future::poll_fn(|context| {
                if tokio::time::Instant::now() >= lease_deadline {
                    return std::task::Poll::Pending;
                }
                work.as_mut().poll(context)
            }) => {
                progress.finish(&result);
                return result;
            },
            () = &mut renewal_sleep => {
                let renewal_started = tokio::time::Instant::now();
                if renewal_started >= lease_deadline {
                    tracing::error!(
                        mailbox_key = %renewal_claim.mailbox_key,
                        "MoogleMail 续租开始前租约已过期"
                    );
                    return WorkerResult::Error;
                }
                let renewal_deadline = lease_deadline.min(renewal_started + renew_timeout);
                let renewed_deadline = renewal_started + lease_span;
                let now = clock.now();
                let Some(lease_until) = now.checked_add_signed(lease_duration) else {
                    tracing::error!(mailbox_key = %renewal_claim.mailbox_key, "MoogleMail 续租截止时间溢出");
                    return WorkerResult::Error;
                };
                // 新期限锚定续租发起时刻；等待和回执都不能越过已知旧租约。
                match tokio::time::timeout_at(
                    renewal_deadline,
                    store.renew(renewal_claim.clone(), now, lease_until),
                )
                .await
                {
                    Ok(Ok(updated)) => {
                        if tokio::time::Instant::now() >= renewal_deadline
                            || tokio::time::Instant::now() >= renewed_deadline
                            || clock.now() >= updated.lease_until
                            || updated.lease_until < lease_until
                        {
                            tracing::error!(
                                mailbox_key = %renewal_claim.mailbox_key,
                                "MoogleMail 续租未在原租约内确认有效新期限"
                            );
                            return WorkerResult::Error;
                        }
                        renewal_claim = updated;
                        lease_deadline = renewed_deadline;
                    }
                    Ok(Err(error)) => {
                        worker_diagnostics::error(
                            "mail_poll",
                            "lease_renew",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                ..Default::default()
                            },
                        );
                        return WorkerResult::Error;
                    }
                    Err(error) => {
                        worker_diagnostics::unpersisted_error(
                            "mail_poll",
                            "lease_renew_timeout",
                            worker_diagnostics::timeout_kind(&error),
                            DiagnosticContext::default(),
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
        diagnostics,
        progress_observer,
        clock,
        world_id,
        mailbox_key: _,
        config,
        schedule,
        metrics,
        loop_kind,
    } = context;
    let mut batch_deadline = std::time::Instant::now() + config.batch_timeout;
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
        deadline: None,
    };
    let polled_at = clock.now();
    let Some(fetch_remaining) = batch_deadline.checked_duration_since(std::time::Instant::now())
    else {
        record_failure(
            store,
            clock,
            claim,
            "batch_timeout".to_owned(),
            "MoogleMail 邮箱批次预算已耗尽，游标保持不变".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    };
    let page = match tokio::time::timeout(
        fetch_remaining,
        fetch_with_retry(gateway.as_ref(), request, claim.clone(), config, metric_context.clone()),
    )
    .await
    {
        Ok(Ok(page)) => page,
        Ok(Err(error)) => {
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
        Err(error) => {
            worker_diagnostics::warn(
                "mail_poll",
                "fetch_batch_timeout",
                worker_diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );

            record_failure(
                store,
                clock,
                claim,
                "batch_timeout".to_owned(),
                "MoogleMail 邮箱批次预算已耗尽，游标保持不变".to_owned(),
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
    let mut budget_exhausted = false;
    let mut suffix_failure = None;
    let mut last_completed_external_mail_id = None;
    for mut message in page.messages {
        if let Some(observer) = &progress_observer {
            observer.update(
                &claim,
                MailPollProgressStage::Processing,
                Some(&message.external_mail_id),
            );
        }
        let Some(lookup_remaining) =
            batch_deadline.checked_duration_since(std::time::Instant::now())
        else {
            budget_exhausted = true;
            break;
        };
        let completion = tokio::time::timeout(
            lookup_remaining,
            store.lookup_completion(
                claim.mailbox_key.clone(),
                message.world_id.clone(),
                message.external_mail_id.clone(),
                message.received_at,
            ),
        )
        .await;
        match completion {
            Err(error) => {
                worker_diagnostics::warn(
                    "mail_poll",
                    "lookup_completion_timeout",
                    worker_diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                budget_exhausted = true;
                break;
            }
            Ok(Ok(MailPollCompletion::Completed)) => {
                // 已有完整入站事实和命令账本时，远端 mutable digest 不得触发再次路由。
                last_completed_external_mail_id = Some(message.external_mail_id);
                continue;
            }
            Ok(Ok(MailPollCompletion::Missing)) => {}
            Ok(Err(error)) => {
                worker_diagnostics::error(
                    "mail_poll",
                    "lookup_completion",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                suffix_failure = Some((
                    "completion_lookup_failed".to_owned(),
                    "MoogleMail 已完成事实核验失败，未越过失败邮件".to_owned(),
                ));
                break;
            }
        }
        let Some(process_remaining) =
            batch_deadline.checked_duration_since(std::time::Instant::now())
        else {
            budget_exhausted = true;
            break;
        };
        let decision = match (processor.as_ref(), message.business.take()) {
            (Some(processor), Some(business)) => {
                let diagnostic =
                    diagnostics::MailMessageDiagnostic::prepare(diagnostics.clone(), &business);
                let mut pending_diagnostics =
                    Vec::with_capacity(usize::from(config.process_attempts));
                let mut decision_diagnostic = None;
                let mut processed = tokio::time::timeout(
                    process_remaining,
                    process_with_retry(
                        processor.as_ref(),
                        business,
                        config,
                        metric_context.clone(),
                        &mut pending_diagnostics,
                        &mut decision_diagnostic,
                    ),
                )
                .await;
                // 业务结果及其超时已冻结；留证不占后续邮件的业务预算，也不更改重试分类。
                if !pending_diagnostics.is_empty() {
                    let retention_started = std::time::Instant::now();
                    for (index, pending) in pending_diagnostics.into_iter().enumerate() {
                        let diagnostic = pending.resolve().await;
                        if decision_diagnostic == Some(index)
                            && let Ok(Ok(decision)) = &mut processed
                            && decision.mail_kind == "equipment"
                            && decision.status == "rejected"
                            && diagnostic.validate()
                        {
                            decision.equipment_diagnostic = Some(diagnostic);
                        }
                    }
                    if let Some(extended) = batch_deadline.checked_add(retention_started.elapsed())
                    {
                        batch_deadline = extended;
                    } else {
                        tracing::warn!("MoogleMail diagnostic budget extension overflow");
                    }
                }
                let decision = match processed {
                    Ok(Ok(decision)) => decision,
                    Ok(Err(failure)) => {
                        diagnostic.retain_failure(failure.failure_code).await;
                        suffix_failure =
                            Some((failure.failure_code.to_owned(), failure.safe_detail.to_owned()));
                        break;
                    }
                    Err(error) => {
                        worker_diagnostics::warn(
                            "mail_poll",
                            "process_batch_timeout",
                            worker_diagnostics::timeout_kind(&error),
                            DiagnosticContext::default(),
                        );

                        diagnostic.retain_failure("batch_processing_timeout").await;
                        budget_exhausted = true;
                        break;
                    }
                };
                if decision.validate().is_err() {
                    diagnostic.retain_failure("invalid_business_decision").await;
                    suffix_failure = Some((
                        "invalid_business_decision".to_owned(),
                        "MoogleMail 业务处理器返回了无效决定".to_owned(),
                    ));
                    break;
                }
                diagnostic.retain_decision(decision).await
            }
            (Some(_), None) => {
                suffix_failure = Some((
                    "business_payload_missing".to_owned(),
                    "MoogleMail 网关未提供业务 envelope，未越过失败邮件".to_owned(),
                ));
                break;
            }
            (None, Some(_)) | (None, None) => pending_decision(&message),
        };
        let external_mail_id = message.external_mail_id.clone();
        items.push(intake_from_message(&claim.mailbox_key, message, decision, polled_at));
        last_completed_external_mail_id = Some(external_mail_id);
    }
    if let Some((code, detail)) = suffix_failure {
        // 只允许明确完成的连续前缀推进游标；失败时绝不使用整页 next_cursor。
        let prefix = last_completed_external_mail_id.map(|cursor| MailPollBatch {
            mailbox_key: claim.mailbox_key.clone(),
            claim_token: claim.claim_token,
            next_cursor: Some(cursor),
            polled_at,
            next_poll_at,
            items,
        });
        record_failure_with_prefix(store, clock, claim, code, detail, failure_context, prefix)
            .await;
        return WorkerResult::Error;
    }
    if budget_exhausted && last_completed_external_mail_id.is_none() {
        record_failure(
            store,
            clock,
            claim,
            "batch_timeout".to_owned(),
            "MoogleMail 邮箱批次预算已耗尽，未完成任何邮件".to_owned(),
            failure_context,
        )
        .await;
        return WorkerResult::Error;
    }
    let batch = MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        // cursor 由最后一封已完成邮件推进；预算耗尽时未完成后缀不进入本批次，下一轮从
        // 持久待读队列优先恢复；已完成前缀由入站/命令账本幂等校验防止重复副作用。
        next_cursor: last_completed_external_mail_id.or(page.next_cursor),
        polled_at,
        next_poll_at,
        items,
    };
    if let Some(observer) = &progress_observer {
        observer.update(&claim, MailPollProgressStage::Committing, None);
    }
    if let Err(error) = store.commit(claim.clone(), batch).await {
        worker_diagnostics::error(
            "mail_poll",
            "commit_batch",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
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
