//! Public 看板的实时连接样式和刷新事件判定。

use super::*;

/// Public 页头的 readiness 有限状态；只决定外层连接徽章，不改写核心实时四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicReadinessState {
    Checking,
    Ready,
    Degraded,
}

/// Public 页头在核心实时四态之外展示的完整七态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicConnectionState {
    Connecting,
    Degraded,
    Live,
    Manual,
    /// 已登录、展示场次处于预览期：开拍后外壳会按 `Auction` 目标自动建立连接。
    Preview,
    /// 展示场次不在竞价期（已结束或没有场次），本来就不需要实时连接。
    Idle,
    Unavailable,
}

/// 旧版 Public 外壳在任一时刻只拥有一个实时目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicRealtimeTarget {
    Checking,
    Anonymous,
    SessionError,
    /// 页面目标的拍卖身份读取失败；不拿 URL 世界或默认 Main 代替权威归属。
    IdentityError {
        target_id: String,
    },
    /// 已确认未进入竞价期；世界属于目标身份，切换世界不会沿用旧连接目标。
    Inactive {
        world: String,
        preview: bool,
    },
    /// 当前场次读取失败，不把失败伪装为没有拍卖或健康的实时连接。
    CurrentError {
        world: String,
    },
    Auction {
        auction_id: String,
        world: String,
        preview_revision: i64,
    },
}

/// 页面实时目标必须绑定明确请求，不能跨显式拍卖查询复用。
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum PublicRealtimePageKey {
    Auction(String),
}

/// 页面公布已按拍卖 UUID 解析的实时目标；外壳只接纳当前请求身份的结果。
#[derive(Clone, Copy)]
pub(crate) struct PublicPageRealtimeTarget(
    pub Signal<Option<(PublicRealtimePageKey, PublicRealtimeTarget)>>,
);

/// 拍卖页按显式 `auction_id` 展示的场次摘要。外壳 ticker 承担页面标题，必须与正文是同一场次；
/// 页面离开或请求变化后残留的值由 [`ticker_auction`] 按拍卖 UUID 拒绝。
#[derive(Clone, Copy)]
pub(crate) struct PublicDisplayedAuction(pub Signal<Option<CurrentAuction>>);

/// 选出 ticker 展示的场次。
///
/// 显式请求某一场次时只接纳页面已加载的同一场次：加载中、加载失败或身份不符都返回 `None`，
/// 不借用所选世界当前场次的编号、状态与倒计时去标注另一场的目录。其余页面显示当前场次。
pub(crate) fn ticker_auction<'a>(
    requested: Option<&PublicRealtimePageKey>,
    displayed: Option<&'a CurrentAuction>,
    current: Option<&'a CurrentAuction>,
) -> Option<&'a CurrentAuction> {
    match requested {
        Some(PublicRealtimePageKey::Auction(requested)) => {
            displayed.filter(|auction| &auction.id == requested)
        }
        _ => current,
    }
}

/// 顶栏倒计时数向哪个时刻；决定标签与到点后的提示文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TickerCountdown {
    /// 竞价期（live/final）数到计划截止。
    Closing { ends_at: String },
    /// 预览期数到开拍。
    Opening { opens_at: String },
}

/// 按展示场次的状态选出顶栏倒计时。
///
/// 只有竞价期与预览期有“还剩多久”的含义；已结算、结算中、取消等状态即使携带历史截止
/// 时刻也不显示，否则过去的截止会被渲染成“落锤中…”。
pub(crate) fn ticker_countdown(auction: &CurrentAuction) -> Option<TickerCountdown> {
    match auction.status.as_str() {
        "live" | "final" => {
            auction.scheduled_ends_at.clone().map(|ends_at| TickerCountdown::Closing { ends_at })
        }
        "preview" => Some(TickerCountdown::Opening { opens_at: auction.opens_at.clone() }),
        _ => None,
    }
}

impl PublicRealtimeTarget {
    pub(crate) fn from_auction(auction: &CurrentAuction) -> Self {
        if matches!(auction.status.as_str(), "live" | "final") {
            Self::Auction {
                auction_id: auction.id.clone(),
                world: auction.world.clone(),
                preview_revision: auction.preview_revision,
            }
        } else {
            Self::Inactive { world: auction.world.clone(), preview: auction.status == "preview" }
        }
    }
}

/// 重连退避起点：1 秒起步，避免旧版固定 3 秒在故障初期空等。
const RECONNECT_BASE_MILLIS: u32 = 1_000;
/// 重连退避上限：指数翻倍到 30 秒封顶，限制服务端故障期间的重试压力。
const RECONNECT_MAX_MILLIS: u32 = 30_000;
/// 连续快速失败达到该次数后认定实时通道不可用：转 Manual 态，页头降级为手动刷新。
const RECONNECT_MAX_FAILURES: u32 = 8;
/// 单次连接存活超过该时长视为健康会话，断开后重置退避计数。
const RECONNECT_HEALTHY_MILLIS: i64 = 30_000;

