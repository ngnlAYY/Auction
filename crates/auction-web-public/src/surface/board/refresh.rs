//! Public 竞价资源失效与有界合并读取；不修改事件游标或推导竞价结果。
//!
//! 实时事件只是“竞价发生变化”的通知：这里只递增本地修订号，由各页面资源在短窗口后
//! 重读自己的权威接口。事件负载中的价格、领先者或版本都不参与页面结果，因此取消合并或
//! 丢弃通知只会多读或少读一次快照，不会产生第二种竞价状态。

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

/// 从首次待处理通知起合并 100 毫秒；后续竞价不延后截止时间。
///
/// “每次出价的页面重读量”即由该窗口约束：窗口内的 N 条竞价通知对同一资源
/// 只产生一次重读，窗口外的通知各自产生一次。HTTP 路由计数用于在 Final 阶段
/// 实测该比值，口径见 `crates/auction-observability/README.md`。
const BID_REFRESH_COALESCE_MILLIS: u32 = 100;

/// Public 外壳持有的竞价失效修订号，与生命周期和人工刷新的全局修订号分离。
#[derive(Clone, Copy)]
pub(crate) struct PublicBidRefresh(pub(crate) Signal<u64>);

impl PublicBidRefresh {
    pub(crate) fn notify(mut self) {
        let next = self.0.peek().saturating_add(1);
        self.0.set(next);
    }
}

/// 单次竞价通知在合并窗口内的调度判定。
///
/// 与信号和计时器解耦，保证以下性质可直接测试，而不是只靠组件行为推断：
/// 未消费的修订号只排定一次；窗口等待中的通知既不延长截止也不叠加读取；资源未就绪时
/// 不消费修订号，等它就绪后仍会读取最新快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BidRefreshDecision {
    /// 修订号已消费（或通知已被禁用分支吸收）：不动作。
    Idle,
    /// 资源尚未 `Ready`：本次不排定、不消费，避免取消进行中的请求。
    WaitForReady,
    /// 已有窗口在等待：合并进该窗口，不延长其截止时间。
    Coalescing,
    /// 为当前修订号排定一个合并窗口。
    Schedule { revision: u64 },
}

/// 纯判定函数：只依赖修订号与资源状态，不读事件负载、不产生副作用。
pub(crate) fn plan_bid_refresh(
    current_revision: u64,
    consumed_revision: u64,
    scheduled: bool,
    ready: bool,
) -> BidRefreshDecision {
    if current_revision == consumed_revision {
        return BidRefreshDecision::Idle;
    }
    if scheduled {
        return BidRefreshDecision::Coalescing;
    }
    if !ready {
        return BidRefreshDecision::WaitForReady;
    }
    BidRefreshDecision::Schedule { revision: current_revision }
}

