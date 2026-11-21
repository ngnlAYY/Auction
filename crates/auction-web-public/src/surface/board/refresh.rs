//! Public 竞价资源失效与有界合并读取，以及权威读取的统一刷新入口；不修改事件游标或推导竞价
//! 结果。
//!
//! 实时事件只是“竞价发生变化”的通知：这里只递增本地修订号，由各页面资源在 100–250 毫秒的
//! 随机窗口后重读自己的权威接口。事件负载中的价格、领先者或版本都不参与页面结果，因此取消
//! 合并或丢弃通知只会多读或少读一次快照，不会产生第二种竞价状态。

use std::collections::{BTreeMap, VecDeque};

use auction_web_shared::timer::TimeoutFuture;
use auction_web_shared::visibility::document_is_visible;
use dioxus::dioxus_core::Task;
use dioxus::prelude::*;

/// 合并窗口的基础时长：从首次待处理通知起至少合并 100 毫秒；后续竞价不延后截止时间。
///
/// “每次出价的页面重读量”即由该窗口约束：窗口内的 N 条竞价通知对同一资源
/// 只产生一次重读，窗口外的通知各自产生一次。HTTP 路由计数用于在 Final 阶段
/// 实测该比值，口径见 `crates/auction-observability/README.md`。
const BID_REFRESH_COALESCE_MILLIS: u32 = 100;
/// 每个窗口另加的均匀抖动上界（不含）：同一事件推给全部连接，固定窗口会让所有客户端在
/// 同一时刻重读并一起撞上匿名读闸门的并发许可上限；抖动把重读打散到 100–250 毫秒。
const BID_REFRESH_JITTER_MILLIS: u32 = 150;
const MAX_TRACKED_AUCTION_NOTICES: usize = 128;

/// 由 `[0, 1)` 的均匀随机样本计算本次合并窗口，结果总在 `[100, 250)` 毫秒内。
/// 随机源由调用方注入（浏览器为 `Math.random`）；越界或非有限样本按边界处理，不会越出窗口。
fn coalesce_window_millis(sample: f64) -> u32 {
    let sample = if sample.is_finite() { sample.clamp(0.0, 1.0) } else { 0.0 };
    // 样本已截到 [0, 1]：乘积落在 [0, 150]，转换不会溢出；1.0 再收进上界之内。
    let jitter = (sample * f64::from(BID_REFRESH_JITTER_MILLIS)) as u32;
    BID_REFRESH_COALESCE_MILLIS + jitter.min(BID_REFRESH_JITTER_MILLIS - 1)
}

/// 外壳权威轮询、恢复可见、开场迁移、看板对齐重读与错误重试共用的刷新入口：先使公开读缓存
/// 过期，再推进外壳的全局刷新代号。只推进代号时，TTL 内的重读直接命中新鲜条目，等于没有重读；
/// 失效后的重读带旧 ETag 条件请求，正文未变时 304 复用，同一 URL 的在途读取仍单飞合并。
pub(crate) fn refresh_public_reads(mut refresh_generation: Signal<u64>) {
    crate::api::invalidate_all();
    let next_generation = refresh_generation.peek().saturating_add(1);
    refresh_generation.set(next_generation);
}

/// Public 外壳持有的竞价失效修订号，与生命周期和人工刷新的全局修订号分离。
#[derive(Clone, Copy)]
pub(crate) struct PublicBidRefresh(Signal<BidRefreshNotices>);

#[derive(Default)]
struct BidRefreshNotices {
    latest: BidRefreshNotice,
    by_auction: BTreeMap<String, BidRefreshNotice>,
    insertion_order: VecDeque<String>,
}

#[derive(Clone, Copy, Default)]
struct BidRefreshNotice {
    revision: u64,
    authority_generation: u64,
}

impl PublicBidRefresh {
    pub(crate) fn new() -> Self {
        Self(Signal::new(BidRefreshNotices::default()))
    }

    pub(crate) fn notify(mut self, auction_id: &str, authority_generation: u64) {
        if bid_refresh_visible() {
            let mut notices = self.0.write();
            let revision = notices.latest.revision.saturating_add(1);
            let notice = BidRefreshNotice { revision, authority_generation };
            // 与全局修订号一起更新所属场次，另一场的通知不能覆盖已排定的抽屉补读。
            if !notices.by_auction.contains_key(auction_id) {
                notices.insertion_order.push_back(auction_id.to_owned());
            }
            notices.by_auction.insert(auction_id.to_owned(), notice);
            while notices.insertion_order.len() > MAX_TRACKED_AUCTION_NOTICES {
                if let Some(expired) = notices.insertion_order.pop_front() {
                    notices.by_auction.remove(&expired);
                }
            }
            notices.latest = notice;
        }
    }
}

fn bid_refresh_visible() -> bool {
    #[cfg(test)]
    if let Some(visible) = crate::surface::PageReadFixture::<(), bool>::response(()) {
        return visible;
    }
    document_is_visible()
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
    if current_revision <= consumed_revision {
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
pub(crate) fn use_bid_refresh<T: 'static>(resource: Resource<T>, enabled: bool) {
    use_bid_refresh_scope(
        resource,
        if enabled { BidRefreshScope::Any } else { BidRefreshScope::Disabled },
    );
}

/// 只在所读场次收到普通竞价时补读。
pub(crate) fn use_auction_bid_refresh<T: 'static>(resource: Resource<T>, auction_id: Option<&str>) {
    let scope =
        auction_id.map_or(BidRefreshScope::Disabled, |id| BidRefreshScope::Auction(id.to_owned()));
    use_bid_refresh_scope(resource, scope);
}

