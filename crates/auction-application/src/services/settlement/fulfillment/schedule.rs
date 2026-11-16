//! 履约证据轮询的年龄退避、付款截止聚焦与等待领取的寄出时长分档。

use chrono::{DateTime, Duration, Utc};

use crate::errors::ApplicationError;

/// 证据退避上限；晚付款的确认与卖方货款最多因此延后 10 分钟（寄出满 24 小时后的等待领取
/// 另按下文的寄出时长分档放宽）。
pub(super) const EVIDENCE_BACKOFF_MAXIMUM: Duration = Duration::minutes(10);

/// 付款截止前后恢复基础间隔的聚焦窗口；与退避上限同宽，覆盖截止前后的迟到付款。
const EVIDENCE_BACKOFF_DEADLINE_FOCUS: Duration = Duration::minutes(10);

/// 间隔至少一秒，避免非正配置产生零间隔空轮询。
const EVIDENCE_BACKOFF_MINIMUM: Duration = Duration::seconds(1);

/// 翻倍次数的显式上限；`u32` 级计数不能靠循环逼近，间隔本身已在上限处饱和。
const EVIDENCE_BACKOFF_MAXIMUM_DOUBLINGS: u32 = 32;

// 已寄出、等待收件人领取的 Sent 读取分档。每次读取都要在与真人共用的游戏账号上加载 Sent
// 列表页与详情页（邮件越旧、所在页越深，加载越多），并与发信、收件共用同一请求门；收件人
// 数天不领取时仍按 5 分钟窗口或 10 分钟退避读取，只会持续消耗账号的页面预算。寄出后的头
// 24 小时保持既有节奏，覆盖绝大多数正常领取；之后逐档放宽。长期未领取由运维告警提示人工
// 跟进，轮询只负责发现领取，放宽不影响正确性。

/// 寄出满该时长后改为每小时读取一次。
const COLLECTION_WAIT_HOURLY_AFTER: Duration = Duration::hours(24);
/// 寄出 24 小时至 7 天的读取间隔。
const COLLECTION_WAIT_HOURLY_INTERVAL: Duration = Duration::hours(1);
/// 寄出满该时长后改为每 6 小时读取一次。
const COLLECTION_WAIT_SLOW_AFTER: Duration = Duration::days(7);
/// 寄出 7 天以上的读取间隔。
const COLLECTION_WAIT_SLOW_INTERVAL: Duration = Duration::hours(6);

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
/// 基础间隔大于上限时以上限为准。
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

/// 已寄出、等待领取的义务下次 Sent 读取的最小间隔；`None` 表示保持既有节奏。
///
/// 寄出时长取投递完成的数据库时刻（`deliveries.completed_at`），不另取时钟来源。寄出不足
/// 24 小时、寄出时刻未知（不放宽，失败关闭到既有的较密节奏）、或观察时刻落在付款截止聚焦
/// 窗口内时返回 `None`；24 小时至 7 天为 1 小时，7 天以上为 6 小时。
pub(super) fn collection_wait_interval(
    sent_at: Option<DateTime<Utc>>,
    observed_at: DateTime<Utc>,
    payment_deadline_at: Option<DateTime<Utc>>,
) -> Option<Duration> {
    if payment_deadline_focus(payment_deadline_at, observed_at) {
        return None;
    }
    let waited = observed_at - sent_at?;
    if waited >= COLLECTION_WAIT_SLOW_AFTER {
        Some(COLLECTION_WAIT_SLOW_INTERVAL)
    } else if waited >= COLLECTION_WAIT_HOURLY_AFTER {
        Some(COLLECTION_WAIT_HOURLY_INTERVAL)
    } else {
        None
    }
}

