//! 账号忙熔断：战斗页只限制请求节奏，不产生任何业务状态。
//!
//! 保护的是“外部账号正被玩家占用”这一临时事实：连续命中战斗页时按 1→2→5→10 分钟
//! 退避，窗口内只放行一次只读探测，其余请求在发出网络之前直接返回 `AccountBusy`。
//! 熔断状态只保留在进程内，可丢失；丢失后最坏只是多一次远端请求，不会改变受理、
//! 结算或写入未知等业务判定（不变量 5）。

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use super::MoogleMailWorld;

/// 连续命中战斗页时的退避档位；第 5 次及以后维持最后一档。
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(300),
    Duration::from_secs(600),
];

#[derive(Default)]
struct BusyState {
    /// 当前忙碌期已升级到的退避档位（1 基）。
    level: usize,
    /// 当前退避窗口结束时刻；过期后不立即清零档位，重复命中才继续升级。
    until: Option<Instant>,
    /// 当前窗口是否已经放行过只读探测。
    probe_used: bool,
    /// 同一忙碌期是否已经保存过首份战斗页证据。
    evidence_saved: bool,
}

/// 单个账号（一个 `MoogleMailClient`）内的按世界熔断状态。
#[derive(Default)]
pub(crate) struct MoogleMailAccountBusy {
    states: Mutex<HashMap<MoogleMailWorld, BusyState>>,
}

impl MoogleMailAccountBusy {
    fn states(&self) -> MutexGuard<'_, HashMap<MoogleMailWorld, BusyState>> {
        // 状态只影响限流节奏；锁中毒时直接沿用残留状态，不能把账号忙升级成进程故障。
        self.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 退避窗口内只放行一次只读探测；窗口外恢复常规读取节奏。
    pub(crate) fn permits_read(&self, world: MoogleMailWorld, now: Instant) -> bool {
        let mut states = self.states();
        let state = states.entry(world).or_default();
        match state.until {
            Some(until) if now < until => {
                if state.probe_used {
                    false
                } else {
                    state.probe_used = true;
                    true
                }
            }
            _ => true,
        }
    }

    /// 写入类请求在窗口内一律不发网络；拒绝发生在 sending 围栏之前，不能算作写入未知。
    pub(crate) fn permits_write(&self, world: MoogleMailWorld, now: Instant) -> bool {
        self.states().get(&world).and_then(|state| state.until).is_none_or(|until| now >= until)
    }

    /// 观察到战斗页：升级退避档位并延长窗口。
    /// 不重置探测额度：若本次失败来自窗口内探测，立即重新放行会让探测退化成常规请求。
    pub(crate) fn record_busy(&self, world: MoogleMailWorld, now: Instant) {
        let mut states = self.states();
        let state = states.entry(world).or_default();
        state.level = (state.level + 1).min(BACKOFF.len());
        state.until = Some(now + BACKOFF[state.level - 1]);
    }

    /// 只读请求成功证明账号已退出战斗：结束当前忙碌期并重置退避档位。
    pub(crate) fn record_success(&self, world: MoogleMailWorld) {
        self.states().remove(&world);
    }

