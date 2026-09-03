//! Delivery outbox 的领取、外部发送、租约续期和终态落库。
//!
//! 外部结果未知时保留安全失败语义并进入持久化对账路径，不能凭本地超时推断远端未写入。

use super::*;

use std::collections::HashSet;

/// 以独立 cadence 和 bounded concurrency 处理 delivery outbox。
///
/// 只有获得空闲并发槽后才领取下一条记录，避免已经领取但尚未执行的任务在队列中耗尽 lease。
/// 凭据暂不可用时释放当前 claim 并停止本轮，防止同一条 pending 记录被紧密重复领取。
pub(super) async fn run_delivery_cycle<C>(worker: &PersistentWorker<C>) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let (Some(delivery_store), Some(delivery_transport)) =
        (&worker.delivery_store, &worker.delivery_transport)
    else {
        return Ok(());
    };
    let transport_concurrency = delivery_transport.maximum_concurrency();
    if transport_concurrency == 0 {
        return Err(WorkerError::InvalidDeliveryConfiguration);
    }
    let effective_concurrency = usize::from(worker.delivery.concurrency.min(transport_concurrency));
    let delivery_started = std::time::Instant::now();
    let delivery_lease = ChronoDuration::from_std(worker.delivery.lease_duration)
        .map_err(|_| WorkerError::InvalidDeliveryConfiguration)?;
    let mut delivery_count = 0usize;
    let mut delivery_result = WorkerResult::Success;
    let mut claim_error: Option<ApplicationError> = None;
    let mut tasks = JoinSet::new();
    let mut skipped_recipients = HashSet::new();

    while delivery_count < usize::from(worker.delivery.batch_limit) {
        if tasks.len() >= effective_concurrency
            && let Some(result) = tasks.join_next().await
        {
            merge_delivery_task_result(&mut delivery_result, &mut skipped_recipients, result);
        }

        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(delivery_lease) else {
            claim_error =
                Some(ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    "delivery claim lease 截止时间溢出".to_owned(),
                )));
            break;
        };
        let mut claims = match delivery_store
            .claim_due(
                worker.worker_id.clone(),
                now,
                lease_until,
                worker.delivery.sending_grace,
                skipped_recipients
                    .iter()
                    .map(|(world, recipient)| DeliveryRecipientExclusion {
                        world: world.clone(),
                        recipient: recipient.clone(),
                    })
                    .collect(),
                1,
            )
            .await
        {
            Ok(claims) => claims,
            Err(error) => {
                claim_error = Some(error);
                break;
            }
        };
        let Some(claim) = claims.pop() else {
            break;
        };
        delivery_count += 1;
        if claim.attempt_count > 1 {
            worker.metrics.observe_retry(WorkerLoop::Delivery, WorkerRetryKind::Persisted);
        }

        if !delivery_transport.available(&claim) {
            let release = tokio::time::timeout(
                worker.delivery.finalize_timeout,
                delivery_store.release(claim.clone(), worker.clock.now()),
            )
            .await;
            match release {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker failed to release unavailable claim");
                    delivery_result = WorkerResult::Error;
                }
                Err(_) => {
                    tracing::error!(delivery_id = %claim.delivery_id, "delivery worker timed out while releasing unavailable claim");
                    delivery_result = WorkerResult::Error;
                }
            }
            break;
        }

        tasks.spawn(process_delivery_claim(
            Arc::clone(delivery_store),
            Arc::clone(delivery_transport),
            Arc::clone(&worker.clock),
            worker.delivery,
            claim,
        ));
    }

    while let Some(result) = tasks.join_next().await {
        merge_delivery_task_result(&mut delivery_result, &mut skipped_recipients, result);
    }
    if claim_error.is_none() && delivery_result == WorkerResult::Success {
        worker.metrics.observe_batch(
            WorkerLoop::Delivery,
            delivery_count,
            usize::from(worker.delivery.batch_limit),
        );
    }
    if claim_error.is_some() {
        delivery_result = WorkerResult::Error;
    }
    worker.metrics.observe_iteration(
        WorkerLoop::Delivery,
        delivery_result,
        delivery_started.elapsed(),
    );
    match claim_error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn merge_delivery_task_result(
    delivery_result: &mut WorkerResult,
    skipped_recipients: &mut HashSet<(String, String)>,
    result: Result<DeliveryTaskResult, tokio::task::JoinError>,
) {
    match result {
        Ok(result) => {
            if let Some(recipient) = result.skipped_recipient {
                skipped_recipients.insert(recipient);
            }
            match result.worker_result {
                WorkerResult::Success => {}
                WorkerResult::Error => *delivery_result = WorkerResult::Error,
                WorkerResult::Canceled => *delivery_result = WorkerResult::Canceled,
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "delivery worker task join failed");
            *delivery_result = WorkerResult::Canceled;
        }
    }
}

struct DeliveryTaskResult {
    worker_result: WorkerResult,
    skipped_recipient: Option<(String, String)>,
}

impl DeliveryTaskResult {
    fn completed(worker_result: WorkerResult) -> Self {
        Self { worker_result, skipped_recipient: None }
    }

