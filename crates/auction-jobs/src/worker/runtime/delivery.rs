//! Delivery outbox 的领取、外部发送、租约续期和终态落库；
//! 外部结果未知时保留安全失败语义并进入持久化对账，不能凭本地超时推断远端未写入。

use std::collections::HashSet;

use auction_application::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::{CriticalWriteKind, ExternalWriteActivity};

use super::*;
use crate::worker::diagnostics;

mod lease;
mod notification;
mod wakeup;
use lease::DeliveryLease;
pub(super) use wakeup::run as run_delivery_wakeup;

/// 以独立 cadence 和 bounded concurrency 处理 delivery outbox。
/// 只有获得空闲并发槽后才领取下一条记录，避免已经领取但尚未执行的任务在队列中耗尽 lease。
/// 凭据暂不可用时释放 claim 并在本轮排除对应世界，避免重复领取或阻塞其他世界。
pub(super) async fn run_delivery_cycle<C>(worker: &PersistentWorker<C>) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::Delivery;
    let body = async move {
        let (Some(delivery_store), Some(delivery_transport)) =
            (&worker.delivery_store, &worker.delivery_transport)
        else {
            return Ok(());
        };
        // W12.5：投递属于批量轮次，受共享连接配额限制；许可随本轮结束释放，等待在 runner 内。
        let _bulk_permit = worker.bulk_permit().await;
        let transport_concurrency = delivery_transport.maximum_concurrency();
        if transport_concurrency == 0 {
            return Err(WorkerError::InvalidDeliveryConfiguration);
        }
        // W1.9：暂停只阻止本轮开始新的领取与外部发送；已领取且越过 sending 围栏的写入
        // 仍在各自任务中按原预算完成，这里不取消任何在途任务。
        if worker.external_access_paused() {
            return Ok(());
        }
        // 本轮使用边界处的完整热快照；租约、续租和宽限仍来自启动配置。并发只由 transport
        // 决定（MoogleMail 为 1），再以 batch_limit 收口，不存在单独的“配置并发”上限。
        let delivery = worker.delivery_runtime();
        let effective_concurrency = usize::from(transport_concurrency.min(delivery.batch_limit));
        let delivery_started = std::time::Instant::now();
        let delivery_lease = ChronoDuration::from_std(delivery.lease_duration)
            .map_err(|_| WorkerError::InvalidDeliveryConfiguration)?;
        let mut delivery_count = 0usize;
        let mut delivery_result = WorkerResult::Success;
        let mut claim_error: Option<ApplicationError> = None;
        let mut tasks = JoinSet::new();
        let mut skipped_recipients = HashSet::new();
        let mut unavailable_worlds = HashSet::new();
        let mut unavailable_send_worlds = HashSet::new();

        while !worker.control.is_draining() {
            if (tasks.len() >= effective_concurrency
                || delivery_count >= usize::from(delivery.batch_limit))
                && let Some(result) = tasks.join_next().await
            {
                merge_delivery_task_result(
                    &mut delivery_result,
                    &mut skipped_recipients,
                    &mut unavailable_send_worlds,
                    &mut delivery_count,
                    result,
                );
            }

            if worker.control.is_draining() || delivery_count >= usize::from(delivery.batch_limit) {
                break;
            }

            // 领取可能已提交但回执丢失；本地只等待原 lease，不主动释放未知结果。
            let claim_deadline = tokio::time::Instant::now() + delivery.lease_duration;
            let now = worker.clock.now();
            let Some(lease_until) = now.checked_add_signed(delivery_lease) else {
                claim_error = Some(ApplicationError::Repository(
                    auction_application::RepositoryError::Failed(
                        "delivery claim lease 截止时间溢出".to_owned(),
                    ),
                ));
                break;
            };
            let mut claims = match worker
                .write_lifecycle
                .background(tokio::time::timeout_at(
                    claim_deadline,
                    delivery_store.claim_due(
                        worker.worker_id.clone(),
                        now,
                        lease_until,
                        delivery.sending_grace,
                        DeliveryClaimExclusions {
                            unavailable_worlds: unavailable_worlds.iter().cloned().collect(),
                            unavailable_send_worlds: unavailable_send_worlds
                                .iter()
                                .cloned()
                                .collect(),
                            recipients: skipped_recipients
                                .iter()
                                .map(|(world, recipient)| DeliveryRecipientExclusion {
                                    world: world.clone(),
                                    recipient: recipient.clone(),
                                })
                                .collect(),
                        },
                        1,
                    ),
                ))
                .await
            {
                Some(Ok(Ok(claims))) if tokio::time::Instant::now() < claim_deadline => claims,
                diagnostic_outcome @ (Some(Ok(Ok(_))) | Some(Err(..))) => {
                    diagnostics::warn(
                        "delivery",
                        if matches!(&diagnostic_outcome, Some(Err(..))) {
                            "claim_wave_timeout"
                        } else {
                            "claim_wave_expired"
                        },
                        if matches!(&diagnostic_outcome, Some(Err(..))) {
                            SafeErrorKind::TIMEOUT
                        } else {
                            SafeErrorKind::LEASE_EXPIRED
                        },
                        DiagnosticContext::default(),
                    );
                    drop(diagnostic_outcome);

                    claim_error = Some(ApplicationError::Repository(
                        auction_application::RepositoryError::Failed(
                            "delivery claim 未在租约截止前确认".to_owned(),
                        ),
                    ));
                    break;
                }
                Some(Ok(Err(error))) => {
                    claim_error = Some(error);
                    break;
                }
                None => break,
            };
            let Some(claim) = claims.pop() else {
                break;
            };
            // 领取等待可能跨过 drain；尚未准备或进入 sending 的 claim 可以按原 fence 安全释放。
            if worker.control.is_draining() {
                if worker.write_lifecycle.enabled() {
                    break;
                }
                let released = release_unstarted_delivery(
                    delivery_store.as_ref(),
                    worker.clock.as_ref(),
                    worker.delivery,
                    &claim,
                    WorkerResult::Success,
                    None,
                )
                .await;
                merge_worker_result(&mut delivery_result, released.worker_result);
                break;
            }
            if !delivery_transport.available(&claim) {
                let release = tokio::time::timeout(
                    delivery.finalize_timeout,
                    delivery_store.release(claim.clone(), worker.clock.now()),
                )
                .await;
                match release {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        diagnostics::error(
                            "delivery",
                            "release_drain_claim",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                delivery_id: diagnostic_id(&claim.delivery_id),
                                ..Default::default()
                            },
                        );
                        merge_worker_result(&mut delivery_result, WorkerResult::Error);
                    }
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "delivery",
                            "release_drain_claim_timeout",
                            diagnostics::timeout_kind(&error),
                            DiagnosticContext {
                                delivery_id: diagnostic_id(&claim.delivery_id),
                                ..Default::default()
                            },
                        );

                        merge_worker_result(&mut delivery_result, WorkerResult::Error);
                    }
                }
                let world = claim.payload.get("world").and_then(serde_json::Value::as_str);
                let Some(world @ ("main" | "isekai")) = world else {
                    // false 只能表示有效世界没有凭据；违约适配器不能令同一 claim 紧密重复领取。
                    claim_error = Some(ApplicationError::CapabilityUnavailable(
                        "delivery transport 为无效世界返回了不可用状态",
                    ));
                    break;
                };
                unavailable_worlds.insert(world.to_owned());
                // 跳过的世界不消耗可执行批次容量，batch_limit=1 时也能继续处理其他世界。
                continue;
            }

            delivery_count += 1;
            if claim.attempt_count > 1 {
                worker.metrics.observe_retry(WorkerLoop::Delivery, WorkerRetryKind::Persisted);
            }

            let kind = match claim.payload.get("operation").and_then(serde_json::Value::as_str) {
                Some("take") => CriticalWriteKind::DeliveryTake,
                Some("return") => CriticalWriteKind::DeliveryReturn,
                _ => CriticalWriteKind::DeliverySend,
            };
            let activity = worker.write_lifecycle.activity(kind);
            let store = Arc::clone(delivery_store);
            let transport = Arc::clone(delivery_transport);
            let clock = Arc::clone(&worker.clock);
            let config = delivery;
            let control = worker.control.clone();
            let notifications = Arc::clone(&worker.notifications);
            let panic_store = Arc::clone(&store);
            let panic_clock = Arc::clone(&clock);
            let panic_claim = claim.clone();
            tasks.spawn(diagnostics::inherit(async move {
                match diagnostics::catch_item_panic(
                    "delivery",
                    activity.run(process_delivery_claim(
                        store,
                        transport,
                        clock,
                        config,
                        control,
                        notifications,
                        claim,
                        &activity,
                    )),
                )
                .await
                {
                    Ok(result) => result
                        .unwrap_or_else(|| DeliveryTaskResult::completed(WorkerResult::Canceled)),
                    Err(()) => {
                        let error = DeliveryTransportError {
                            failure_code: "task_panic".to_owned(),
                            safe_detail: "工作项 panic，需人工检查后恢复".to_owned(),
                            outcome_unknown: true,
                            external_reference: None,
                            result_metadata: serde_json::json!({"reason": "task_panic"}),
                        };
                        match tokio::time::timeout(
                            config.finalize_timeout,
                            panic_store.fail(panic_claim, error, panic_clock.now()),
                        )
                        .await
                        {
                            Ok(Ok(())) => activity.complete(),
                            _ => diagnostics::unpersisted(SafeErrorKind::TASK_PANIC),
                        }
                        DeliveryTaskResult::completed(WorkerResult::Error)
                    }
                }
            }));
        }

        while let Some(result) = tasks.join_next().await {
            merge_delivery_task_result(
                &mut delivery_result,
                &mut skipped_recipients,
                &mut unavailable_send_worlds,
                &mut delivery_count,
                result,
            );
        }
        if claim_error.is_none() && delivery_result == WorkerResult::Success {
            worker.metrics.observe_batch(
                WorkerLoop::Delivery,
                delivery_count,
                usize::from(delivery.batch_limit),
            );
        }
        if claim_error.is_some() {
            merge_worker_result(&mut delivery_result, WorkerResult::Error);
        }
        worker.metrics.observe_iteration(
            WorkerLoop::Delivery,
            delivery_result,
            delivery_started.elapsed(),
        );
        match claim_error {
            Some(error) => {
                diagnostics::unpersisted(error.diagnostic_kind());
                Err(error.into())
            }
            None => Ok(()),
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
}

