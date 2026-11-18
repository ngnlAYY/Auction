//! Delivery 只读准备、sending fence 交接与未开始释放。租约不确定时保留隔离，
//! 确定的收件人身份拒绝交给 Worker 的投递结果路径。

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{ApplicationError, Clock};
use auction_application::delivery::{
    AuthorizedDeliverySend, DeliveryAdminError, DeliveryPreparedSend, DeliveryTransport,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::operations::ExternalWriteActivity;
use auction_observability::WorkerResult;
use auction_time::TimeDelta;

use super::lease::DeliveryLease;
use super::report::DeliveryTaskResult;
use crate::worker::diagnostics;
use crate::worker::runtime::DeliveryWorkerRuntimeConfig;

/// 已准备的传输。空值表示确定的换绑拒绝，不能持有或执行远端 future。
pub(super) struct PreparedDelivery<'a>(Option<AuthorizedDeliverySend<'a>>);

impl<'a> PreparedDelivery<'a> {
    pub(super) fn into_send(self) -> DeliveryPreparedSend<'a> {
        match self.0 {
            Some(prepared) => prepared.into_send(),
            None => Box::pin(std::future::ready(Err(DeliveryTransportError {
                failure_code: "preflight_blocked".to_owned(),
                safe_detail: "登录 key 回信的收件角色已换绑".to_owned(),
                outcome_unknown: false,
                external_reference: None,
                result_metadata: serde_json::json!({
                    "stage": "preflight_blocked", "preflight_reason": "recipient_renamed", "write_started": false
                }),
            }))),
        }
    }
}

/// 只读准备期间维持 claimed 租约。确认换绑时交给普通结果路径，不申请发送授权。
pub(super) async fn prepare_delivery<'a, C: Clock>(
    store: &dyn DeliveryWorkStore,
    transport: &'a dyn DeliveryTransport,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    request_deadline: tokio::time::Instant,
) -> Result<PreparedDelivery<'a>, DeliveryTransportError> {
    // 首次确认独立于 select 排序，不能为了操作公平性绕过准备前的续租门禁。
    // 调用方 timeout_at 覆盖本次确认及后续全部续租，预算不随回执重置。
    if !renew_before_write(lease, store, claim, clock, config)
        .await
        .map_err(delivery_preparation_error)?
    {
        return Ok(PreparedDelivery(None));
    }
    // 适配器拿到的是准备请求的原绝对截止，不与此刻的租约截止取较小值：租约由下面的循环持续
    // 续租守住，续租失败或租约到期都立即丢弃（取消）准备 future。若把创建时的租约截止交给
    // 适配器，之后的续租无法再延长它，准备预算会被默认 60 秒租约截短，远低于请求预算。
    let mut preparation = transport.prepare_authorized(claim.clone(), request_deadline.into_std());
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(request_deadline) => return Err(DeliveryTransportError::preflight_timeout()),
            _ = tokio::time::sleep_until(lease.deadline) => return Err(delivery_preparation_error("lease_expired")),
            result = std::future::poll_fn(|context| {
                // 与发送相同：直接核对两个截止，确保成功续租后操作先获得一次 poll。
                let now = tokio::time::Instant::now();
                if now >= lease.deadline || now >= request_deadline {
                    return std::task::Poll::Pending;
                }
                preparation.as_mut().poll(context)
            }) => return result.map(|transport| PreparedDelivery(Some(transport))),
            _ = tokio::time::sleep_until(lease.renew_at) => {
                if !renew_before_write(lease, store, claim, clock, config)
                    .await
                    .map_err(delivery_preparation_error)?
                {
                    return Ok(PreparedDelivery(None));
                }
            }
        }
    }
}