/// 指数退避 + 随机抖动：1s 起步 ×2 至 30s 上限，实际等待取 [base/2, base] 区间，
/// 打散多客户端在服务端恢复后的重连同步。
fn reconnect_delay_millis(failures: u32) -> u32 {
    // 1 << 5 = 32 已覆盖 30s 上限，更大的失败计数不会再抬升底数。
    let shift = failures.saturating_sub(1).min(5);
    let base = RECONNECT_BASE_MILLIS.saturating_mul(1_u32 << shift).min(RECONNECT_MAX_MILLIS);
    let half = base / 2;
    half + (js_sys::Math::random() * f64::from(base - half)) as u32
}

/// 根据共享 session/current 快照建立唯一实时连接；目标或用户变化时取消旧任务并重新握手。
#[component]
pub(crate) fn PublicRealtimeOwner(
    target: PublicRealtimeTarget,
    user_id: Option<String>,
    refresh_generation: Signal<u64>,
    mut discovery_after: Signal<i64>,
    mut auction_after: Signal<Option<(String, i64)>>,
    mut realtime_state: Signal<LiveConnectionState>,
) -> Element {
    let page_visibility = use_context::<PageVisibility>();
    let bid_refresh = use_context::<PublicBidRefresh>();
    // 身份必须进入资源依赖；单独修改静态组件的 key 不会使相等的 target 重启订阅。
    let _stream = use_resource(use_reactive((&target, &user_id), move |(target, _)| {
        let page_visibility = page_visibility.clone();
        async move {
            let (auction_target, cache) = match target {
                PublicRealtimeTarget::Checking => {
                    realtime_state.set(LiveConnectionState::Connecting);
                    return;
                }
                PublicRealtimeTarget::Anonymous => {
                    realtime_state.set(LiveConnectionState::Manual);
                    return;
                }
                PublicRealtimeTarget::SessionError => {
                    // 旧版无法确认认证身份时不会建立实时房间，外层按匿名手动刷新模式显示。
                    realtime_state.set(LiveConnectionState::Manual);
                    return;
                }
                PublicRealtimeTarget::Inactive { .. } => {
                    // 核心实时四态保持原契约；页头由明确停用目标显示 idle/预览，且不创建 WS。
                    realtime_state.set(LiveConnectionState::Manual);
                    return;
                }
                PublicRealtimeTarget::CurrentError { .. }
                | PublicRealtimeTarget::IdentityError { .. } => {
                    realtime_state.set(LiveConnectionState::Degraded);
                    return;
                }
                PublicRealtimeTarget::Auction { auction_id, world, preview_revision } => {
                    let initial_after = auction_after
                        .peek()
                        .as_ref()
                        .filter(|(known_auction_id, _)| known_auction_id == &auction_id)
                        .map_or(0, |(_, after)| *after);
                    let cache = Rc::new(RefCell::new(RealtimeCache::new_after(
                        auction_id.clone(),
                        preview_revision,
                        initial_after,
                    )));
                    (Some((auction_id, world)), Some(cache))
                }
            };

            let mut consecutive_failures = 0_u32;
            loop {
                realtime_state.set(LiveConnectionState::Connecting);
                let after = if let Some(cache) = cache.as_ref() {
                    cache.borrow().after()
                } else {
                    // 游标只用于建立本轮 catch-up 起点，不是资源依赖；普通读取会让本任务在
                    // 回写游标时取消并重建，重新形成毫秒级连接循环。
                    *discovery_after.peek()
                };
                let cursor = Rc::new(Cell::new(after));
                let callback_cursor = Rc::clone(&cursor);
                let callback_cache = cache.clone();
                let callback_auction_id =
                    auction_target.as_ref().map(|(auction_id, _)| auction_id.clone());
                let mut callback_discovery_after = discovery_after;
                let mut callback_auction_after = auction_after;
                let mut callback_state = realtime_state;
                let mut callback_refresh = refresh_generation;
                let on_page = move |page: crate::api::RealtimePage| {
                    let (refresh_required, bids_changed, continue_catch_up, degraded, next_after) =
                        if let Some(cache) = callback_cache.as_ref() {
                            let merge = cache.borrow_mut().merge_page(&page);
                            let next_after = cache.borrow().after();
                            (
                                merge.refresh_required,
                                merge.bids_changed,
                                merge.continue_catch_up,
                                merge.degraded,
                                next_after,
                            )
                        } else {
                            (
                                page.refetch_required
                                    || page.events.iter().any(should_refresh_auction_board),
                                false,
                                !page.refetch_required,
                                page.refetch_required,
                                callback_cursor.get().max(page.next_after),
                            )
                        };
                    callback_cursor.set(next_after);
                    if callback_cache.is_none() {
                        // current-auction 重读可能让目标短暂切到 Checking 并重启订阅；先把
                        // discovery 游标写回外壳，避免从 0 重放同一事件并形成请求风暴。
                        callback_discovery_after.set(next_after);
                    } else if let Some(auction_id) = callback_auction_id.as_ref() {
                        callback_auction_after.set(Some((auction_id.clone(), next_after)));
                    }
                    // Manual 只由永久错误/重试耗尽进入；普通缺口恢复不应永久粘在 Degraded。
                    if *callback_state.peek() != LiveConnectionState::Manual {
                        callback_state.set(if degraded {
                            LiveConnectionState::Degraded
                        } else {
                            LiveConnectionState::Live
                        });
                    }
                    if refresh_required || bids_changed {
                        // 实时事件已改变服务端权威快照：在唯一的事件处理点整体失效公开读缓存，
                        // 避免随后的资源重读命中 TTL 旧值；不按行建立额外订阅。
                        crate::api::invalidate_all();
                    }
                    if refresh_required {
                        let next_refresh = callback_refresh.peek().saturating_add(1);
                        callback_refresh.set(next_refresh);
                    } else if bids_changed {
                        // 同页的生命周期/缺口刷新优先；普通竞价只使相关读取资源失效。
                        bid_refresh.notify();
                    }
                    continue_catch_up
                };
                let attempt_started_millis = estimated_now().timestamp_millis();
                let result = if let Some((auction_id, world)) = auction_target.as_ref() {
                    subscribe_auction_realtime(after, auction_id.clone(), world.clone(), on_page)
                        .await
                } else {
                    subscribe_realtime(after, on_page).await
                };
                if cache.is_none() {
                    discovery_after.set(cursor.get());
                }
                match &result {
                    Err(error) if error.is_permanent() => {
                        // 永久错误（拍卖 ID 无效、认证失败等）重连不能自愈：转 Manual 态，
                        // 页头降级为手动刷新，不再自动重连。
                        tracing::warn!(
                            detail = error.detail(),
                            "实时订阅遇到永久错误，转手动刷新态"
                        );
                        realtime_state.set(LiveConnectionState::Manual);
                        return;
                    }
                    Err(error) => {
                        // 网络类等可重试错误按退避策略重连；明细只进诊断日志。
                        tracing::debug!(detail = error.detail(), "实时订阅结束，按退避策略重连");
                    }
                    Ok(()) => {}
                }
                // 连接存活足够久视为健康会话，断开后重置退避；快速失败累计退避。
                let survived_millis = estimated_now().timestamp_millis() - attempt_started_millis;
                if survived_millis >= RECONNECT_HEALTHY_MILLIS {
                    consecutive_failures = 0;
                } else {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                }
                if consecutive_failures >= RECONNECT_MAX_FAILURES {
                    // 连续快速失败达到上限：放弃自动重连，转 Manual 态交还手动刷新。
                    realtime_state.set(LiveConnectionState::Manual);
                    return;
                }
                realtime_state.set(LiveConnectionState::Degraded);
                let delay_millis = reconnect_delay_millis(consecutive_failures.max(1));
                // 页面隐藏时挂起退避（不重置剩余时长），恢复可见后继续。
                sleep_while_visible(&page_visibility, delay_millis).await;
            }
        }
    }));

    rsx! {}
}

