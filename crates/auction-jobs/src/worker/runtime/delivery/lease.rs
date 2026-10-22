//! Delivery 准备、fence 交接和发送共用的有界续租；本地截止只收紧持久租约。

use super::*;

use std::time::Duration;
use tokio::time::Instant;

/// 以续租请求开始时刻计算保守单调截止；不代替数据库 token/fencing 权威门禁。
pub(super) struct DeliveryLease {
    pub(super) deadline: Instant,
    pub(super) renew_at: Instant,
    span: ChronoDuration,
}

impl DeliveryLease {
    pub(super) fn new<C: Clock>(
        claim: &DeliveryWorkClaim,
        clock: &C,
        config: DeliveryWorkerRuntimeConfig,
    ) -> Result<Self, &'static str> {
        let observed_at = Instant::now();
        let remaining = (claim.lease_until - clock.now()).to_std().map_err(|_| "lease_expired")?;
        let deadline = observed_at.checked_add(remaining).ok_or("lease_deadline_overflow")?;
        let span =
            ChronoDuration::from_std(config.lease_duration).map_err(|_| "lease_span_invalid")?;
        // 领取提交/排队已经消耗 TTL；在首次 prepare poll 前立即确认一次续租。
        Ok(Self { deadline, renew_at: observed_at, span })
    }

    pub(super) fn operation_deadline(&self, timeout: Duration) -> Instant {
        Instant::now().checked_add(timeout).map_or(self.deadline, |limit| self.deadline.min(limit))
    }

    pub(super) async fn renew<C: Clock>(
        &mut self,
        store: &dyn DeliveryWorkStore,
        claim: &DeliveryWorkClaim,
        clock: &C,
        config: DeliveryWorkerRuntimeConfig,
    ) -> Result<(), &'static str> {
        let started = Instant::now();
        if started >= self.deadline {
            return Err("lease_expired");
        }
        let renewed_at = clock.now();
        let lease_until =
            renewed_at.checked_add_signed(self.span).ok_or("lease_deadline_overflow")?;
        let deadline =
            started.checked_add(config.lease_duration).ok_or("lease_deadline_overflow")?;
        let renewal_deadline = self.operation_deadline(config.finalize_timeout);
        // 等待数据库不能跨越已知旧租约；超时即停，不凭可能提交的续租结果继续执行。
        match tokio::time::timeout_at(
            renewal_deadline,
            store.renew(claim.clone(), renewed_at, lease_until),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker lease renewal failed");
                return Err("lease_renewal");
            }
            Err(_) => return Err("lease_renewal_timeout"),
        }
        // timeout_at 先 poll 内层 future；单次 poll 或调度延迟可能让成功回执晚于旧截止。
        // 迟到回执不能重建本地 owner 的有效期，也不能授权继续 poll 外部发送。
        if Instant::now() >= renewal_deadline {
            return Err("lease_renewal_timeout");
        }
        // 回执本身可能已耗尽新 TTL，不能先 poll prepared 再等待首轮 tick。
        if Instant::now() >= deadline || clock.now() >= lease_until {
            return Err("lease_expired");
        }
        self.deadline = deadline;
        self.renew_at = started + config.renew_interval;
        Ok(())
    }
}