/// 登录 key 回信的续租回执须区分确定的身份拒绝和未确认的租约。共享相同的旧租约与请求预算。
async fn renew_before_write<C: Clock>(
    lease: &mut DeliveryLease,
    store: &dyn DeliveryWorkStore,
    claim: &DeliveryWorkClaim,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
) -> Result<bool, &'static str> {
    if claim.payload.get("user_key_user_id").and_then(serde_json::Value::as_str).is_none() {
        return lease.renew(store, claim, clock, config).await.map(|()| true);
    }
    let started = tokio::time::Instant::now();
    if started >= lease.deadline {
        return Err("lease_expired");
    }
    let span = TimeDelta::from_std(config.lease_duration).ok().ok_or("lease_span_invalid")?;
    let renewed_at = clock.now();
    let lease_until = renewed_at.checked_add_signed(span).ok_or("lease_deadline_overflow")?;
    let deadline = started.checked_add(config.lease_duration).ok_or("lease_deadline_overflow")?;
    let renewal_deadline = lease.operation_deadline(config.finalize_timeout);
    let binding_current = match tokio::time::timeout_at(
        renewal_deadline,
        store.renew(claim.clone(), renewed_at, lease_until),
    )
    .await
    {
        Ok(Ok(())) => true,
        Ok(Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::RecipientRenamed))) => false,
        Ok(Err(error)) => {
            diagnostics::error(
                "delivery",
                "lease_renew",
                error.diagnostic_kind(),
                DiagnosticContext {
                    delivery_id: diagnostic_id(&claim.delivery_id),
                    ..Default::default()
                },
            );
            return Err("lease_renewal");
        }
        Err(error) => {
            diagnostics::warn(
                "delivery",
                "lease_renew_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );
            return Err("lease_renewal_timeout");
        }
    };
    if tokio::time::Instant::now() >= renewal_deadline {
        return Err("lease_renewal_timeout");
    }
    if tokio::time::Instant::now() >= deadline || clock.now() >= lease_until {
        return Err("lease_expired");
    }
    lease.deadline = deadline;
    lease.renew_at = started + config.renew_interval;
    Ok(binding_current)
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

/// 投递交接的结论；只有 [`DeliveryHandoff::Sending`] 可以消费已准备的结果。
pub(super) enum DeliveryHandoff {
    /// 发送已授权，或传输已丢弃、仅能收口确定的本地失败。
    Sending,
    /// 围栏事务在截止前按业务门确定结束但没有进入 sending（确定回滚或已提交冻结），例如竞价警告
    /// 在交接期间失效；尚未外部写入，按正常延期释放 claim，不是 Worker 错误。
    Deferred,
    /// 围栏事务在截止前因内部故障（SQL 错误、冲突重试耗尽等）确定回滚：尚未外部写入，可以安全
    /// 释放 claim，但按存储层给出的安全分类保留 Worker 错误与未持久化计数，不能当作正常延期。
    Faulted(SafeErrorKind),
    /// 续租、证据、关闭登记或提交结果未得到确认；沿用原有错误隔离与 owner 撤销语义。
    Unconfirmed,
}

/// 围栏调用的确定终点；提交结果未知用外层 `None` 表示。
enum FenceEnd {
    Sending,
    Deferred,
    /// 内部故障后确定回滚，携带存储层的安全分类。
    Faulted(SafeErrorKind),
    /// 现金证据未能开始提交，围栏根本没有发起。
    NotStarted,
}