/// readiness 与身份仍优先；读取失败显示降级，不借用旧实时成功态。
/// 停用目标不建立连接：预览期对已登录用户说明开拍后自动连接，匿名用户与竞价期一样是
/// 手动刷新；其余停用场次显示无需实时，不呈现为“未启用”的故障观感。
pub(crate) fn public_connection_state(
    readiness: PublicReadinessState,
    session_pending: bool,
    authenticated: bool,
    target: &PublicRealtimeTarget,
    live_state: LiveConnectionState,
) -> PublicConnectionState {
    if readiness == PublicReadinessState::Checking {
        return PublicConnectionState::Connecting;
    }
    if readiness != PublicReadinessState::Ready {
        return PublicConnectionState::Unavailable;
    }
    if session_pending {
        return PublicConnectionState::Connecting;
    }
    match target {
        PublicRealtimeTarget::Inactive { preview: true, .. } => {
            return if authenticated {
                PublicConnectionState::Preview
            } else {
                PublicConnectionState::Manual
            };
        }
        PublicRealtimeTarget::Inactive { preview: false, .. } => {
            return PublicConnectionState::Idle;
        }
        PublicRealtimeTarget::CurrentError { .. } | PublicRealtimeTarget::IdentityError { .. } => {
            return PublicConnectionState::Degraded;
        }
        PublicRealtimeTarget::Checking => return PublicConnectionState::Connecting,
        _ => {}
    }
    if !authenticated {
        return PublicConnectionState::Manual;
    }
    match live_state {
        LiveConnectionState::Connecting => PublicConnectionState::Connecting,
        LiveConnectionState::Degraded => PublicConnectionState::Degraded,
        LiveConnectionState::Live => PublicConnectionState::Live,
        LiveConnectionState::Manual => PublicConnectionState::Manual,
    }
}

