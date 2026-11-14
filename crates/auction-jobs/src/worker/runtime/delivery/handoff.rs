//! Delivery 只读准备、sending fence 交接与未开始释放；证据失效或租约不足时只延期重试，
//! 不把没有 poll 外部写入的失败伪装成发送失败或远端未知。

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::delivery::{
    DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::operations::ExternalWriteActivity;
use auction_observability::WorkerResult;

use super::lease::DeliveryLease;
use super::report::DeliveryTaskResult;
use crate::worker::diagnostics;
use crate::worker::runtime::DeliveryWorkerRuntimeConfig;

/// 只读准备期间维持 claimed 租约，任何失败都不经过发送失败/unknown 落库。
pub(super) async fn prepare_delivery<'a, C: Clock>(
    store: &dyn DeliveryWorkStore,
    transport: &'a dyn DeliveryTransport,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    request_deadline: tokio::time::Instant,
) -> Result<auction_application::delivery::AuthorizedDeliverySend<'a>, DeliveryTransportError> {
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
pub(super) fn delivery_preparation_error(stage: &str) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: stage.to_owned(),
        safe_detail: "delivery 只读准备未完成，尚未开始远端写入".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: serde_json::json!({"stage": stage, "write_started": false}),
    }
}

/// fence 前后均续租，避免 prepare 尾部或 fence 提交等待吃掉首次发送的 TTL。
pub(super) async fn confirm_delivery_handoff<C: Clock>(
    store: &dyn DeliveryWorkStore,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    prepared: &mut auction_application::delivery::AuthorizedDeliverySend<'_>,
    activity: &ExternalWriteActivity,
) -> bool {
    // 现金证据只授权 Send；Take 的原账号期待随普通 fence 冻结，不能进入现金授权路径。
    if prepared.evidence_mut().is_some()
        && (claim.payload.get("operation").and_then(serde_json::Value::as_str) != Some("send")
            || !matches!(
                prepared.expectation(),
                Some(auction_application::delivery::DeliveryPreparedExpectation::Send(_))
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
                        Some(matches!(
                            outcome,
                            auction_application::delivery::DeliveryFenceOutcome::Sending
                        ))
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
            Ok(Ok(outcome)) => Some(matches!(
                outcome,
                auction_application::delivery::DeliveryFenceOutcome::Sending
            )),
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
pub(super) async fn release_unstarted_delivery<C: Clock>(
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