/// 把既有轮询间隔放宽到等待领取分档：只放宽、不收紧（配置的追踪窗口更宽时保持窗口）。
///
/// 付款截止尚未进入聚焦窗口时，放宽后的读取最迟落在窗口起点，聚焦期内迟到付款仍按基础
/// 间隔确认；付款期限、逾期判定与恢复策略都不变。
pub(super) fn widen_for_collection_wait(
    delay: Duration,
    interval: Option<Duration>,
    observed_at: DateTime<Utc>,
    payment_deadline_at: Option<DateTime<Utc>>,
) -> Duration {
    let Some(interval) = interval else {
        return delay;
    };
    let widened = delay.max(interval);
    match payment_deadline_at {
        Some(deadline) => {
            let until_focus = deadline - EVIDENCE_BACKOFF_DEADLINE_FOCUS - observed_at;
            if until_focus > Duration::zero() {
                widened.min(until_focus).max(delay)
            } else {
                widened
            }
        }
        None => widened,
    }
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
        EVIDENCE_BACKOFF_MAXIMUM, collection_poll_delay, collection_wait_interval,
        evidence_poll_delay, evidence_read_failure_delay, payment_deadline_focus,
        widen_for_collection_wait,
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
        // 截止前十秒、截止时刻、截止后一秒直至聚焦窗口边界都使用基础间隔。
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
        // 首次轮询沿用配置基础间隔。
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

    /// 寄出时长分档的边界：不足 24 小时保持现状，24 小时至 7 天每小时，7 天及以上每 6 小时；
    /// 寄出时刻未知或晚于观察时刻（时钟偏差）时不放宽。
    #[test]
    fn collection_wait_tiers_follow_time_since_sent() {
        let now = at(1_700_000_000);
        let sent = |ago: chrono::Duration| Some(now - ago);
        let hour = chrono::Duration::hours(1);
        let six_hours = chrono::Duration::hours(6);
        for (ago, expected) in [
            (chrono::Duration::hours(23) + chrono::Duration::minutes(59), None),
            (chrono::Duration::hours(24) - chrono::Duration::seconds(1), None),
            (chrono::Duration::hours(24), Some(hour)),
            (chrono::Duration::days(3), Some(hour)),
            (chrono::Duration::days(7) - chrono::Duration::seconds(1), Some(hour)),
            (chrono::Duration::days(7), Some(six_hours)),
            (chrono::Duration::days(30), Some(six_hours)),
        ] {
            assert_eq!(collection_wait_interval(sent(ago), now, None), expected, "寄出 {ago}");
        }
        assert_eq!(collection_wait_interval(None, now, None), None);
        assert_eq!(collection_wait_interval(sent(chrono::Duration::hours(-1)), now, None), None);
    }

    /// 分档只放宽既有间隔：5 分钟窗口与 10 分钟退避被放宽到分档，配置得更宽的追踪窗口保持原值。
    #[test]
    fn collection_wait_only_widens_existing_delay() {
        let now = at(1_700_000_000);
        let hour = chrono::Duration::hours(1);
        let window = chrono::Duration::seconds(300);
        assert_eq!(widen_for_collection_wait(window, Some(hour), now, None), hour);
        assert_eq!(
            widen_for_collection_wait(EVIDENCE_BACKOFF_MAXIMUM, Some(hour), now, None),
            hour
        );
        let wide_window = chrono::Duration::hours(2);
        assert_eq!(widen_for_collection_wait(wide_window, Some(hour), now, None), wide_window);
        assert_eq!(widen_for_collection_wait(window, None, now, None), window);
    }

    /// COD 付款截止聚焦保持不变：聚焦窗口内不放宽；窗口之前放宽的读取最迟落在窗口起点；
    /// 截止早已过去时按寄出时长放宽。
    #[test]
    fn collection_wait_keeps_payment_deadline_focus() {
        let now = at(1_700_000_000);
        let sent = Some(now - chrono::Duration::days(3));
        let window_next = Some(at(1_700_000_300));

        // 截止前 5 分钟：处于聚焦窗口，按基础间隔确认迟到付款。
        let deadline = Some(now + chrono::Duration::minutes(5));
        assert_eq!(collection_wait_interval(sent, now, deadline), None);
        let evidence_delay = evidence_poll_delay(base(), 6, deadline, now);
        let delay = collection_poll_delay(window_next, now, evidence_delay, deadline);
        assert_eq!(widen_for_collection_wait(delay, None, now, deadline), base());

        // 截止前 30 分钟：分档为 1 小时，但下次读取最迟在截止前 10 分钟（聚焦窗口起点）。
        let deadline = Some(now + chrono::Duration::minutes(30));
        let interval = collection_wait_interval(sent, now, deadline);
        assert_eq!(interval, Some(chrono::Duration::hours(1)));
        let window = chrono::Duration::seconds(300);
        assert_eq!(
            widen_for_collection_wait(window, interval, now, deadline),
            chrono::Duration::minutes(20)
        );
        assert!(payment_deadline_focus(deadline, now + chrono::Duration::minutes(20)));
        // 距聚焦窗口起点比既有间隔还近时保持既有间隔，不因截止而缩短。
        let deadline = Some(now + chrono::Duration::minutes(12));
        let interval = collection_wait_interval(sent, now, deadline);
        assert_eq!(widen_for_collection_wait(window, interval, now, deadline), window);

        // 截止已过去一天：不在聚焦窗口，按寄出 8 天放宽到 6 小时。
        let deadline = Some(now - chrono::Duration::days(1));
        let interval =
            collection_wait_interval(Some(now - chrono::Duration::days(8)), now, deadline);
        assert_eq!(interval, Some(chrono::Duration::hours(6)));
        assert_eq!(
            widen_for_collection_wait(window, interval, now, deadline),
            chrono::Duration::hours(6)
        );
    }
}
