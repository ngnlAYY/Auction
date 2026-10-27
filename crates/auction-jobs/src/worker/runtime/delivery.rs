//! Delivery outbox 的领取、外部发送、租约续期和终态落库；
//! 外部结果未知时保留安全失败语义并进入持久化对账，不能凭本地超时推断远端未写入。

use super::*;

use std::collections::HashSet;

mod lease;
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
    let mut unavailable_worlds = HashSet::new();
    let mut unavailable_send_worlds = HashSet::new();

    while !worker.control.is_draining() {
        if (tasks.len() >= effective_concurrency
            || delivery_count >= usize::from(worker.delivery.batch_limit))
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

        if worker.control.is_draining()
            || delivery_count >= usize::from(worker.delivery.batch_limit)
        {
            break;
        }

        // 领取可能已提交但回执丢失；本地只等待原 lease，不主动释放未知结果。
        let claim_deadline = tokio::time::Instant::now() + worker.delivery.lease_duration;
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(delivery_lease) else {
            claim_error =
                Some(ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    "delivery claim lease 截止时间溢出".to_owned(),
                )));
            break;
        };
        let mut claims = match tokio::time::timeout_at(
            claim_deadline,
            delivery_store.claim_due(
                worker.worker_id.clone(),
                now,
                lease_until,
                worker.delivery.sending_grace,
                DeliveryClaimExclusions {
                    unavailable_worlds: unavailable_worlds.iter().cloned().collect(),
                    unavailable_send_worlds: unavailable_send_worlds.iter().cloned().collect(),
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
        )
        .await
        {
            Ok(Ok(claims)) if tokio::time::Instant::now() < claim_deadline => claims,
            Ok(Ok(_)) | Err(_) => {
                claim_error = Some(ApplicationError::Repository(
                    auction_application::RepositoryError::Failed(
                        "delivery claim 未在租约截止前确认".to_owned(),
                    ),
                ));
                break;
            }
            Ok(Err(error)) => {
                claim_error = Some(error);
                break;
            }
        };
        let Some(claim) = claims.pop() else {
            break;
        };
        // 领取等待可能跨过 drain；尚未准备或进入 sending 的 claim 可以按原 fence 安全释放。
        if worker.control.is_draining() {
            let released = release_unstarted_delivery(
                delivery_store.as_ref(),
                worker.clock.as_ref(),
                worker.delivery,
                &claim,
                WorkerResult::Success,
            )
            .await;
            merge_worker_result(&mut delivery_result, released.worker_result);
            break;
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
                    merge_worker_result(&mut delivery_result, WorkerResult::Error);
                }
                Err(_) => {
                    tracing::error!(delivery_id = %claim.delivery_id, "delivery worker timed out while releasing unavailable claim");
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

        tasks.spawn(process_delivery_claim(
            Arc::clone(delivery_store),
            Arc::clone(delivery_transport),
            Arc::clone(&worker.clock),
            worker.delivery,
            worker.control.clone(),
            claim,
        ));
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
            usize::from(worker.delivery.batch_limit),
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
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
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
            tracing::error!(error = %error, "delivery worker task join failed");
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

    fn recipient_not_found(worker_result: WorkerResult, world: &str, recipient: &str) -> Self {
        Self {
            worker_result,
            deferred_world: None,
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
    control: WorkerControlHandle,
    claim: DeliveryWorkClaim,
) -> DeliveryTaskResult
where
    C: Clock + 'static,
{
    // 领取后派生的任务可能尚未获得调度；首次 poll 前再次确认 drain。
    if control.is_draining() {
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Success,
        )
        .await;
    }
    let mut lease = match DeliveryLease::new(&claim, clock.as_ref(), config) {
        Ok(lease) => lease,
        Err(failure_code) => {
            tracing::warn!(delivery_id = %claim.delivery_id, %failure_code, "delivery claim has no usable lease");
            return release_unstarted_delivery(
                delivery_store.as_ref(),
                clock.as_ref(),
                config,
                &claim,
                WorkerResult::Error,
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
            let result = tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.reject_preparation(claim, clock.now()),
            )
            .await;
            return DeliveryTaskResult::completed(if matches!(result, Ok(Ok(()))) {
                WorkerResult::Success
            } else {
                WorkerResult::Error
            });
        }
        preparation => {
            let failure_code = match preparation {
                Ok(Err(error)) => error.failure_code,
                _ => "preflight_timeout".to_owned(),
            };
            tracing::warn!(delivery_id = %claim.delivery_id, %failure_code, "delivery preflight deferred before sending fence");
            return release_unstarted_delivery(
                delivery_store.as_ref(),
                clock.as_ref(),
                config,
                &claim,
                WorkerResult::Success,
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
    )
    .await
    {
        drop(prepared);
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Error,
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
                break Err(delivery_gateway_outcome_unknown("lease_expired", "delivery 发送期间 lease 已过期"));
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
                ).await;
                let stage = match renewal {
                    Ok(Ok(())) => continue,
                    Ok(Err(stage)) => stage,
                    Err(_) => "request_timeout",
                };
                lease_failed = true;
                break Err(delivery_gateway_outcome_unknown(stage, "delivery 发送期间 lease 续租失败或超时"));
            }
        }
    };
    drop(send);
    if !send_polled {
        // fence 后尚未 poll 也可能失去租约；不伪造外部 unknown，sending 本身仍保守隔离。
        return release_unstarted_delivery(
            delivery_store.as_ref(),
            clock.as_ref(),
            config,
            &claim,
            WorkerResult::Error,
        )
        .await;
    }
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
                lease.renew(store, claim, clock, config).await.map_err(delivery_preparation_error)?;
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
            Err(_) => Some(false),
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
    lease.renew(store, claim, clock, config).await.is_ok()
}

/// 尚未 poll 外部写入时尽力释放；SQL 仅允许有效 claimed，sending 提交未知仍保守隔离。
async fn release_unstarted_delivery<C: Clock>(
    store: &dyn DeliveryWorkStore,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    worker_result: WorkerResult,
) -> DeliveryTaskResult {
    let released =
        tokio::time::timeout(config.finalize_timeout, store.release(claim.clone(), clock.now()))
            .await;
    let worker_result = match released {
        Ok(Ok(())) => worker_result,
        _ => {
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
