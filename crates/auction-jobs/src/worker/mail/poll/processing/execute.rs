//! 单封邮件的处理执行：在租约内调用处理器并落定结果。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::mail::{
    MailPollBatch, MailPollClaim, MailPollCompletion, MailPollProgressStage,
};
use auction_observability::WorkerResult;

use super::business::{MailProcessFailure, process_with_retry};
use super::consignment::{CandidateStep, handle_candidate};
use super::diagnostics;
use super::fetch::fetch_with_retry;
use crate::worker::diagnostics as worker_diagnostics;
use crate::worker::mail::poll::MailPollFetchRequest;
use crate::worker::mail::poll::cycle::MailPollProcessContext;
use crate::worker::mail::poll::failure::{
    MailPollFailureContext, MailPollMetricContext, record_failure, record_failure_with_prefix,
    safe_failure_code, safe_failure_detail,
};
use crate::worker::mail::poll::mapping::{
    intake_from_message, intake_subject, pending_decision, valid_page,
};

pub(super) async fn process_claim_inner<C>(
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
        consignment,
        diagnostics,
        progress_observer,
        clock,
        world_id,
        mailbox_key: _,
        config,
        schedule,
        metrics,
        external_metrics,
        loop_kind,
    } = context;
    let batch_deadline = std::time::Instant::now() + config.batch_timeout;
    let metric_context = MailPollMetricContext {
        metrics: Arc::clone(&metrics),
        external_metrics: Arc::clone(&external_metrics),
        loop_kind,
    };
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
        super::super::types::observe_budget(
            &world_id,
            external_metrics.as_ref(),
            auction_application::external_metrics::MailPhase::PollBatch,
            auction_application::external_metrics::MailBudget::Time,
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
            super::super::types::observe_budget(
                &world_id,
                external_metrics.as_ref(),
                auction_application::external_metrics::MailPhase::PollBatch,
                auction_application::external_metrics::MailBudget::Time,
            );
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
            super::super::types::observe_budget(
                &world_id,
                external_metrics.as_ref(),
                auction_application::external_metrics::MailPhase::PollBatch,
                auction_application::external_metrics::MailBudget::Time,
            );
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
                super::super::types::observe_budget(
                    &world_id,
                    external_metrics.as_ref(),
                    auction_application::external_metrics::MailPhase::PollBatch,
                    auction_application::external_metrics::MailBudget::Time,
                );
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
        // 在业务 envelope 释放附件 key 前生成安全主题。分类与协议匹配继续使用原主题。
        let subject = intake_subject(&message);
        if let Some(candidate) = message.consignment_candidate.take() {
            // 本批在候选之前完成、但尚未随批次提交的旧信照原路径先提交；claim::commit
            // 会释放租约，所以本轮到此结束，候选留到下一轮由自己的 deposit 事务推进到它。
            // 只被 lookup_completion 跳过的旧信无需先提交：候选事务的前缀 CAS 会一并越过。
            if !items.is_empty() {
                let batch = MailPollBatch {
                    mailbox_key: claim.mailbox_key.clone(),
                    claim_token: claim.claim_token,
                    next_cursor: last_completed_external_mail_id,
                    polled_at,
                    next_poll_at,
                    items,
                };
                if let Err(error) = store.commit(claim.clone(), batch).await {
                    worker_diagnostics::error(
                        "mail_poll",
                        "commit_before_consignment",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    record_failure(
                        store,
                        clock,
                        claim,
                        "commit_failed".to_owned(),
                        "MoogleMail 候选前的入站批次未能确认提交".to_owned(),
                        failure_context,
                    )
                    .await;
                    return WorkerResult::Error;
                }
                return WorkerResult::Success;
            }
            let (Some(processor), Some(service)) = (processor.as_ref(), consignment.as_ref())
            else {
                record_failure(
                    store,
                    clock,
                    claim,
                    "consignment_unavailable".to_owned(),
                    "寄售候选已到达但寄售处理或旧业务处理器未装配".to_owned(),
                    failure_context,
                )
                .await;
                return WorkerResult::Error;
            };
            let Some(business) = message.business.take() else {
                record_failure(
                    store,
                    clock,
                    claim,
                    "business_payload_missing".to_owned(),
                    "MoogleMail 网关未提供寄售候选业务 envelope".to_owned(),
                    failure_context,
                )
                .await;
                return WorkerResult::Error;
            };
            if batch_deadline.checked_duration_since(std::time::Instant::now()).is_none() {
                super::super::types::observe_budget(
                    &world_id,
                    external_metrics.as_ref(),
                    auction_application::external_metrics::MailPhase::PollBatch,
                    auction_application::external_metrics::MailBudget::Time,
                );
                record_failure(
                    store,
                    clock,
                    claim,
                    "batch_timeout".to_owned(),
                    "MoogleMail 邮箱批次预算已耗尽，游标停在候选之前".to_owned(),
                    failure_context,
                )
                .await;
                return WorkerResult::Error;
            }
            if let Some(observer) = &progress_observer {
                observer.update(
                    &claim,
                    MailPollProgressStage::Committing,
                    Some(&candidate.external_mail_id),
                );
            }
            let diagnostic =
                diagnostics::MailMessageDiagnostic::prepare(diagnostics.clone(), &business);
            match handle_candidate(
                service.as_ref(),
                processor.as_ref(),
                business,
                candidate,
                &claim,
                polled_at,
                next_poll_at,
                batch_deadline,
                diagnostic,
                config,
                metric_context.clone(),
            )
            .await
            {
                CandidateStep::Committed => return WorkerResult::Success,
                CandidateStep::Legacy(decision) => {
                    let external_mail_id = message.external_mail_id.clone();
                    items.push(intake_from_message(
                        &claim.mailbox_key,
                        message,
                        decision,
                        polled_at,
                        subject,
                    ));
                    last_completed_external_mail_id = Some(external_mail_id);
                    continue;
                }
                CandidateStep::Failed(failure) => {
                    record_failure(
                        store,
                        clock,
                        claim,
                        failure.failure_code,
                        failure.safe_detail,
                        failure_context,
                    )
                    .await;
                    return WorkerResult::Error;
                }
            }
        }
        let Some(process_remaining) =
            batch_deadline.checked_duration_since(std::time::Instant::now())
        else {
            super::super::types::observe_budget(
                &world_id,
                external_metrics.as_ref(),
                auction_application::external_metrics::MailPhase::PollBatch,
                auction_application::external_metrics::MailBudget::Time,
            );
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
                // 业务结果及其超时已冻结；留证不占后续邮件的业务预算，也不更改重试分类。
                let (processed, timed_out) = match tokio::time::timeout(
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
                .await
                {
                    Ok(result) => (result, false),
                    Err(error) => {
                        super::super::types::observe_budget(
                            &world_id,
                            external_metrics.as_ref(),
                            auction_application::external_metrics::MailPhase::PollBatch,
                            auction_application::external_metrics::MailBudget::Time,
                        );
                        worker_diagnostics::warn(
                            "mail_poll",
                            "process_batch_timeout",
                            worker_diagnostics::timeout_kind(&error),
                            DiagnosticContext::default(),
                        );
                        (
                            Err(MailProcessFailure {
                                failure_code: "batch_processing_timeout",
                                safe_detail: "MoogleMail 单封邮件处理在总时限内未完成",
                            }),
                            true,
                        )
                    }
                };
                match diagnostics::finalize_legacy_result(
                    diagnostic,
                    processed,
                    pending_diagnostics,
                    decision_diagnostic,
                )
                .await
                {
                    Ok(decision) => decision,
                    Err(failure) => {
                        if timed_out {
                            budget_exhausted = true;
                        } else {
                            suffix_failure = Some((
                                failure.failure_code.to_owned(),
                                failure.safe_detail.to_owned(),
                            ));
                        }
                        break;
                    }
                }
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
        items.push(intake_from_message(&claim.mailbox_key, message, decision, polled_at, subject));
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
