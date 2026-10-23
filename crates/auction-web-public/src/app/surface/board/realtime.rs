//! Public 看板的实时连接样式和刷新事件判定。

use super::*;

/// Public 页头的 readiness 有限状态；只决定外层连接徽章，不改写核心实时四态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicReadinessState {
    Checking,
    Ready,
    Degraded,
}

/// 旧版 Public 页头在核心实时四态之外展示的完整六态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicConnectionState {
    Connecting,
    Degraded,
    Live,
    Manual,
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
    /// 已确认未进入竞价期；世界进入 key，切换世界不会保留上一世界的连接目标。
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

/// 页面实时目标必须绑定明确请求，不能跨 lot 或显式拍卖查询复用。
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum PublicRealtimePageKey {
    Lot(String),
    Auction(String),
}

/// 页面公布已按拍卖 UUID 解析的实时目标；外壳只接纳当前请求身份的结果。
#[derive(Clone, Copy)]
pub(crate) struct PublicPageRealtimeTarget(
    pub Signal<Option<(PublicRealtimePageKey, PublicRealtimeTarget)>>,
);

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

    pub(crate) fn key(&self) -> String {
        match self {
            Self::Checking => "checking".to_owned(),
            Self::Anonymous => "anonymous".to_owned(),
            Self::SessionError => "session-error".to_owned(),
            Self::IdentityError { target_id } => format!("identity-error:{target_id}"),
            Self::Inactive { world, preview } => {
                format!("inactive:world:{world}:preview:{preview}")
            }
            Self::CurrentError { world } => format!("current-error:world:{world}"),
            Self::Auction { auction_id, world, preview_revision } => {
                format!("auction:{auction_id}:world:{world}:preview:{preview_revision}")
            }
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

/// 根据共享 session/current 快照建立唯一实时连接；目标变化时取消旧任务并按新目标重建。
#[component]
pub(crate) fn PublicRealtimeOwner(
    target: PublicRealtimeTarget,
    refresh_generation: Signal<u64>,
    mut discovery_after: Signal<i64>,
    mut auction_after: Signal<Option<(String, i64)>>,
    mut realtime_state: Signal<LiveConnectionState>,
) -> Element {
    let page_visibility = use_context::<PageVisibility>();
    let bid_refresh = use_context::<PublicBidRefresh>();
    let _stream = use_resource(use_reactive((&target,), move |(target,)| {
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
                        .map(|(_, after)| *after)
                        .unwrap_or(0);
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
                        // current-auction 刷新会让目标短暂切到 Checking，并按 key 重建 owner；先把
                        // discovery 游标写回外壳，避免重建后从 0 重放同一事件并形成请求风暴。
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

/// readiness 与身份仍优先；明确停用的场次显示 idle，读取失败显示降级，不借用旧实时成功态。
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
        PublicRealtimeTarget::Inactive { .. } => return PublicConnectionState::Idle,
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

pub(crate) fn public_connection_class(state: PublicConnectionState) -> &'static str {
    match state {
        PublicConnectionState::Connecting => "public-connection-badge--connecting",
        PublicConnectionState::Degraded => "public-connection-badge--degraded",
        PublicConnectionState::Live => "public-connection-badge--live",
        PublicConnectionState::Manual => "public-connection-badge--manual",
        PublicConnectionState::Idle => "public-connection-badge--idle",
        PublicConnectionState::Unavailable => "public-connection-badge--unavailable",
    }
}

/// 返回供 Public 页头 `data-state` 使用的稳定六态名称。
pub(crate) fn public_connection_state_name(state: PublicConnectionState) -> &'static str {
    match state {
        PublicConnectionState::Connecting => "connecting",
        PublicConnectionState::Degraded => "degraded",
        PublicConnectionState::Live => "live",
        PublicConnectionState::Manual => "manual",
        PublicConnectionState::Idle => "idle",
        PublicConnectionState::Unavailable => "unavailable",
    }
}

/// 返回供 `data-state` 和实时徽章共同使用的稳定状态名。
pub(crate) fn live_connection_state_name(state: LiveConnectionState) -> &'static str {
    match state {
        LiveConnectionState::Connecting => "connecting",
        LiveConnectionState::Degraded => "degraded",
        LiveConnectionState::Live => "live",
        LiveConnectionState::Manual => "manual",
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