    /// 同一忙碌期只允许保存首份战斗页证据；写入未知等其他证据不受影响。
    pub(crate) fn retain_busy_evidence(&self, world: MoogleMailWorld) -> bool {
        let mut states = self.states();
        let state = states.entry(world).or_default();
        if state.evidence_saved {
            false
        } else {
            state.evidence_saved = true;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{MoogleMailAccountBusy, MoogleMailWorld};

    const MAIN: MoogleMailWorld = MoogleMailWorld::Main;
    const ISEKAI: MoogleMailWorld = MoogleMailWorld::Isekai;

    fn busy(busy: &MoogleMailAccountBusy, world: MoogleMailWorld, now: Instant) {
        busy.record_busy(world, now);
    }

    #[test]
    fn backoff_escalates_one_two_five_ten_minutes_and_caps() {
        let breaker = MoogleMailAccountBusy::default();
        let start = Instant::now();
        let schedule = [60u64, 120, 300, 600, 600];

        for (index, seconds) in schedule.into_iter().enumerate() {
            let observed_at = start + Duration::from_secs(index as u64 * 1000);
            busy(&breaker, MAIN, observed_at);
            assert!(
                !breaker.permits_write(MAIN, observed_at + Duration::from_secs(seconds - 1)),
                "第 {} 次忙碌在退避结束前不得放行写入",
                index + 1
            );
            assert!(
                breaker.permits_write(MAIN, observed_at + Duration::from_secs(seconds)),
                "第 {} 次忙碌在 {} 秒后应恢复写入",
                index + 1,
                seconds
            );
        }
    }

    #[test]
    fn backoff_window_allows_exactly_one_read_probe() {
        let breaker = MoogleMailAccountBusy::default();
        let now = Instant::now();
        busy(&breaker, MAIN, now);

        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(1)));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(2)));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(59)));
        // 窗口结束后恢复常规读取，不继续消耗探测额度。
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(60)));
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(61)));
    }

    #[test]
    fn repeated_busy_does_not_rearm_the_probe_immediately() {
        let breaker = MoogleMailAccountBusy::default();
        let now = Instant::now();
        busy(&breaker, MAIN, now);
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(1)));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(2)));

        // 探测同样命中战斗页：升级到 2 分钟，但同一窗口不能紧接着再探测。
        busy(&breaker, MAIN, now + Duration::from_secs(3));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(4)));
        assert!(!breaker.permits_write(MAIN, now + Duration::from_secs(122)));
        assert!(breaker.permits_write(MAIN, now + Duration::from_secs(123)));

        // 窗口结束后恢复常规读取；再次命中战斗页才重新开启更长的窗口。
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(123)));
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(124)));
        busy(&breaker, MAIN, now + Duration::from_secs(125));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(126)));
    }

    #[test]
    fn success_ends_the_busy_period_and_resets_the_schedule() {
        let breaker = MoogleMailAccountBusy::default();
        let now = Instant::now();
        busy(&breaker, MAIN, now);
        busy(&breaker, MAIN, now + Duration::from_secs(1));
        assert!(!breaker.permits_write(MAIN, now + Duration::from_secs(2)));

        breaker.record_success(MAIN);
        assert!(breaker.permits_write(MAIN, now + Duration::from_secs(2)));
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(2)));

        // 忙碌期结束后重新从 1 分钟档起步。
        busy(&breaker, MAIN, now + Duration::from_secs(3));
        assert!(!breaker.permits_write(MAIN, now + Duration::from_secs(62)));
        assert!(breaker.permits_write(MAIN, now + Duration::from_secs(63)));
    }

    #[test]
    fn worlds_and_accounts_are_isolated() {
        let breaker = MoogleMailAccountBusy::default();
        let other_account = MoogleMailAccountBusy::default();
        let now = Instant::now();
        busy(&breaker, MAIN, now);

        assert!(!breaker.permits_write(MAIN, now + Duration::from_secs(1)));
        assert!(breaker.permits_write(ISEKAI, now + Duration::from_secs(1)));
        // 异世界还没有忙碌窗口，读取不受主世界熔断影响。
        assert!(breaker.permits_read(ISEKAI, now + Duration::from_secs(1)));
        assert!(breaker.permits_read(ISEKAI, now + Duration::from_secs(2)));

        // 异世界自己的窗口有独立探测额度，主世界的探测额度也未被消耗。
        busy(&breaker, ISEKAI, now);
        assert!(breaker.permits_read(ISEKAI, now + Duration::from_secs(3)));
        assert!(!breaker.permits_read(ISEKAI, now + Duration::from_secs(4)));
        assert!(breaker.permits_read(MAIN, now + Duration::from_secs(3)));
        assert!(!breaker.permits_read(MAIN, now + Duration::from_secs(4)));

        // 另一个账号（另一个客户端）完全独立。
        assert!(other_account.permits_write(MAIN, now + Duration::from_secs(1)));
        busy(&other_account, MAIN, now);
        assert!(other_account.permits_read(MAIN, now + Duration::from_secs(5)));
        assert!(!other_account.permits_read(MAIN, now + Duration::from_secs(6)));
    }

    #[test]
    fn busy_evidence_is_retained_once_per_busy_period() {
        let breaker = MoogleMailAccountBusy::default();
        let now = Instant::now();
        busy(&breaker, MAIN, now);

        assert!(breaker.retain_busy_evidence(MAIN));
        assert!(!breaker.retain_busy_evidence(MAIN));
        busy(&breaker, MAIN, now + Duration::from_secs(1));
        assert!(!breaker.retain_busy_evidence(MAIN));
        assert!(breaker.retain_busy_evidence(ISEKAI));

        breaker.record_success(MAIN);
        busy(&breaker, MAIN, now + Duration::from_secs(2));
        assert!(breaker.retain_busy_evidence(MAIN));
    }
}