    fn recipient_not_found(worker_result: WorkerResult, world: &str, recipient: &str) -> Self {
        Self {
            worker_result,
            skipped_recipient: Some((
                world.trim().to_ascii_lowercase(),
                recipient.trim().to_ascii_lowercase(),
            )),
        }
    }
}

async fn process_delivery_claim<C>(
    delivery_store: Arc<dyn DeliveryWorkStore>,
    delivery_transport: Arc<dyn DeliveryTransport>,
    clock: Arc<C>,
    config: DeliveryWorkerRuntimeConfig,
    claim: DeliveryWorkClaim,
) -> DeliveryTaskResult
where
    C: Clock + 'static,
{
    if let Err(error) = delivery_store.start_sending(claim.clone(), clock.now()).await {
        tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker failed to start sending");
        // 外部写尚未开始，尽力释放 claim；失败时仍由 lease 到期后的安全领取兜底。
        match tokio::time::timeout(
            config.finalize_timeout,
            delivery_store.release(claim.clone(), clock.now()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(release_error)) => {
                tracing::error!(error = %release_error, delivery_id = %claim.delivery_id, "delivery worker failed to release claim after start failure");
            }
            Err(_) => {
                tracing::error!(delivery_id = %claim.delivery_id, "delivery worker timed out while releasing claim after start failure");
            }
        }
        return DeliveryTaskResult::completed(WorkerResult::Error);
    }
    let Ok(lease_span) = ChronoDuration::from_std(config.lease_duration) else {
        tracing::error!(delivery_id = %claim.delivery_id, "delivery worker configured lease span is invalid");
        return DeliveryTaskResult::completed(WorkerResult::Error);
    };
    let mut renew_tick = tokio::time::interval(config.renew_interval);
    renew_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // tokio interval 的首个 tick 立即完成，先消费它，避免刚进入 sending 就做无意义续租。
    renew_tick.tick().await;
    let request_timeout = tokio::time::sleep(config.request_timeout);
    tokio::pin!(request_timeout);
    let mut send = delivery_transport.send(claim.clone());
    let mut lease_failed = false;
    let result = loop {
        tokio::select! {
            result = &mut send => break result,
            _ = &mut request_timeout => {
                break Err(delivery_gateway_outcome_unknown(
                    "request_timeout",
                    "delivery 外部请求超时，远端结果未知",
                ));
            }
            _ = renew_tick.tick() => {
                let renewed_at = clock.now();
                let Some(lease_until) = renewed_at.checked_add_signed(lease_span) else {
                    tracing::error!(delivery_id = %claim.delivery_id, "delivery worker renewal deadline overflowed");
                    lease_failed = true;
                    break Err(delivery_gateway_outcome_unknown(
                        "lease_renewal",
                        "delivery 发送期间 lease 续租截止时间溢出",
                    ));
                };
                match tokio::time::timeout(
                    config.finalize_timeout,
                    delivery_store.renew(claim.clone(), renewed_at, lease_until),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker lost lease while sending");
                        lease_failed = true;
                        break Err(delivery_gateway_outcome_unknown(
                            "lease_renewal",
                            "delivery 发送期间 lease 续租失败",
                        ));
                    }
                    Err(_) => {
                        tracing::error!(delivery_id = %claim.delivery_id, "delivery worker lease renewal timed out");
                        lease_failed = true;
                        break Err(delivery_gateway_outcome_unknown(
                            "lease_renewal_timeout",
                            "delivery 发送期间 lease 续租超时",
                        ));
                    }
                }
            }
        }
    };
    let delivery_id = claim.delivery_id;
    let recipient = claim.recipient.clone();
    let recipient_world =
        claim.payload.get("world").and_then(serde_json::Value::as_str).map(str::to_owned);
    let result = result.and_then(normalize_delivery_send_result);
    match result {
        Ok(result) => {
            match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.succeed(claim, result, clock.now()),
            )
            .await
            {
                Ok(Ok(())) if lease_failed => DeliveryTaskResult::completed(WorkerResult::Error),
                Ok(Ok(())) => DeliveryTaskResult::completed(WorkerResult::Success),
                Ok(Err(error)) => {
                    tracing::error!(error = %error, %delivery_id, "delivery worker failed to persist success");
                    DeliveryTaskResult::completed(WorkerResult::Error)
                }
                Err(_) => {
                    tracing::error!(%delivery_id, "delivery worker timed out while persisting success");
                    DeliveryTaskResult::completed(WorkerResult::Error)
                }
            }
        }
        Err(error) => {
            let failure_code = error.failure_code.clone();
            let worker_result = match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.fail(claim, error, clock.now()),
            )
            .await
            {
                Ok(Ok(())) if lease_failed => WorkerResult::Error,
                Ok(Ok(())) => WorkerResult::Success,
                Ok(Err(persist_error)) => {
                    tracing::error!(error = %persist_error, %delivery_id, %failure_code, "delivery worker failed to persist failure");
                    WorkerResult::Error
                }
                Err(_) => {
                    tracing::error!(%delivery_id, %failure_code, "delivery worker timed out while persisting failure");
                    WorkerResult::Error
                }
            };
            if failure_code == "recipient_not_found"
                && let Some(world) = recipient_world.as_deref()
            {
                DeliveryTaskResult::recipient_not_found(worker_result, world, &recipient)
            } else {
                DeliveryTaskResult::completed(worker_result)
            }
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