pub(crate) fn should_refresh_auction_board(event: &RealtimeEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "auction.created"
            | "auction.preview_updated"
            | "preview_catalog_changed"
            | "auction.opened"
            | "auction.schedule_updated"
            | "auction.final_phase_entered"
            | "auction.early_settlement_requested"
            | "auction.cancelled"
            | "auction.settling"
            | "auction.settled"
            | "lot.bid_accepted"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        LiveConnectionState, PublicConnectionState, PublicReadinessState, PublicRealtimePageKey,
        PublicRealtimeTarget, TickerCountdown, public_connection_state, ticker_auction,
        ticker_countdown,
    };
    use crate::api::CurrentAuction;

    fn auction(id: &str, number: i64, status: &str) -> CurrentAuction {
        CurrentAuction {
            id: id.to_owned(),
            world: "main".to_owned(),
            number,
            status: status.to_owned(),
            opens_at: "2026-11-01T00:00:00Z".to_owned(),
            scheduled_ends_at: Some("2026-11-08T00:00:00Z".to_owned()),
            presentation_mode: "live".to_owned(),
            bidding_open: status == "live",
            preview_revision: 1,
        }
    }

    #[test]
    fn ticker_labels_the_auction_that_the_page_displays() {
        let current = auction("current", 12, "live");
        let historical = auction("historical", 8, "settled");
        let requested = PublicRealtimePageKey::Auction("historical".to_owned());

        // 没有显式场次请求的页面显示所选世界的当前场次。
        assert_eq!(ticker_auction(None, Some(&historical), Some(&current)), Some(&current));

        // 历史场次页显示正文同一场次，而不是当前场次的编号、状态与倒计时。
        assert_eq!(
            ticker_auction(Some(&requested), Some(&historical), Some(&current)),
            Some(&historical)
        );
        // 尚未加载、加载失败或残留的是另一场次时不借用当前场次。
        assert_eq!(ticker_auction(Some(&requested), None, Some(&current)), None);
        let stale = auction("another", 7, "settled");
        assert_eq!(ticker_auction(Some(&requested), Some(&stale), Some(&current)), None);
    }

    #[test]
    fn ticker_counts_down_to_close_while_bidding_and_to_open_during_preview() {
        for status in ["live", "final"] {
            assert_eq!(
                ticker_countdown(&auction("a", 5, status)),
                Some(TickerCountdown::Closing { ends_at: "2026-11-08T00:00:00Z".to_owned() })
            );
        }
        let mut preview = auction("a", 5, "preview");
        preview.scheduled_ends_at = None;
        assert_eq!(
            ticker_countdown(&preview),
            Some(TickerCountdown::Opening { opens_at: "2026-11-01T00:00:00Z".to_owned() })
        );
        let mut live_without_end = auction("a", 5, "live");
        live_without_end.scheduled_ends_at = None;
        assert_eq!(ticker_countdown(&live_without_end), None);
    }

    #[test]
    fn ticker_hides_the_countdown_once_bidding_is_over() {
        // 历史场次携带过去的截止时刻；不能再渲染成“落锤中…”。
        for status in ["settling", "settled", "archived", "cancelled", "draft"] {
            assert_eq!(ticker_countdown(&auction("a", 4, status)), None, "{status}");
        }
    }

    #[test]
    fn inactive_targets_explain_why_no_realtime_connection_exists() {
        let state = |authenticated: bool, preview: bool| {
            public_connection_state(
                PublicReadinessState::Ready,
                false,
                authenticated,
                &PublicRealtimeTarget::Inactive { world: "main".to_owned(), preview },
                LiveConnectionState::Manual,
            )
        };
        assert_eq!(state(true, true), PublicConnectionState::Preview);
        // 匿名用户开拍后也不会建立连接，与竞价期一致显示手动刷新。
        assert_eq!(state(false, true), PublicConnectionState::Manual);
        assert_eq!(state(true, false), PublicConnectionState::Idle);
        assert_eq!(state(false, false), PublicConnectionState::Idle);
    }
}
