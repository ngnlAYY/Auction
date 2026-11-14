//! 履约证据轮询的年龄退避与付款截止聚焦。

use chrono::{DateTime, Duration, Utc};

use crate::errors::ApplicationError;

/// 证据退避上限（决策 55）；晚付款的确认与卖方货款最多因此延后 10 分钟。
pub(super) const EVIDENCE_BACKOFF_MAXIMUM: Duration = Duration::minutes(10);

/// 付款截止前后恢复基础间隔的聚焦窗口；与退避上限同宽，覆盖截止前后的迟到付款。
const EVIDENCE_BACKOFF_DEADLINE_FOCUS: Duration = Duration::minutes(10);

/// 间隔至少一秒，避免非正配置产生零间隔空轮询。
const EVIDENCE_BACKOFF_MINIMUM: Duration = Duration::seconds(1);

/// 翻倍次数的显式上限；`u32` 级计数不能靠循环逼近，间隔本身已在上限处饱和。
const EVIDENCE_BACKOFF_MAXIMUM_DOUBLINGS: u32 = 32;

/// 计算履约证据下一次轮询的间隔。
///
/// 证据等待（delivery 未到终态、Sent 回执未观察、买家/卖家尚未在 MoogleMail 领取）
/// 可能持续数天，而每次延期都会持久化递增 `retry_count`。这里以该持久化次数作为
/// 义务的年龄步数逐步翻倍：
///
/// ```text
/// delay = min(上限, max(1s, base) × 2^retry_count)
/// ```
///
/// 默认 60 秒的基础间隔给出 1→2→4→8→10 分钟。
///
/// 付款截止（`payment_deadline_at`）前后一个上限周期内恢复基础间隔：既要更快发现
/// 迟到付款，也不因退避把截止附近的确认拖到 10 分钟之后。这里只决定轮询时刻，付款
/// 期限本身与 recovery owner 的逾期判定都不改变；缺失期限的历史义务仍按年龄退避。
///
/// 基础间隔若大于上限则以上限为准：证据轮询的 10 分钟上限优先于配置。
pub(super) fn evidence_poll_delay(
    base: Duration,
    retry_count: u32,
    payment_deadline_at: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
) -> Duration {
    let base = base.max(EVIDENCE_BACKOFF_MINIMUM).min(EVIDENCE_BACKOFF_MAXIMUM);
    if payment_deadline_focus(payment_deadline_at, observed_at) {
        return base;
    }
    let maximum = EVIDENCE_BACKOFF_MAXIMUM.num_seconds();
    let mut seconds = base.num_seconds();
    for _ in 0..retry_count.min(EVIDENCE_BACKOFF_MAXIMUM_DOUBLINGS) {
        if seconds >= maximum {
            break;
        }
        seconds = seconds.saturating_mul(2).min(maximum);
    }
    Duration::seconds(seconds)
}

/// Sent 证据读取失败后的重试间隔。
///
/// 远端失败（账号忙、协议漂移、限流、会话失效等）与证据等待一样使用按义务年龄退避的
/// `evidence_delay`：远端故障期间每条义务每个配置间隔都重读一遍 Sent，只会放大同一游戏
/// 账号的页面加载。退避仍受 10 分钟上限与付款截止聚焦约束。本地依赖错误（存储、配置）
/// 按配置的 `retry_delay` 尽快重试。
pub(super) fn evidence_read_failure_delay(
    error: &ApplicationError,
    retry_delay: Duration,
    evidence_delay: Duration,
) -> Duration {
    if matches!(
        error,
        ApplicationError::RemoteDependency { .. } | ApplicationError::FulfillmentAccountBusy
    ) {
        evidence_delay
    } else {
        retry_delay
    }
}

/// 观察时刻是否落在付款截止前后的聚焦窗口内。
pub(super) fn payment_deadline_focus(
    payment_deadline_at: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
) -> bool {
    let Some(deadline) = payment_deadline_at else {
        return false;
    };
    let focus = u64::try_from(EVIDENCE_BACKOFF_DEADLINE_FOCUS.num_seconds()).unwrap_or_default();
    (observed_at - deadline).num_seconds().unsigned_abs() <= focus
}

