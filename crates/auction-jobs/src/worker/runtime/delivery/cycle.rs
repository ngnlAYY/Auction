//! Delivery 轮次领取与在途任务编排；只在本轮持有空闲并发槽和可执行批次容量时领取。

use std::collections::HashSet;
use std::sync::Arc;

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{ApplicationError, Clock};
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryRecipientExclusion, DeliveryTransportError,
};
use auction_application::operations::CriticalWriteKind;
use auction_observability::{WorkerLoop, WorkerResult, WorkerRetryKind};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError, merge_worker_result};

use super::dispatch::process_delivery_claim;
use super::handoff::release_unstarted_delivery;
use super::report::{DeliveryTaskResult, merge_delivery_task_result};

/// 以独立 cadence 和 bounded concurrency 处理 delivery outbox。
/// 只有获得空闲并发槽后才领取下一条记录，避免已经领取但尚未执行的任务在队列中耗尽 lease。
/// 凭据暂不可用时释放 claim 并在本轮排除对应世界，避免重复领取或阻塞其他世界。
pub(in crate::worker::runtime) async fn run_delivery_cycle<C>(
    worker: &PersistentWorker<C>,
) -> Result<(), WorkerError>
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
        // 投递属于批量轮次，受共享连接配额限制；许可随本轮结束释放，等待在 runner 内。
        let _bulk_permit = worker.bulk_permit().await;
        let transport_concurrency = delivery_transport.maximum_concurrency();
        if transport_concurrency == 0 {
            return Err(WorkerError::InvalidDeliveryConfiguration);
        }
        // 暂停只阻止本轮开始新的领取与外部发送；已领取且越过 sending 围栏的写入
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

        // 过期回收与退役扫描每轮只做一次；逐条领取只执行 claim，避免积压时按条重复扫描。
        // 与 claim 使用同一写生命周期守卫与租约预算：drain 可在进入关键段前取消整轮清理，
        // 清理超时则结束本轮而不是把无界 SQL 扫描带入停机排空。
        let cleanup_deadline = tokio::time::Instant::now() + delivery.lease_duration;
        let cleanup = tokio::time::timeout_at(
            cleanup_deadline,
            delivery_store.begin_claim_cycle(worker.clock.now(), delivery.sending_grace),
        );
        match worker.write_lifecycle.background(cleanup).await {
            None => return Ok(()),
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(error))) => claim_error = Some(error),
            Some(Err(elapsed)) => {
                tracing::warn!(error = %elapsed, "delivery 领取前清理超时");
                diagnostics::warn(
                    "delivery",
                    "preclaim_cleanup_timeout",
                    SafeErrorKind::TIMEOUT,
                    DiagnosticContext::default(),
                );
                claim_error = Some(ApplicationError::Repository(
                    auction_application::core::RepositoryError::Failed(
                        "delivery 领取前清理未在预算内完成".to_owned(),
                    ),
                ));
            }
        }
        while claim_error.is_none() && !worker.control.is_draining() {
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
                    auction_application::core::RepositoryError::Failed(
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
                            // 关闭类只暂停领取；投递保留在 outbox，重新开启后补发。
                            suppressed_classes: worker.hot.snapshot().mail.send_classes,
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
                        auction_application::core::RepositoryError::Failed(
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