fn merge_delivery_task_result(
    delivery_result: &mut WorkerResult,
    skipped_recipients: &mut HashSet<(String, String)>,
    unavailable_send_worlds: &mut HashSet<String>,
    delivery_count: &mut usize,
    result: Result<DeliveryTaskResult, tokio::task::JoinError>,
) {
    match result {
        Ok(result) => {
            if let Some(world) = result.deferred_world {
                unavailable_send_worlds.insert(world);
                // 只读延后不占发送批次；batch_limit=1 也继续处理 Take/Return 或另一 world。
                *delivery_count = delivery_count.saturating_sub(1);
            }
            if let Some(recipient) = result.skipped_recipient {
                skipped_recipients.insert(recipient);
            }
            merge_worker_result(delivery_result, result.worker_result);
        }
        Err(error) => {
            diagnostics::observe_task_panic("delivery", &error);
            diagnostics::error(
                "delivery",
                "item_join",
                diagnostics::join_kind(&error),
                DiagnosticContext {
                    disposition: if error.is_cancelled() {
                        None
                    } else {
                        Some(DiagnosticDisposition::Unpersisted)
                    },
                    ..Default::default()
                },
            );
            *delivery_result = WorkerResult::Canceled;
        }
    }
}

struct DeliveryTaskResult {
    worker_result: WorkerResult,
    skipped_recipient: Option<(String, String)>,
    deferred_world: Option<String>,
}

