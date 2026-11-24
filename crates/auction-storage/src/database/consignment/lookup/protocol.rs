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

/// 按已使用的第 1–5 次尝试选择下次等待。第六次终结，收到 429 时沿用第五档的两小时。
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

/// 429 的本次冷却时长，取本地退避与封顶六小时的 Retry-After 的较大值。
/// 第六次缺失提示或提示短于两小时时取两小时。落库时与已有冷却截止取最大值。
pub(super) fn rate_limit_delay(
    retry_delay: Option<Duration>,
    retry_after: Option<LookupRetryAfter>,
) -> Duration {
    retry_delay
        .unwrap_or(RETRY_DELAYS[RETRY_DELAYS.len() - 1])
        .max(retry_after.map_or(Duration::ZERO, LookupRetryAfter::duration))
}
