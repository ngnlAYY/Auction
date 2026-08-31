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
    Discovery,
    Auction { auction_id: String, world: String, preview_revision: i64 },
}

impl PublicRealtimeTarget {
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Checking => "checking".to_owned(),
            Self::Anonymous => "anonymous".to_owned(),
            Self::SessionError => "session-error".to_owned(),
            Self::Discovery => "discovery".to_owned(),
            Self::Auction { auction_id, world, preview_revision } => {
                format!("auction:{auction_id}:world:{world}:preview:{preview_revision}")
            }
        }
    }
}

/// 根据共享 session/current 快照建立唯一实时连接；目标变化时取消旧任务并按新目标重建。
#[component]
pub(crate) fn PublicRealtimeOwner(
    target: PublicRealtimeTarget,
    refresh_generation: Signal<u64>,
    mut discovery_after: Signal<i64>,
    mut realtime_state: Signal<LiveConnectionState>,
) -> Element {
    let _stream = use_resource(use_reactive((&target,), move |(target,)| async move {
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
            PublicRealtimeTarget::Discovery => (None, None),
            PublicRealtimeTarget::Auction { auction_id, world, preview_revision } => {
                let cache =
                    Rc::new(RefCell::new(RealtimeCache::new(auction_id.clone(), preview_revision)));
                (Some((auction_id, world)), Some(cache))
            }
        };

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
            let mut callback_discovery_after = discovery_after;
            let mut callback_state = realtime_state;
            let mut callback_refresh = refresh_generation;
            let on_page = move |page: crate::api::RealtimePage| {
                let (refresh_required, continue_catch_up, degraded, next_after) =
                    if let Some(cache) = callback_cache.as_ref() {
                        let merge = cache.borrow_mut().merge_page(&page);
                        let next_after = cache.borrow().after();
                        (
                            merge.refresh_required,
                            merge.continue_catch_up,
                            merge.degraded,
                            next_after,
                        )
                    } else {
                        (
                            page.refetch_required
                                || page.events.iter().any(should_refresh_auction_board),
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
                }
                if degraded {
                    callback_state.set(LiveConnectionState::Degraded);
                } else if *callback_state.peek() != LiveConnectionState::Degraded {
                    callback_state.set(LiveConnectionState::Live);
                }
                if refresh_required {
                    let next_refresh = callback_refresh.peek().saturating_add(1);
                    callback_refresh.set(next_refresh);
                }
                continue_catch_up
            };
            let _result = if let Some((auction_id, world)) = auction_target.as_ref() {
                subscribe_auction_realtime(after, auction_id.clone(), world.clone(), on_page).await
            } else {
                subscribe_realtime(after, on_page).await
            };
            if cache.is_none() {
                discovery_after.set(cursor.get());
            }
            realtime_state.set(LiveConnectionState::Degraded);
            TimeoutFuture::new(3_000).await;
        }
    }));

    rsx! {}
}

/// 按旧 `PublicApp` 的优先级把 readiness、session、current 与核心实时四态合成为页头六态。
pub(crate) fn public_connection_state(
    readiness: PublicReadinessState,
    session_pending: bool,
    authenticated: bool,
    has_current_auction: bool,
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
    if !authenticated {
        return PublicConnectionState::Manual;
    }
    if !has_current_auction && live_state == LiveConnectionState::Live {
        return PublicConnectionState::Idle;
    }
    if !has_current_auction && live_state == LiveConnectionState::Degraded {
        return PublicConnectionState::Degraded;
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
            | "auction.final_phase_entered"
            | "auction.early_settlement_requested"
            | "auction.cancelled"
            | "auction.settling"
            | "auction.settled"
            | "lot.bid_accepted"
    )
}