impl DeliveryTaskResult {
    fn completed(worker_result: WorkerResult) -> Self {
        Self { worker_result, skipped_recipient: None, deferred_world: None }
    }
}

#[tracing::instrument(

    skip_all,

    fields(

        delivery_id = %claim.delivery_id,

    )

)]
#[allow(clippy::too_many_arguments)]
async fn process_delivery_claim<C>(
    delivery_store: Arc<dyn DeliveryWorkStore>,
    delivery_transport: Arc<dyn DeliveryTransport>,
    clock: Arc<C>,
    config: DeliveryWorkerRuntimeConfig,
    control: WorkerControlHandle,
    notifications: Arc<dyn auction_application::NotificationRecorder>,
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

/// 只读准备期间维持 claimed 租约，任何失败都不经过发送失败/unknown 落库。
async fn prepare_delivery<'a, C: Clock>(
    store: &dyn DeliveryWorkStore,
    transport: &'a dyn DeliveryTransport,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    request_deadline: tokio::time::Instant,
) -> Result<auction_application::AuthorizedDeliverySend<'a>, DeliveryTransportError> {
    // 首次确认独立于 select 排序，不能为了操作公平性绕过准备前的续租门禁。
    // 调用方 timeout_at 覆盖本次确认及后续全部续租，预算不随回执重置。
    lease.renew(store, claim, clock, config).await.map_err(delivery_preparation_error)?;
    let mut preparation = transport
        .prepare_authorized(claim.clone(), request_deadline.min(lease.deadline).into_std());
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(request_deadline) => return Err(delivery_preparation_error("preflight_timeout")),
            _ = tokio::time::sleep_until(lease.deadline) => return Err(delivery_preparation_error("lease_expired")),
            result = std::future::poll_fn(|context| {
                // 与发送相同：直接核对两个截止，确保成功续租后操作先获得一次 poll。
                let now = tokio::time::Instant::now();
                if now >= lease.deadline || now >= request_deadline {
                    return std::task::Poll::Pending;
                }
                preparation.as_mut().poll(context)
            }) => return result,
            _ = tokio::time::sleep_until(lease.renew_at) => {
                lease
                    .renew(store, claim, clock, config)
                    .await
                    .map_err(delivery_preparation_error)?;
            }
        }
    }
}