/// 等待买家支付 COD 或卖家领取货款时的轮询间隔。
///
/// 共享追踪窗口按绝对时间对齐多条义务的读取，避免同一账号在一轮里重复读取同一页；
/// 该对齐只在非聚焦期保持。付款截止前后改用 `min(窗口, 证据间隔)`，让迟到付款按基础
/// 频率被确认；没有共享窗口的普通装备寄送直接使用证据间隔。
pub(super) fn collection_poll_delay(
    window_next: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    evidence_delay: Duration,
    payment_deadline_at: Option<DateTime<Utc>>,
) -> Duration {
    let Some(next) = window_next else {
        return evidence_delay;
    };
    let window_delay = next - observed_at;
    if payment_deadline_focus(payment_deadline_at, observed_at) {
        return window_delay.min(evidence_delay);
    }
    window_delay
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        EVIDENCE_BACKOFF_MAXIMUM, collection_poll_delay, evidence_poll_delay,
        evidence_read_failure_delay, payment_deadline_focus,
    };
    use crate::errors::ApplicationError;
    use crate::errors::diagnostics::SafeErrorKind;
    use crate::ports::core::RepositoryError;

    fn at(seconds: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().expect("时间戳必须在范围内")
    }

    fn base() -> chrono::Duration {
        chrono::Duration::seconds(60)
    }

    #[test]
    fn evidence_schedule_widens_from_one_minute_to_ten_minutes() {
        let now = at(1_700_000_000);
        let delays: Vec<i64> = (0..=5)
            .map(|retry_count| evidence_poll_delay(base(), retry_count, None, now).num_seconds())
            .collect();
        // 60s→120s→240s→480s→600s（上限），没有 30 分钟档。
        assert_eq!(delays, vec![60, 120, 240, 480, 600, 600]);
    }

    #[test]
    fn remote_read_failures_back_off_while_local_failures_retry_promptly() {
        let retry = base();
        let backoff = EVIDENCE_BACKOFF_MAXIMUM;
        for remote in [
            ApplicationError::RemoteDependency {
                kind: SafeErrorKind::REMOTE_PROTOCOL_ERROR,
                detail: "MoogleMail protocol_drift 证据读取失败".to_owned(),
            },
            ApplicationError::FulfillmentAccountBusy,
        ] {
            assert_eq!(evidence_read_failure_delay(&remote, retry, backoff), backoff);
        }
        let local = ApplicationError::Repository(RepositoryError::Unavailable);
        assert_eq!(evidence_read_failure_delay(&local, retry, backoff), retry);
    }

    #[test]
    fn evidence_backoff_caps_at_ten_minutes() {
        let now = at(1_700_000_000);
        assert_eq!(evidence_poll_delay(base(), 4, None, now), EVIDENCE_BACKOFF_MAXIMUM);
        assert_eq!(evidence_poll_delay(base(), u32::MAX, None, now), EVIDENCE_BACKOFF_MAXIMUM);
    }

    #[test]
    fn deadline_long_past_keeps_age_schedule_and_recovery_policy() {
        // 逾期义务的期限已经过去很久；退避不把它永久钉在基础频率，也不改写期限本身。
        // 逾期判定仍由 recovery owner 负责，这里只决定轮询时刻。
        let now = at(1_700_000_000);
        let deadline = now - chrono::Duration::days(3);
        assert!(!payment_deadline_focus(Some(deadline), now));
        assert!(!payment_deadline_focus(None, now));
        assert_eq!(evidence_poll_delay(base(), 4, Some(deadline), now), EVIDENCE_BACKOFF_MAXIMUM);
        // 同一观察时刻、同一基础间隔下，过期期限与无期限的历史义务得到相同间隔。
        assert_eq!(
            evidence_poll_delay(base(), 4, Some(deadline), now),
            evidence_poll_delay(base(), 4, None, now)
        );
    }

    #[test]
    fn payment_deadline_focus_restores_base_before_and_after_deadline() {
        let deadline = at(1_700_003_600);
        // 截止前十秒、截止时刻本身、截止后十分之一秒内都使用基础间隔。
        for observed_at in [
            deadline - chrono::Duration::seconds(10),
            deadline,
            deadline + chrono::Duration::seconds(1),
            deadline + EVIDENCE_BACKOFF_MAXIMUM,
        ] {
            assert!(payment_deadline_focus(Some(deadline), observed_at));
            assert_eq!(
                evidence_poll_delay(base(), 6, Some(deadline), observed_at),
                base(),
                "付款截止前后必须回到基础频率"
            );
        }
        // 聚焦窗口之外继续按年龄退避，且仍不超过上限。
        for observed_at in [
            deadline - EVIDENCE_BACKOFF_MAXIMUM - chrono::Duration::seconds(1),
            deadline + EVIDENCE_BACKOFF_MAXIMUM + chrono::Duration::seconds(1),
        ] {
            assert!(!payment_deadline_focus(Some(deadline), observed_at));
            assert_eq!(
                evidence_poll_delay(base(), 6, Some(deadline), observed_at),
                EVIDENCE_BACKOFF_MAXIMUM
            );
        }
    }

    #[test]
    fn missing_deadline_keeps_age_schedule() {
        let now = at(1_700_000_000);
        assert_eq!(evidence_poll_delay(base(), 0, None, now), base());
        assert_eq!(evidence_poll_delay(base(), 3, None, now), chrono::Duration::seconds(480));
    }

    #[test]
    fn configured_base_wins_when_not_wider_than_cap() {
        let now = at(1_700_000_000);
        // 首次轮询沿用配置基础间隔：不改变既有行为。
        assert_eq!(evidence_poll_delay(base(), 0, None, now), base());
        // 上限优先于更大的配置值，非正配置被抬到 1 秒。
        assert_eq!(
            evidence_poll_delay(chrono::Duration::seconds(1_800), 0, None, now),
            EVIDENCE_BACKOFF_MAXIMUM
        );
        assert_eq!(
            evidence_poll_delay(chrono::Duration::zero(), 0, None, now),
            chrono::Duration::seconds(1)
        );
    }

    #[test]
    fn collection_window_stays_aligned_outside_deadline_focus() {
        let now = at(1_700_000_000);
        let window_next = at(1_700_000_300);
        // 非聚焦期保持共享窗口节奏，不被证据退避缩短或打断。
        assert_eq!(
            collection_poll_delay(Some(window_next), now, base(), None),
            chrono::Duration::seconds(300)
        );
        // 没有共享窗口的普通寄送直接使用证据退避间隔。
        assert_eq!(
            collection_poll_delay(None, now, chrono::Duration::seconds(480), None),
            chrono::Duration::seconds(480)
        );
    }

    #[test]
    fn collection_window_tightens_to_base_near_payment_deadline() {
        let deadline = at(1_700_000_120);
        let now = at(1_700_000_000);
        let evidence_delay = evidence_poll_delay(base(), 6, Some(deadline), now);
        assert_eq!(evidence_delay, base());
        assert_eq!(
            collection_poll_delay(Some(at(1_700_000_300)), now, evidence_delay, Some(deadline)),
            base(),
            "付款截止前后按基础频率确认迟到付款"
        );
    }
}