/// fence 前后均续租，避免 prepare 尾部或 fence 提交等待吃掉首次发送的 TTL。
pub(super) async fn confirm_delivery_handoff<C: Clock>(
    store: &dyn DeliveryWorkStore,
    clock: &C,
    config: DeliveryWorkerRuntimeConfig,
    claim: &DeliveryWorkClaim,
    lease: &mut DeliveryLease,
    prepared: &mut PreparedDelivery<'_>,
    activity: &ExternalWriteActivity,
) -> DeliveryHandoff {
    let Some(transport) = prepared.0.as_mut() else {
        return begin_local_completion(activity);
    };
    // 现金证据只授权 Send；Take 的原账号期待随普通 fence 冻结，不能进入现金授权路径。
    if transport.evidence_mut().is_some()
        && (claim.payload.get("operation").and_then(serde_json::Value::as_str) != Some("send")
            || !matches!(
                transport.expectation(),
                Some(auction_application::delivery::DeliveryPreparedExpectation::Send(_))
            ))
    {
        return DeliveryHandoff::Unconfirmed;
    }
    let renewed = if let Some(evidence) = transport.evidence_mut() {
        let deadline = lease
            .operation_deadline(config.finalize_timeout)
            .min(tokio::time::Instant::from_std(evidence.deadline()));
        tokio::time::timeout_at(deadline, renew_before_write(lease, store, claim, clock, config))
            .await
            .ok()
            .filter(|_| tokio::time::Instant::now() < deadline)
    } else {
        // 无现金证据的 legacy 沿用原续租预算。
        Some(renew_before_write(lease, store, claim, clock, config).await)
    };
    match renewed {
        Some(Ok(true)) => {}
        Some(Ok(false)) => {
            prepared.0 = None;
            return begin_local_completion(activity);
        }
        _ => return DeliveryHandoff::Unconfirmed,
    }
    let deadline = lease.operation_deadline(config.finalize_timeout);
    // 只读准备的原单调截止覆盖检查、全部事务重试及 COMMIT，不在交接时重建寿命。
    let deadline = transport.evidence_mut().map_or(deadline, |evidence| {
        deadline.min(tokio::time::Instant::from_std(evidence.deadline()))
    });
    if let Some(evidence) = transport.evidence_mut()
        && (!matches!(
            tokio::time::timeout_at(deadline, evidence.check_current()).await,
            Ok(Ok(()))
        ) || tokio::time::Instant::now() >= deadline)
    {
        return DeliveryHandoff::Unconfirmed;
    }
    // Send 摘要或 Take 原账号期待与 Sending 同事务落库；只复制枚举内的 Arc，不复制内容。
    let expectation = transport.expectation().cloned();
    // 围栏调用前登记交接，与关闭入口同步竞争；已关闭时不发起围栏，保留原 claim。
    // 已登记的围栏往返也计入排空活动；登记本身不授权外写，仍须确认 Sending 提交。
    if !activity.enter_critical() {
        return DeliveryHandoff::Unconfirmed;
    }
    transport.begin_fence();
    let fence = if let Some(evidence) = transport.evidence_mut() {
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
                        // 三种结果均在截止前确定结束；Deferred只允许确定回滚或已提交冻结，Faulted
                        // 只允许确定回滚。
                        guard.confirm_committed();
                        Some(fence_end(outcome))
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
                Some(FenceEnd::NotStarted)
            }
        }
    } else {
        match tokio::time::timeout_at(
            deadline,
            store.start_sending_authorized(claim.clone(), clock.now(), None, expectation.as_ref()),
        )
        .await
        {
            Ok(Ok(outcome)) => Some(fence_end(outcome)),
            _ => None,
        }
    };
    // timeout_at 的内部 future 可能一次 poll 越过截止；回执过晚仍保留撤销状态。
    if tokio::time::Instant::now() >= deadline {
        return DeliveryHandoff::Unconfirmed;
    }
    if fence.is_some() {
        transport.confirm_fence();
    }
    match fence {
        Some(FenceEnd::Sending) => {}
        // 业务门确定未进入 sending 不是 Worker 错误：由调用方按正常延期释放，不记 ERROR。
        Some(FenceEnd::Deferred) => return DeliveryHandoff::Deferred,
        // 内部故障确定回滚：同样由调用方释放，但保留错误分类；不撤销原会话。
        Some(FenceEnd::Faulted(kind)) => return DeliveryHandoff::Faulted(kind),
        Some(FenceEnd::NotStarted) | None => {
            tracing::error!(delivery_id = %claim.delivery_id, "delivery worker did not confirm sending fence");
            return DeliveryHandoff::Unconfirmed;
        }
    }
    // sending 提交后、首次 poll 发送 future 前的续租锁验收件身份，提交即释放用户行锁。回信实际
    // POST 前仍会重读 compose、等待请求门和在途锁、复核凭据 owner；期间换绑，已渲染的回信仍
    // 可能寄往旧名。已开始外写且结果不明时转人工核对。
    let Ok(binding_current) = renew_before_write(lease, store, claim, clock, config).await else {
        return DeliveryHandoff::Unconfirmed;
    };
    if !binding_current {
        // 原远端 future 尚未 poll，丢弃后只交付本地失败给 Worker 原有的结果路径。
        prepared.0 = None;
    }
    DeliveryHandoff::Sending
}

fn begin_local_completion(activity: &ExternalWriteActivity) -> DeliveryHandoff {
    if activity.enter_critical() { DeliveryHandoff::Sending } else { DeliveryHandoff::Unconfirmed }
}

fn fence_end(outcome: auction_application::delivery::DeliveryFenceOutcome) -> FenceEnd {
    match outcome {
        auction_application::delivery::DeliveryFenceOutcome::Sending => FenceEnd::Sending,
        auction_application::delivery::DeliveryFenceOutcome::Deferred => FenceEnd::Deferred,
        auction_application::delivery::DeliveryFenceOutcome::Faulted(kind) => {
            FenceEnd::Faulted(kind)
        }
    }
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