/// 准备期没有外部写入；租约或预算失败只能延期，不产生未知发送结果。
fn delivery_preparation_error(stage: &str) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: stage.to_owned(),
        safe_detail: "delivery 只读准备未完成，尚未开始远端写入".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: serde_json::json!({"stage": stage, "write_started": false}),
    }
}

/// fence 前后均续租，避免 prepare 尾部或 fence 提交等待吃掉首次发送的 TTL。
async fn confirm_delivery_handoff<C: Clock>(
    store: &dyn DeliveryWorkStore,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    prepared: &mut auction_application::AuthorizedDeliverySend<'_>,
    activity: &ExternalWriteActivity,
) -> bool {
    // 现金证据只授权 Send；Take 的原账号期待随普通 fence 冻结，不能进入现金授权路径。
    if prepared.evidence_mut().is_some()
        && (claim.payload.get("operation").and_then(serde_json::Value::as_str) != Some("send")
            || !matches!(
                prepared.expectation(),
                Some(auction_application::DeliveryPreparedExpectation::Send(_))
            ))
    {
        return false;
    }
    let renewed = if let Some(evidence) = prepared.evidence_mut() {
        let deadline = lease
            .operation_deadline(config.finalize_timeout)
            .min(tokio::time::Instant::from_std(evidence.deadline()));
        matches!(
            tokio::time::timeout_at(deadline, lease.renew(store, claim, clock, config)).await,
            Ok(Ok(()))
        ) && tokio::time::Instant::now() < deadline
    } else {
        // 无现金证据的 legacy 沿用原续租预算。
        lease.renew(store, claim, clock, config).await.is_ok()
    };
    if !renewed {
        return false;
    }
    let deadline = lease.operation_deadline(config.finalize_timeout);
    // 只读准备的原单调截止覆盖检查、全部事务重试及 COMMIT，不在交接时重建寿命。
    let deadline = prepared.evidence_mut().map_or(deadline, |evidence| {
        deadline.min(tokio::time::Instant::from_std(evidence.deadline()))
    });
    if let Some(evidence) = prepared.evidence_mut()
        && (!matches!(
            tokio::time::timeout_at(deadline, evidence.check_current()).await,
            Ok(Ok(()))
        ) || tokio::time::Instant::now() >= deadline)
    {
        return false;
    }
    // Send 摘要或 Take 原账号期待与 Sending 同事务落库；只复制枚举内的 Arc，不复制内容。
    let expectation = prepared.expectation().cloned();
    // 围栏调用前登记交接，与关闭入口同步竞争；已关闭时不发起围栏，保留原 claim。
    // 已登记的围栏往返也计入排空活动；登记本身不授权外写，仍须确认 Sending 提交。
    if !activity.enter_critical() {
        return false;
    }
    prepared.begin_fence();
    let confirmed = if let Some(evidence) = prepared.evidence_mut() {
        match evidence.begin_commit() {
            Ok(guard) => {
                // future借用guard，取消或提交未知同步撤销原owner；不detach、不重建session。
                match tokio::time::timeout_at(
                    deadline,
                    store.start_sending_authorized_until(
                        claim.clone(),
                        clock.now(),
                        guard.snapshot(),
                        deadline.into_std(),
                        expectation.as_ref(),
                    ),
                )
                .await
                {
                    Ok(Ok(outcome)) if tokio::time::Instant::now() < deadline => {
                        // 两种结果均在截止前确定结束；Deferred只允许确定回滚或已提交冻结。
                        guard.confirm_committed();
                        Some(matches!(outcome, auction_application::DeliveryFenceOutcome::Sending))
                    }
                    _ => None,
                }
            }
            Err(error) => {
                diagnostics::warn(
                    "delivery",
                    "confirm_fence",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        delivery_id: diagnostic_id(&claim.delivery_id),
                        ..Default::default()
                    },
                );
                Some(false)
            }
        }
    } else {
        match tokio::time::timeout_at(
            deadline,
            store.start_sending_authorized(claim.clone(), clock.now(), None, expectation.as_ref()),
        )
        .await
        {
            Ok(Ok(outcome)) => {
                Some(matches!(outcome, auction_application::DeliveryFenceOutcome::Sending))
            }
            _ => None,
        }
    };
    // timeout_at 的内部 future 可能一次 poll 越过截止；回执过晚仍保留撤销状态。
    if tokio::time::Instant::now() >= deadline {
        return false;
    }
    if confirmed.is_some() {
        prepared.confirm_fence();
    }
    if confirmed != Some(true) {
        tracing::error!(delivery_id = %claim.delivery_id, "delivery worker did not confirm sending fence");
        return false;
    }
    // Sending 已确定提交；沿用围栏前登记的活动，即使正在排空也继续续租及原发送。
    lease.renew(store, claim, clock, config).await.is_ok()
}

