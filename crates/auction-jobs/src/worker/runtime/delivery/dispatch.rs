//! Delivery 单个 claim 的执行编排：drain 复核、只读准备、sending fence、外部发送、
//! 租约续期与终态落库；外部结果未知时按安全失败语义持久化，不凭本地超时推断远端未写入。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::delivery::{
    DeliverySendResult, DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
use auction_application::operations::ExternalWriteActivity;
use auction_observability::WorkerResult;

use crate::WorkerControlHandle;
use crate::worker::diagnostics;
use crate::worker::runtime::DeliveryWorkerRuntimeConfig;

use super::handoff::{confirm_delivery_handoff, prepare_delivery, release_unstarted_delivery};
use super::lease::DeliveryLease;
use super::notification;
use super::report::DeliveryTaskResult;

#[tracing::instrument(

    skip_all,

    fields(

        delivery_id = %claim.delivery_id,

    )

)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn process_delivery_claim<C>(
    delivery_store: Arc<dyn DeliveryWorkStore>,
    delivery_transport: Arc<dyn DeliveryTransport>,
    clock: Arc<C>,
    config: DeliveryWorkerRuntimeConfig,
    control: WorkerControlHandle,
    notifications: Arc<dyn auction_application::notification_metrics::NotificationRecorder>,
    claim: DeliveryWorkClaim,
    activity: &ExternalWriteActivity,
) -> DeliveryTaskResult
where
    C: Clock + 'static,
{
    // 领取后派生的任务可能尚未获得调度；首次 poll 前再次确认 drain。
    if control.is_draining() {
        if activity.enabled() {
            return DeliveryTaskResult::completed(WorkerResult::Canceled);
        }
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Success,
            None,
        )
        .await;
    }
    // 通知指标只依赖冻结载荷与创建时间；在 claim 被移入各终态调用前取值。
    let notification_kind = notification::kind_of(&claim);
    let notification_created_at = claim.created_at;
    let mut lease = match DeliveryLease::new(&claim, clock.as_ref(), config) {
        Ok(lease) => lease,
        Err(failure_code) => {
            tracing::warn!(delivery_id = %claim.delivery_id, %failure_code, "delivery claim has no usable lease");
            if activity.is_cancelled() || (activity.enabled() && control.is_draining()) {
                return DeliveryTaskResult::completed(WorkerResult::Canceled);
            }
            return release_unstarted_delivery(
                delivery_store.as_ref(),
                clock.as_ref(),
                config,
                &claim,
                WorkerResult::Error,
                Some(SafeErrorKind::LEASE_EXPIRED),
            )
            .await;
        }
    };
    // prepare 总预算包含续租等待；其返回句柄持有同 Cookie guard，在交接完成前绝不 poll。
    let preparation_deadline = tokio::time::Instant::now() + config.request_timeout;
    let mut prepared = match tokio::time::timeout_at(
        preparation_deadline,
        prepare_delivery(
            delivery_store.as_ref(),
            delivery_transport.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            &mut lease,
            preparation_deadline,
        ),
    )
    .await
    {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) if error.failure_code == "invalid_payload" => {
            // 确定的本地载荷错误未执行任何写入；不能伪造 Sending，也不能永久重领坏任务。
            let completed_at = clock.now();
            let result = tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.reject_preparation(claim, completed_at),
            )
            .await;
            return DeliveryTaskResult::completed(if matches!(result, Ok(Ok(()))) {
                notification::record_failed(
                    notifications.as_ref(),
                    notification_kind,
                    "invalid_payload",
                    &serde_json::json!({"stage": "preflight_blocked"}),
                    notification::latency(notification_created_at, completed_at),
                );
                WorkerResult::Success
            } else {
                diagnostics::unpersisted(SafeErrorKind::REPOSITORY_FAILED);
                WorkerResult::Error
            });
        }
        preparation => {
            let preflight_kind = if preparation.is_err() {
                SafeErrorKind::TIMEOUT
            } else {
                SafeErrorKind::DEPENDENCY_UNAVAILABLE
            };
            let failure_code = match preparation {
                Ok(Err(error)) => error.failure_code,
                _ => "preflight_timeout".to_owned(),
            };
            tracing::warn!(delivery_id = %claim.delivery_id, %failure_code, "delivery preflight deferred before sending fence");
            if activity.is_cancelled() || (activity.enabled() && control.is_draining()) {
                return DeliveryTaskResult::completed(WorkerResult::Canceled);
            }
            return release_unstarted_delivery(
                delivery_store.as_ref(),
                clock.as_ref(),
                config,
                &claim,
                WorkerResult::Success,
                Some(preflight_kind),
            )
            .await;
        }
    };
    // prepare 可能恰在续租前完成；先补足 TTL，再有界提交 fence，最后确认 sending 租约。
    // 任一步骤不明确成功都丢弃未 poll 的句柄。fence 提交未知不能释放成 pending。
    if !confirm_delivery_handoff(
        delivery_store.as_ref(),
        clock.as_ref(),
        config,
        &claim,
        &mut lease,
        &mut prepared,
        activity,
    )
    .await
    {
        drop(prepared);
        if activity.is_cancelled() || (activity.enabled() && control.is_draining()) {
            return DeliveryTaskResult::completed(WorkerResult::Canceled);
        }
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Error,
            Some(SafeErrorKind::DEPENDENCY_UNAVAILABLE),
        )
        .await;
    }
    let mut send = prepared.into_send();
    let request_timeout = tokio::time::sleep(config.request_timeout);
    let request_deadline = request_timeout.deadline();
    tokio::pin!(request_timeout);
    let mut lease_failed = false;
    let mut send_polled = false;
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut request_timeout => {
                break Err(delivery_gateway_outcome_unknown(
                    "request_timeout",
                    "delivery 外部请求超时，远端结果未知",
                ));
            }
            _ = tokio::time::sleep_until(lease.deadline) => {
                lease_failed = true;
                break Err(delivery_gateway_outcome_unknown(
                    "lease_expired",
                    "delivery 发送期间 lease 已过期",
                ));
            }
            result = std::future::poll_fn(|context| {
                // 截止优先；在有效租约内先给操作一次 poll，再处理已到期的续租。
                // 慢续租回执可能晚于 renew_at，不能让续租分支持续独占循环。
                let now = tokio::time::Instant::now();
                if now >= lease.deadline || now >= request_deadline {
                    return std::task::Poll::Pending;
                }
                send_polled = true;
                send.as_mut().poll(context)
            }) => break result,
            _ = tokio::time::sleep_until(lease.renew_at) => {
                // 续租等待也纳入发送总预算，不能因数据库缓慢无限延长外部请求。
                let renewal = tokio::time::timeout_at(
                    request_deadline,
                    lease.renew(delivery_store.as_ref(), &claim, clock.as_ref(), config),
                )
                .await;
                let stage = match renewal {
                    Ok(Ok(())) => continue,
                    Ok(Err(stage)) => stage,
                    Err(error) => {
                        diagnostics::warn(
                            "delivery",
                            "lease_request_timeout",
                            diagnostics::timeout_kind(&error),
                            DiagnosticContext::default(),
                        );
                        "request_timeout"
                    }
                };
                lease_failed = true;
                break Err(delivery_gateway_outcome_unknown(
                    stage,
                    "delivery 发送期间 lease 续租失败或超时",
                ));
            }
        }
    };
    drop(send);
    if !send_polled {
        // fence 后尚未 poll 也可能失去租约；不伪造外部 unknown，sending 本身仍保守隔离。
        if activity.is_cancelled() || (activity.enabled() && control.is_draining()) {
            return DeliveryTaskResult::completed(WorkerResult::Canceled);
        }
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Error,
            Some(SafeErrorKind::LEASE_EXPIRED),
        )
        .await;
    }
    let delivery_id = claim.delivery_id;
    let result = result.and_then(normalize_delivery_send_result);
    match result {
        Ok(result) => {
            let completed_at = clock.now();
            match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.succeed(claim, result, completed_at),
            )
            .await
            {
                Ok(Ok(())) => {
                    notification::record_sent(
                        notifications.as_ref(),
                        notification_kind,
                        notification::latency(notification_created_at, completed_at),
                    );
                    activity.complete();
                    DeliveryTaskResult::completed(if lease_failed {
                        WorkerResult::Error
                    } else {
                        WorkerResult::Success
                    })
                }
                Ok(Err(error)) => {
                    diagnostics::error(
                        "delivery",
                        "persist_completion",
                        error.diagnostic_kind(),
                        DiagnosticContext {
                            disposition: Some(DiagnosticDisposition::Unpersisted),
                            delivery_id: diagnostic_id(&delivery_id),
                            ..Default::default()
                        },
                    );
                    DeliveryTaskResult::completed(WorkerResult::Error)
                }
                Err(error) => {
                    diagnostics::unpersisted_error(
                        "delivery",
                        "persist_completion_timeout",
                        diagnostics::timeout_kind(&error),
                        DiagnosticContext {
                            delivery_id: diagnostic_id(&delivery_id),
                            ..Default::default()
                        },
                    );

                    DeliveryTaskResult::completed(WorkerResult::Error)
                }
            }
        }
        Err(error) => {
            // 只有确定的收件人缺失才需要批次排除项；成功和其它失败不复制收件人/world。
            let skipped_recipient = if error.failure_code == "recipient_not_found" {
                claim.payload.get("world").and_then(serde_json::Value::as_str).map(|world| {
                    (world.trim().to_ascii_lowercase(), claim.recipient.trim().to_ascii_lowercase())
                })
            } else {
                None
            };
            let failure_code = error.failure_code.clone();
            let failure_metadata = error.result_metadata.clone();
            let completed_at = clock.now();
            let worker_result = match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.fail(claim, error, completed_at),
            )
            .await
            {
                Ok(Ok(())) => {
                    notification::record_failed(
                        notifications.as_ref(),
                        notification_kind,
                        &failure_code,
                        &failure_metadata,
                        notification::latency(notification_created_at, completed_at),
                    );
                    activity.complete();
                    if lease_failed { WorkerResult::Error } else { WorkerResult::Success }
                }
                Ok(Err(persist_error)) => {
                    diagnostics::error(
                        "delivery",
                        "persist_send_failure",
                        persist_error.diagnostic_kind(),
                        DiagnosticContext {
                            disposition: Some(DiagnosticDisposition::Unpersisted),
                            delivery_id: diagnostic_id(&delivery_id),
                            ..Default::default()
                        },
                    );
                    WorkerResult::Error
                }
                Err(error) => {
                    diagnostics::unpersisted_error(
                        "delivery",
                        "persist_send_failure_timeout",
                        diagnostics::timeout_kind(&error),
                        DiagnosticContext {
                            delivery_id: diagnostic_id(&delivery_id),
                            ..Default::default()
                        },
                    );

                    WorkerResult::Error
                }
            };
            DeliveryTaskResult { worker_result, skipped_recipient, deferred_world: None }
        }
    }
}

fn normalize_delivery_send_result(
    mut result: DeliverySendResult,
) -> Result<DeliverySendResult, DeliveryTransportError> {
    if result.external_reference.is_empty()
        || result.external_reference != result.external_reference.trim()
    {
        return Err(DeliveryTransportError {
            failure_code: "invalid_gateway_result".to_owned(),
            safe_detail: "delivery gateway 成功结果无效，远端结果未知".to_owned(),
            outcome_unknown: true,
            external_reference: None,
            result_metadata: serde_json::json!({
                "category": "invalid_result",
            }),
        });
    }
    // 旧版会把非对象 metadata 规范化为空对象后再进入成功分支；保持该边界，避免
    // 传输适配器的无关 metadata 形状改变 delivery 的最终 phase。
    if !result.result_metadata.is_object() {
        result.result_metadata = serde_json::json!({});
    }
    Ok(result)
}

fn delivery_gateway_outcome_unknown(
    stage: &'static str,
    safe_detail: &'static str,
) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "gateway_outcome_unknown".to_owned(),
        safe_detail: safe_detail.to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: serde_json::json!({
            "category": "gateway_error",
            "stage": stage,
        }),
    }
}