/// 仅刷新已挂载且启用的竞价资源；进行中的读取先完成，再补读尚未消费的通知。
/// 竞价修订号不能直接成为 `use_resource` 的依赖，否则持续竞价会反复取消慢请求。
/// 延后任务由消费组件拥有：卸载会取消任务，新挂载的资源自行读取最新权威快照。
pub(crate) fn use_bid_refresh<T: 'static>(mut resource: Resource<T>, enabled: bool) {
    let refresh = use_context::<PublicBidRefresh>();
    let enabled = use_memo(use_reactive((&enabled,), |(enabled,)| enabled));
    let mut consumed = use_signal(|| *refresh.0.peek());
    let mut scheduled = use_signal(|| false);

    use_effect(move || {
        if !enabled() {
            // 闭合的代理面板不订阅竞价或资源状态，避免每行随通知执行空刷新。
            consumed.set(*refresh.0.peek());
            return;
        }
        let revision = (refresh.0)();
        // 先读资源状态以订阅它就绪/重新挂起的变化，再按纯函数决定是否排定。
        let state = resource.state().cloned();
        let decision = plan_bid_refresh(
            revision,
            *consumed.peek(),
            *scheduled.peek(),
            state == UseResourceState::Ready,
        );
        match decision {
            BidRefreshDecision::Idle
            | BidRefreshDecision::Coalescing
            | BidRefreshDecision::WaitForReady => {}
            BidRefreshDecision::Schedule { .. } => {
                scheduled.set(true);
                spawn(async move {
                    TimeoutFuture::new(BID_REFRESH_COALESCE_MILLIS).await;
                    scheduled.set(false);
                    let revision = *refresh.0.peek();
                    // 等待期间可能切换查询、关闭代理面板或触发全局刷新；只重启空闲资源。
                    if *enabled.peek()
                        && revision != *consumed.peek()
                        && *resource.state().peek() == UseResourceState::Ready
                    {
                        consumed.set(revision);
                        resource.restart();
                    }
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{BID_REFRESH_COALESCE_MILLIS, BidRefreshDecision, plan_bid_refresh};

    /// 用“窗口从首条通知开始、不因后续通知延长”的同一规则模拟一个资源的重读次数。
    /// 只建模合并策略本身；真实请求仍由权威接口完成，本测试不替代线上度量。
    fn simulate_rereads(notification_millis: &[u32]) -> usize {
        let mut consumed = 0_u64;
        let mut scheduled = false;
        let mut deadline = 0_u32;
        let mut rereads = 0;
        let mut revision = 0_u64;

        for &now in notification_millis {
            // 计时器在截止时刻完成：先消费当时已到达的最新修订号并计数一次重读。
            if scheduled && now >= deadline {
                scheduled = false;
                rereads += 1;
                consumed = revision;
            }
            revision += 1;
            if let BidRefreshDecision::Schedule { .. } =
                plan_bid_refresh(revision, consumed, scheduled, true)
            {
                scheduled = true;
                deadline = now + BID_REFRESH_COALESCE_MILLIS;
            }
        }
        if scheduled {
            rereads += 1;
        }
        rereads
    }

    #[test]
    fn first_notification_schedules_exactly_once() {
        assert_eq!(
            plan_bid_refresh(7, 6, false, true),
            BidRefreshDecision::Schedule { revision: 7 }
        );
    }

    #[test]
    fn consumed_revision_does_not_read_again() {
        assert_eq!(plan_bid_refresh(7, 7, false, true), BidRefreshDecision::Idle);
        // 即使上一窗口仍在等待，也不重复排定同一修订号。
        assert_eq!(plan_bid_refresh(7, 7, true, true), BidRefreshDecision::Idle);
    }

    #[test]
    fn notification_during_window_coalesces_without_extending() {
        assert_eq!(plan_bid_refresh(8, 7, true, true), BidRefreshDecision::Coalescing);
    }

    #[test]
    fn not_ready_resource_waits_and_keeps_revision() {
        assert_eq!(plan_bid_refresh(8, 7, false, false), BidRefreshDecision::WaitForReady);
        // 就绪后同一修订号仍会排定，不因等待期间的状态变化丢失读取。
        assert_eq!(
            plan_bid_refresh(8, 7, false, true),
            BidRefreshDecision::Schedule { revision: 8 }
        );
    }

    #[test]
    fn burst_inside_single_window_reads_once() {
        assert_eq!(simulate_rereads(&[0, 10, 20, 30, 40]), 1);
    }

    #[test]
    fn window_is_not_extended_by_trailing_notifications() {
        // 首条 0ms 的窗口在 100ms 截止；99ms 的通知合入，150ms 的通知另起窗口。
        assert_eq!(simulate_rereads(&[0, 99, 150]), 2);
    }

    #[test]
    fn sustained_bidding_reads_once_per_window() {
        // 每 40ms 一条、共 7 条：窗口在 0、120、240ms 分别开启，读 3 次而不是 7 次。
        assert_eq!(simulate_rereads(&[0, 40, 80, 120, 160, 200, 240]), 3);
    }

    #[test]
    fn sparse_bidding_reads_once_per_bid() {
        // 出价间隔大于窗口时每条通知各读一次，不做延迟合并。
        assert_eq!(simulate_rereads(&[0, 200, 400]), 3);
    }
}