/// 尚未 poll 外部写入时尽力释放；SQL 仅允许有效 claimed，sending 提交未知仍保守隔离。
async fn release_unstarted_delivery<C: Clock>(
    store: &dyn DeliveryWorkStore,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    worker_result: WorkerResult,
    unpersisted_cause: Option<SafeErrorKind>,
) -> DeliveryTaskResult {
    let released =
        tokio::time::timeout(config.finalize_timeout, store.release(claim.clone(), clock.now()))
            .await;
    let worker_result = match released {
        Ok(Ok(())) => {
            if let Some(kind) = unpersisted_cause {
                diagnostics::unpersisted(kind);
            }
            worker_result
        }
        _ => {
            diagnostics::unpersisted(SafeErrorKind::REPOSITORY_FAILED);
            tracing::warn!(delivery_id = %claim.delivery_id, "delivery worker could not confirm unstarted claim release");
            WorkerResult::Error
        }
    };
    DeliveryTaskResult {
        worker_result,
        skipped_recipient: None,
        // 包括续租/fence 失败；本轮不紧密重领，但不让 batch=1 挡住其他 world 或 Take/Return。
        deferred_world: claim
            .payload
            .get("world")
            .and_then(serde_json::Value::as_str)
            .filter(|world| {
                matches!(*world, "main" | "isekai")
                    && claim.payload.get("operation").and_then(serde_json::Value::as_str)
                        == Some("send")
            })
            .map(str::to_owned),
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