#[derive(Clone, PartialEq)]
enum BidRefreshScope {
    Disabled,
    Any,
    Auction(String),
}

impl BidRefreshScope {
    fn latest(&self, notices: &BidRefreshNotices) -> Option<BidRefreshNotice> {
        match self {
            Self::Disabled => None,
            Self::Any => Some(notices.latest),
            Self::Auction(id) => notices.by_auction.get(id).copied(),
        }
    }
}

fn use_bid_refresh_scope<T: 'static>(mut resource: Resource<T>, scope: BidRefreshScope) {
    let refresh = use_context::<PublicBidRefresh>();
    let authority = use_context::<Signal<u64>>();
    let scope = use_memo(use_reactive((&scope,), |(scope,)| scope));
    let mut prior_scope = use_signal(|| scope.peek().clone());
    let mut consumed = use_signal(|| refresh.0.peek().latest.revision);
    let mut scheduled = use_signal(|| None::<Task>);

    use_effect(move || {
        let active = scope();
        if *prior_scope.peek() != active {
            // 场次一变即放弃旧窗口，身份切换不改变场次键或已排定的补读。
            let previous = scheduled.write().take();
            if let Some(previous) = previous {
                previous.cancel();
            }
            prior_scope.set(active.clone());
        }
        if active == BidRefreshScope::Disabled {
            // 闭合的代理面板不订阅竞价或资源状态，避免每行随通知执行空刷新。
            consumed.set(refresh.0.peek().latest.revision);
            return;
        }
        let Some(notice) = active.latest(&refresh.0.read()) else { return };
        let revision = notice.revision;
        if revision <= *consumed.peek() {
            return;
        }
        if notice.authority_generation != *authority.peek() || !bid_refresh_visible() {
            consumed.set(revision);
            return;
        }
        // 先读资源状态以订阅它就绪/重新挂起的变化，再按纯函数决定是否排定。
        let state = resource.state().cloned();
        let decision = plan_bid_refresh(
            revision,
            *consumed.peek(),
            scheduled.peek().is_some(),
            state == UseResourceState::Ready,
        );
        match decision {
            BidRefreshDecision::Idle
            | BidRefreshDecision::Coalescing
            | BidRefreshDecision::WaitForReady => {}
            BidRefreshDecision::Schedule { .. } => {
                let task = spawn(async move {
                    wait_for_bid_refresh().await;
                    scheduled.set(None);
                    if *scope.peek() != active {
                        return;
                    }
                    let Some(notice) = active.latest(&refresh.0.peek()) else { return };
                    let revision = notice.revision;
                    if revision <= *consumed.peek() {
                        return;
                    }
                    let covered = notice.authority_generation != *authority.peek();
                    // 隐藏期间放弃普通竞价补读，重新可见时由外壳权威刷新补偿。
                    if covered || !bid_refresh_visible() {
                        consumed.set(revision);
                    } else if *resource.state().peek() == UseResourceState::Ready {
                        consumed.set(revision);
                        resource.restart();
                    }
                });
                scheduled.set(Some(task));
            }
        }
    });
}

async fn wait_for_bid_refresh() {
    #[cfg(test)]
    if let Some(timer) = super::tests::bid_timer() {
        timer.await;
        return;
    }
    #[cfg(test)]
    if let Some(timer) = crate::surface::PageReadFixture::<
        (),
        std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    >::response(())
    {
        timer.await;
        return;
    }
    #[cfg(test)]
    if let Some(timer) = crate::surface::PageReadFixture::<(), std::future::Ready<()>>::response(())
    {
        timer.await;
        return;
    }
    // 每个窗口各取一次随机样本；抖动只改变重读时刻，不改变合并与消费规则。
    TimeoutFuture::new(coalesce_window_millis(js_sys::Math::random())).await;
}

#[cfg(test)]
mod tests {
    use super::{
        BID_REFRESH_COALESCE_MILLIS, BID_REFRESH_JITTER_MILLIS, BidRefreshDecision,
        coalesce_window_millis, plan_bid_refresh,
    };

    /// 用“窗口从首条通知开始、不因后续通知延长”的同一规则模拟一个资源的重读次数。
    /// 只建模合并策略本身，并按基础窗口计时（抖动只推迟截止时刻，边界另测）；真实请求仍由
    /// 权威接口完成，本测试不替代线上度量。
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

    #[test]
    fn jittered_window_spans_base_to_exclusive_upper_bound() {
        let upper = BID_REFRESH_COALESCE_MILLIS + BID_REFRESH_JITTER_MILLIS;
        assert_eq!((BID_REFRESH_COALESCE_MILLIS, upper), (100, 250));
        // 样本 0 取基础窗口，样本趋近 1 仍小于上界。
        assert_eq!(coalesce_window_millis(0.0), 100);
        assert_eq!(coalesce_window_millis(0.5), 175);
        assert_eq!(coalesce_window_millis(1.0 - f64::EPSILON), 249);
        // 随机源越界或给出非有限值时也不越出窗口。
        assert_eq!(coalesce_window_millis(1.0), 249);
        assert_eq!(coalesce_window_millis(-0.25), 100);
        assert_eq!(coalesce_window_millis(f64::NAN), 100);
        assert_eq!(coalesce_window_millis(f64::INFINITY), 100);
        for step in 0..1_000 {
            let window = coalesce_window_millis(f64::from(step) / 1_000.0);
            assert!((100..upper).contains(&window), "step={step} window={window}");
        }
    }
}
