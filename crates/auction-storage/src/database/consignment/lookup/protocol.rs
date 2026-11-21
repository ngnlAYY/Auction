//! 解析的计次、重排与时限规则。

use std::time::Duration;

use auction_application::consignment::LookupRetryAfter;

/// 领取即计次，尝试上限由应用层端口定义。
pub(crate) use auction_application::consignment::MAX_LISTING_LOOKUP_ATTEMPTS;

/// 原网络硬截止后的租期及在途时间栅栏余量。
pub(crate) const LEASE_GRACE: Duration = Duration::from_secs(5);

/// 本代 Worker 依赖就绪的心跳有效期。
#[allow(dead_code, reason = "由后续任务 2F 续期心跳时调用")]
pub(crate) const WORKER_READY_TTL: Duration = Duration::from_secs(30);

/// 按已使用的第 1–5 次尝试选择下次等待。第六次终结。
const RETRY_DELAYS: [Duration; MAX_LISTING_LOOKUP_ATTEMPTS as usize - 1] = [
    Duration::from_secs(30),
    Duration::from_mins(2),
    Duration::from_mins(10),
    Duration::from_mins(30),
    Duration::from_hours(2),
];

/// 按已提交的尝试次数确定下一次等待。零次及耗尽时没有重排时刻。
pub(super) fn retry_delay(attempt_count: i16) -> Option<Duration> {
    let index = usize::try_from(attempt_count).ok()?.checked_sub(1)?;
    RETRY_DELAYS.get(index).copied()
}

/// 429 的冷却下界。上游提示由 `LookupRetryAfter` 统一封顶为六小时。
pub(super) fn rate_limit_delay(
    retry_delay: Option<Duration>,
    retry_after: Option<LookupRetryAfter>,
) -> Duration {
    // 合同未给第六次 429 的退避下界，沿用最长一档，避免耗尽后提前放行。
    retry_delay
        .unwrap_or(RETRY_DELAYS[RETRY_DELAYS.len() - 1])
        .max(retry_after.map_or(Duration::ZERO, LookupRetryAfter::duration))
}
