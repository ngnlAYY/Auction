//! Public 持久化实时事件 API 客户端。
//! 浏览器只解码经过服务端事件注册表校验的安全信封。WebSocket 沿用旧版逐事件 wire；
//! 浏览器先建立连接再执行 HTTP catch-up，并在序列跳号时重新补齐，不在本地猜测权威状态。

pub(crate) use auction_contracts::PublicRealtimeEventResponse as RealtimeEvent;
#[cfg(target_arch = "wasm32")]
use futures_util::{
    Sink, StreamExt,
    future::{Either, poll_fn, select},
    pin_mut,
};
use std::collections::{BTreeMap, HashSet, VecDeque};
#[cfg(target_arch = "wasm32")]
use std::pin::Pin;
#[cfg(target_arch = "wasm32")]
use url::form_urlencoded;
#[cfg(target_arch = "wasm32")]
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use super::send_get;

#[cfg(target_arch = "wasm32")]
use gloo_net::websocket::{Message, State as WebSocketState, futures::WebSocket};
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;

/// 页面消费的统一实时页；wire 上的旧版单拍卖页和显式发现页在完成目标校验后才投影到这里。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RealtimePage {
    pub events: Vec<RealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

const SEEN_REALTIME_LIMIT: usize = 256;

/// 有界防重集合：HashSet 提供 O(1) 成员判断，VecDeque 记录插入顺序用于先进先出淘汰，
/// 维持 256 条上限语义；接受新 bid 时原地更新，不复制整个集合。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenSet {
    order: VecDeque<String>,
    members: HashSet<String>,
}

impl SeenSet {
    fn new() -> Self {
        Self { order: VecDeque::new(), members: HashSet::new() }
    }

    fn contains(&self, value: &str) -> bool {
        self.members.contains(value)
    }

    /// 记录一个新键；超出上限时返回被淘汰的最旧键，供关联缓存同步删除。
    /// 重复键不改变其原有位置：调用方在重复路径上本就直接忽略事件，
    /// 不会走到这里依赖重排续期。
    fn remember(&mut self, value: String) -> Option<String> {
        if !self.members.insert(value.clone()) {
            return None;
        }
        self.order.push_back(value);
        if self.order.len() > SEEN_REALTIME_LIMIT
            && let Some(oldest) = self.order.pop_front()
        {
            self.members.remove(&oldest);
            return Some(oldest);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RealtimePageMerge {
    /// 拍卖级变化或恢复缺口要求重读全部权威快照。
    pub refresh_required: bool,
    /// 仅已通过防重、事实和版本校验的竞价通知对应资源失效。
    pub bids_changed: bool,
    pub continue_catch_up: bool,
    pub degraded: bool,
}

/// 实时订阅失败的结构化错误：页面层按 `kind` 选择重连策略，禁止对错误文案做字符串比较。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealtimeError {
    kind: RealtimeErrorKind,
    /// 失败明细只用于诊断日志，不参与重连决策。
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeErrorKind {
    /// 重连无法自愈：游标或拍卖 ID 无效、认证/权限被拒绝、目标拍卖不存在。
    Permanent,
    /// 网络抖动、服务端瞬时故障、毒帧超限或握手超时等可重试失败。
    #[cfg(target_arch = "wasm32")]
    Transient,
}

impl RealtimeError {
    fn permanent(detail: impl Into<String>) -> Self {
        Self { kind: RealtimeErrorKind::Permanent, detail: detail.into() }
    }

    #[cfg(target_arch = "wasm32")]
    fn transient(detail: impl Into<String>) -> Self {
        Self { kind: RealtimeErrorKind::Transient, detail: detail.into() }
    }

    pub(crate) const fn is_permanent(&self) -> bool {
        matches!(self.kind, RealtimeErrorKind::Permanent)
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// 只限制连接建立阶段；业务消息空闲不能证明断线，协议 Ping/Pong 不暴露给浏览器 API。
#[cfg(target_arch = "wasm32")]
const REALTIME_CONNECT_TIMEOUT_MILLIS: u32 = 10_000;
/// 连续毒帧上限：单帧解码失败只计数降级，连续 5 帧失败才断开整条订阅。
#[cfg(target_arch = "wasm32")]
const MAX_CONSECUTIVE_BAD_FRAMES: u32 = 5;
/// HTTP catch-up 翻页上限：落后超过 50 页（5000 事件）说明逐页重放代价过大，
/// 转为全量 refetch 降级，由各 resource 重读权威快照。
#[cfg(target_arch = "wasm32")]
const MAX_CATCH_UP_PAGES: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastBidFact {
    fact_index: i64,
    state_version: i64,
}

/// 旧版单拍卖浏览器维护的有界实时防重与版本栅栏。
/// `after` 是 stream cursor；event ID 与 bid fact identity 是两套独立防重键。发生事实冲突、
/// state/fact 回退或 sequence 缺口时清空局部事实缓存，但保留已确认的最大 cursor。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RealtimeCache {
    auction_id: String,
    initial_preview_revision: i64,
    after: i64,
    /// refetch 后每个尚未见过的 lot 可用首个连续 stream 事件建立一次事实基线。
    baseline_after: Option<i64>,
    fact_identities: BTreeMap<String, String>,
    last_facts: BTreeMap<String, LastBidFact>,
    seen: SeenSet,
    seen_facts: SeenSet,
    versions: BTreeMap<String, i64>,
}

impl RealtimeCache {
    pub(crate) fn new(auction_id: String, preview_revision: i64) -> Self {
        let versions = BTreeMap::from([(format!("preview:{auction_id}"), preview_revision)]);
        Self {
            auction_id,
            initial_preview_revision: preview_revision,
            after: 0,
            baseline_after: None,
            fact_identities: BTreeMap::new(),
            last_facts: BTreeMap::new(),
            seen: SeenSet::new(),
            seen_facts: SeenSet::new(),
            versions,
        }
    }

    /// owner 因权威快照 revision 变化而重建时，从已确认 cursor 建立新基线，避免从零重放。
    pub(crate) fn new_after(auction_id: String, preview_revision: i64, after: i64) -> Self {
        let mut cache = Self::new(auction_id, preview_revision);
        cache.after = after.max(0);
        cache.baseline_after = (cache.after > 0).then_some(cache.after);
        cache
    }

    pub(crate) const fn after(&self) -> i64 {
        self.after
    }

    /// 按旧版 `sync()` 顺序合并一页：refetch 页不消费事件，普通页先逐事件合并，再接受
    /// `next_after` 的单调前移。返回 false 时只停止本轮 HTTP catch-up，不关闭 WebSocket。
    pub(crate) fn merge_page(&mut self, page: &RealtimePage) -> RealtimePageMerge {
        if page.refetch_required {
            self.reset_after(page.next_after);
            return RealtimePageMerge {
                refresh_required: true,
                bids_changed: false,
                continue_catch_up: false,
                degraded: true,
            };
        }

        let mut refresh_required = false;
        let mut bids_changed = false;
        for event in &page.events {
            match self.merge_event(event) {
                EventMerge::Ignored => {}
                EventMerge::Applied { refresh } => refresh_required |= refresh,
                EventMerge::BidApplied => bids_changed = true,
                EventMerge::Gap => {
                    self.reset_after(page.next_after);
                    return RealtimePageMerge {
                        refresh_required: true,
                        bids_changed: false,
                        continue_catch_up: false,
                        degraded: true,
                    };
                }
            }
        }
        if page.next_after > self.after {
            self.after = page.next_after;
        }
        RealtimePageMerge {
            refresh_required,
            bids_changed,
            continue_catch_up: true,
            degraded: false,
        }
    }

    fn merge_event(&mut self, event: &RealtimeEvent) -> EventMerge {
        if event.stream_sequence <= self.after || self.seen.contains(&event.event_id) {
            return EventMerge::Ignored;
        }
        if self.after.checked_add(1) != Some(event.stream_sequence) {
            return EventMerge::Gap;
        }
        if event.kind == "lot.bid_accepted" {
            return self.merge_accepted_bid(event);
        }
        let Some((key, version)) = event_version(event) else { return EventMerge::Gap };
        let newer = version > self.versions.get(&key).copied().unwrap_or(0);
        self.after = event.stream_sequence;
        let _ = self.seen.remember(event.event_id.clone());
        if newer {
            self.versions.insert(key, version);
        }
        EventMerge::Applied { refresh: newer }
    }

    fn merge_accepted_bid(&mut self, event: &RealtimeEvent) -> EventMerge {
        let Some(payload) = event.payload.as_object() else { return EventMerge::Gap };
        let Some(lot_id) = payload.get("lot_id").and_then(serde_json::Value::as_str) else {
            return EventMerge::Gap;
        };
        let Some(bid_id) = payload.get("bid_id").and_then(serde_json::Value::as_str) else {
            return EventMerge::Gap;
        };
        let Some(state_version) = payload.get("state_version").and_then(serde_json::Value::as_i64)
        else {
            return EventMerge::Gap;
        };
        let Some(fact_index) = payload.get("fact_index").and_then(serde_json::Value::as_i64) else {
            return EventMerge::Gap;
        };
        let Some(amount) = payload.get("amount").and_then(serde_json::Value::as_i64) else {
            return EventMerge::Gap;
        };
        let Some(mechanism) = payload.get("mechanism").and_then(serde_json::Value::as_str) else {
            return EventMerge::Gap;
        };
        let Some(effective_end) = payload.get("effective_end").and_then(serde_json::Value::as_str)
        else {
            return EventMerge::Gap;
        };
        let identity =
            format!("{lot_id}:{state_version}:{fact_index}:{amount}:{mechanism}:{effective_end}");
        if let Some(stored_identity) = self.fact_identities.get(bid_id) {
            if stored_identity != &identity {
                return EventMerge::Gap;
            }
            let _ = self.seen.remember(event.event_id.clone());
            self.after = event.stream_sequence;
            return EventMerge::Applied { refresh: false };
        }

        if self
            .versions
            .get(lot_id)
            .is_some_and(|previous| state_version != previous.saturating_add(1))
            || !self.ordered_fact_index(lot_id, state_version, fact_index, event.stream_sequence)
        {
            return EventMerge::Gap;
        }

        // 校验全部通过后才写入状态，Gap 路径不会污染防重集合。
        let _ = self.seen.remember(event.event_id.clone());
        if let Some(evicted) = self.seen_facts.remember(bid_id.to_owned()) {
            self.fact_identities.remove(&evicted);
        }
        self.fact_identities.insert(bid_id.to_owned(), identity);
        self.last_facts.insert(lot_id.to_owned(), LastBidFact { fact_index, state_version });
        self.versions.insert(lot_id.to_owned(), state_version);
        self.after = event.stream_sequence;
        EventMerge::BidApplied
    }

    fn ordered_fact_index(
        &self,
        lot_id: &str,
        state_version: i64,
        fact_index: i64,
        stream_sequence: i64,
    ) -> bool {
        state_version > 0
            && fact_index > 0
            && (fact_index == 1
                || self.last_facts.get(lot_id).is_some_and(|previous| {
                    previous.state_version == state_version - 1
                        && previous.fact_index + 1 == fact_index
                })
                || (!self.last_facts.contains_key(lot_id)
                    && self
                        .baseline_after
                        .is_some_and(|baseline_after| stream_sequence > baseline_after)))
    }

    fn reset_after(&mut self, next_after: i64) {
        let after = self.after.max(next_after);
        *self = Self::new(self.auction_id.clone(), self.initial_preview_revision);
        self.after = after;
        self.baseline_after = Some(after);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventMerge {
    Ignored,
    Applied { refresh: bool },
    BidApplied,
    Gap,
}

fn event_version(event: &RealtimeEvent) -> Option<(String, i64)> {
    let payload = event.payload.as_object()?;
    let auction_id = payload.get("auction_id")?.as_str()?;
    match event.kind.as_str() {
        "auction.created" => {
            Some((format!("created:{auction_id}"), payload.get("state_version")?.as_i64()?))
        }
        "auction.preview_updated" | "preview_catalog_changed" => {
            Some((format!("preview:{auction_id}"), payload.get("preview_revision")?.as_i64()?))
        }
        "auction.opened" => {
            Some((format!("opened:{auction_id}"), payload.get("preview_revision")?.as_i64()?))
        }
        "auction.final_phase_entered"
        | "auction.schedule_updated"
        | "auction.early_settlement_requested"
        | "auction.cancelled"
        | "auction.settling"
        | "auction.settled" => {
            Some((auction_id.to_owned(), payload.get("state_version")?.as_i64()?))
        }
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_realtime_page(
    after: i64,
    auction_id: Option<&str>,
) -> Result<RealtimePage, RealtimeError> {
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let canonical_auction_id =
        auction_id.map(canonical_auction_id).transpose().map_err(RealtimeError::permanent)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = canonical_auction_id.as_deref() {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    query.append_pair("after", &after.to_string()).append_pair("page_size", "100");
    let url = format!("/api/v1/events?{}", query.finish());
    let response = send_get(&url).await.map_err(|error| RealtimeError::transient(error.code()))?;
    if !response.ok() {
        let status = response.status();
        // 401/403 认证或权限失败、404 目标拍卖不存在：重连不能自愈，按永久错误处理。
        return Err(if matches!(status, 401 | 403 | 404) {
            RealtimeError::permanent(format!("HTTP {status}"))
        } else {
            RealtimeError::transient(format!("HTTP {status}"))
        });
    }
    let page = if let Some(auction_id) = canonical_auction_id.as_deref() {
        let payload = super::decoder::decode_auction_realtime_page(response, auction_id)
            .await
            .map_err(RealtimeError::transient)?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    } else {
        let payload = super::decoder::decode_discovery_realtime_page(response)
            .await
            .map_err(RealtimeError::transient)?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    };
    validate_realtime_page_sequence(&page, after).map_err(RealtimeError::transient)?;
    Ok(page)
}

/// 订阅 Public 持久化实时流，并把服务端事件页交给页面替换当前安全快照。
/// 浏览器 WebSocket 自动携带同源 HttpOnly session Cookie；匿名客户端由服务端拒绝。
/// 连接建立后先走 HTTP catch-up，期间到达的单事件帧由 WebSocket 缓冲，避免补发与订阅间隙。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_realtime<F>(after: i64, on_page: F) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    subscribe_realtime_stream(after, None, None, on_page).await
}

/// 订阅单个拍卖的实时流；拍卖页面用它对齐旧版的 lot/竞价即时刷新边界。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_auction_realtime<F>(
    after: i64,
    auction_id: String,
    world: String,
    on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    subscribe_realtime_stream(after, Some(&auction_id), Some(&world), on_page).await
}

#[cfg(target_arch = "wasm32")]
async fn subscribe_realtime_stream<F>(
    after: i64,
    auction_id: Option<&str>,
    world: Option<&str>,
    mut on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let canonical_auction_id =
        auction_id.map(canonical_auction_id).transpose().map_err(RealtimeError::permanent)?;
    let auction_id = canonical_auction_id.as_deref();
    let url = websocket_url(auction_id).map_err(RealtimeError::permanent)?;
    let mut socket =
        WebSocket::open(&url).map_err(|error| RealtimeError::transient(error.to_string()))?;
    {
        let connected =
            poll_fn(|context| Sink::<Message>::poll_ready(Pin::new(&mut socket), context));
        let deadline = TimeoutFuture::new(REALTIME_CONNECT_TIMEOUT_MILLIS);
        pin_mut!(connected, deadline);
        match select(connected, deadline).await {
            Either::Left((result, _)) => {
                result.map_err(|error| RealtimeError::transient(error.to_string()))?;
            }
            Either::Right(((), _)) => {
                return Err(RealtimeError::transient("实时 WebSocket 握手超时"));
            }
        }
    }
    if !matches!(socket.state(), WebSocketState::Open) {
        return Err(RealtimeError::transient("实时 WebSocket 建立失败"));
    }
    let (_write, mut read) = socket.split();
    let mut expected_after = after;
    catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
    let mut consecutive_bad_frames = 0_u32;
    // 存活检查由服务端 Ping/Pong 拥有；静默拍卖保持连接，关闭/错误才触发恢复。
    while let Some(message) = read.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                return Err(RealtimeError::transient(format!("实时 WebSocket 读取失败：{error}")));
            }
        };
        let Message::Text(message) = message else {
            // 二进制帧不直接终止订阅：按毒帧计数，连续超限才断开重连。
            consecutive_bad_frames += 1;
            if consecutive_bad_frames >= MAX_CONSECUTIVE_BAD_FRAMES {
                return Err(RealtimeError::transient("实时 WebSocket 连续返回不支持的二进制帧"));
            }
            continue;
        };
        let event = match super::decoder::decode_realtime_event(&message, auction_id, world) {
            Ok(event) => event,
            Err(error) => {
                // 单帧解码失败只计毒帧，不终止整条订阅；连续超限才断开重连。
                consecutive_bad_frames += 1;
                if consecutive_bad_frames >= MAX_CONSECUTIVE_BAD_FRAMES {
                    return Err(RealtimeError::transient(format!(
                        "实时事件帧连续解码失败：{error}"
                    )));
                }
                continue;
            }
        };
        consecutive_bad_frames = 0;
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err(RealtimeError::permanent("实时事件序列超过可表示范围"));
        };
        if event.stream_sequence != next_sequence {
            catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
        }
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err(RealtimeError::permanent("实时事件序列超过可表示范围"));
        };
        if event.stream_sequence != next_sequence {
            return Err(RealtimeError::transient("实时事件序列出现断档"));
        }
        expected_after = event.stream_sequence;
        let page = RealtimePage {
            events: vec![event],
            next_after: expected_after,
            has_more: false,
            refetch_required: false,
        };
        let _ = on_page(page);
    }
    Err(RealtimeError::transient("实时 WebSocket 已关闭"))
}

#[cfg(target_arch = "wasm32")]
async fn catch_up_realtime<F>(
    auction_id: Option<&str>,
    expected_after: &mut i64,
    on_page: &mut F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    let mut fetched_pages = 0_u32;
    loop {
        let requested_after = *expected_after;
        let page = fetch_realtime_page(requested_after, auction_id).await?;
        let next_after = page.next_after;
        let has_more = page.has_more;
        let refetch_required = page.refetch_required;
        if has_more && next_after == requested_after {
            return Err(RealtimeError::transient("实时事件补发游标未前进"));
        }
        *expected_after = next_after;
        let continue_catch_up = on_page(page);
        if refetch_required || !continue_catch_up {
            return Ok(());
        }
        if !has_more {
            return Ok(());
        }
        fetched_pages += 1;
        if fetched_pages >= MAX_CATCH_UP_PAGES {
            // 落后页数超过上限：放弃逐页补齐，合成 refetch 页让各 resource 全量重读权威快照。
            tracing::warn!(
                auction_id = ?auction_id,
                after = *expected_after,
                fetched_pages,
                "实时 catch-up 翻页超限，降级为全量 refetch"
            );
            let degraded_page = RealtimePage {
                events: Vec::new(),
                next_after: *expected_after,
                has_more: false,
                refetch_required: true,
            };
            let _ = on_page(degraded_page);
            return Ok(());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_realtime<F>(_after: i64, _on_page: F) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err(RealtimeError::permanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_auction_realtime<F>(
    _after: i64,
    _auction_id: String,
    _world: String,
    _on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err(RealtimeError::permanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"))
}

#[cfg(target_arch = "wasm32")]
fn websocket_url(auction_id: Option<&str>) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用".to_owned())?;
    let location = window.location();
    let protocol = location.protocol().map_err(|_| "读取浏览器协议失败".to_owned())?;
    let host = location.host().map_err(|_| "读取浏览器主机失败".to_owned())?;
    let websocket_protocol = match protocol.as_str() {
        "http:" => "ws:",
        "https:" => "wss:",
        _ => return Err("浏览器协议不支持 WebSocket".to_owned()),
    };
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = auction_id {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    Ok(format!("{websocket_protocol}//{host}/api/v1/ws?{}", query.finish()))
}

#[cfg(target_arch = "wasm32")]
fn validate_realtime_page_sequence(page: &RealtimePage, after: i64) -> Result<(), String> {
    if page.next_after < after {
        return Err("实时游标发生回退".to_owned());
    }
    let mut previous = after;
    for event in &page.events {
        if event.stream_sequence <= previous || event.stream_sequence > page.next_after {
            return Err("实时事件页序列无效".to_owned());
        }
        previous = event.stream_sequence;
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn canonical_auction_id(value: &str) -> Result<String, String> {
    let auction_id = Uuid::parse_str(value).map_err(|_| "拍卖 ID 无效".to_owned())?;
    let canonical = auction_id.to_string();
    if auction_id.is_nil() || canonical != value {
        return Err("拍卖 ID 无效".to_owned());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;
    use serde_json::{Value, json};

    fn event(sequence: i64, event_id: &str, kind: &str, payload: Value) -> RealtimeEvent {
        RealtimeEvent {
            event_id: event_id.to_owned(),
            stream_sequence: sequence,
            kind: kind.to_owned(),
            schema_version: 1,
            payload,
            created_at: "2026-11-04T00:00:00Z".to_owned(),
        }
    }

    fn page(events: Vec<RealtimeEvent>, next_after: i64) -> RealtimePage {
        RealtimePage { events, next_after, has_more: false, refetch_required: false }
    }

    fn preview_update(sequence: i64, revision: i64) -> RealtimeEvent {
        event(
            sequence,
            &format!("preview-{sequence}"),
            "auction.preview_updated",
            json!({"auction_id": "auction-1", "preview_revision": revision}),
        )
    }

    fn bid(
        sequence: i64,
        event_id: &str,
        bid_id: &str,
        state_version: i64,
        fact_index: i64,
        amount: i64,
    ) -> RealtimeEvent {
        event(
            sequence,
            event_id,
            "lot.bid_accepted",
            json!({
                "lot_id": "lot-1",
                "bid_id": bid_id,
                "state_version": state_version,
                "fact_index": fact_index,
                "amount": amount,
                "mechanism": "ordinary",
                "effective_end": "2026-11-04T12:00:00Z"
            }),
        )
    }

    #[test]
    fn newer_revision_refreshes_and_duplicate_event_is_ignored() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 7);
        let event = preview_update(1, 8);

        let merge = cache.merge_page(&page(vec![event.clone()], 1));
        assert!(merge.refresh_required);
        assert!(!merge.bids_changed);
        assert!(merge.continue_catch_up);
        assert!(!merge.degraded);
        assert_eq!(cache.after(), 1);

        // 同一事件重放不能再次触发资源失效，也不能推进已确认游标。
        let merge = cache.merge_page(&page(vec![event], 1));
        assert!(!merge.refresh_required);
        assert!(merge.continue_catch_up);
        assert_eq!(cache.after(), 1);
    }

    #[test]
    fn sequence_gap_requests_refetch_and_keeps_the_page_cursor() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        let merge = cache.merge_page(&page(vec![preview_update(2, 9)], 4));

        assert!(merge.refresh_required);
        assert!(merge.degraded);
        assert!(!merge.continue_catch_up);
        assert_eq!(cache.after(), 4);
    }

    #[test]
    fn stale_events_are_ignored_without_reporting_a_gap() {
        let mut cache = RealtimeCache::new_after("auction-1".to_owned(), 0, 5);
        let merge =
            cache.merge_page(&page(vec![event(3, "stale", "auction.created", json!({}))], 5));

        assert!(!merge.refresh_required);
        assert!(!merge.bids_changed);
        assert!(!merge.degraded);
        assert_eq!(cache.after(), 5);
    }

    #[test]
    fn accepted_bid_marks_only_the_bid_resource_changed() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        let merge = cache.merge_page(&page(vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1));

        assert!(merge.bids_changed);
        assert!(!merge.refresh_required);
        assert!(merge.continue_catch_up);
        assert_eq!(cache.after(), 1);
    }

    #[test]
    fn conflicting_bid_identity_or_state_version_is_a_gap() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        assert!(
            cache
                .merge_page(&page(vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1))
                .bids_changed
        );

        // 同一 bid_id 携带不同金额与版本，说明本地事实与权威流冲突。
        let conflict = bid(2, "bid-event-2", "bid-1", 2, 2, 2_000);
        let merge = cache.merge_page(&page(vec![conflict], 2));
        assert!(merge.degraded);
        assert!(merge.refresh_required);
        assert!(!merge.continue_catch_up);

        // 另一条事实跳过中间版本同样不能接受。
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        assert!(
            cache
                .merge_page(&page(vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1))
                .bids_changed
        );
        let skipped = bid(2, "bid-event-2", "bid-2", 3, 1, 2_000);
        assert!(cache.merge_page(&page(vec![skipped], 2)).degraded);
    }

    #[test]
    fn refetch_page_advances_the_baseline_for_new_lots() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        let refetch = RealtimePage {
            events: vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)],
            next_after: 9,
            has_more: false,
            refetch_required: true,
        };
        let merge = cache.merge_page(&refetch);
        assert!(merge.refresh_required);
        assert!(merge.degraded);
        assert!(!merge.bids_changed);
        assert_eq!(cache.after(), 9);

        // refetch 后的首条事实不要求从 fact_index 1 重建历史。
        let baseline = bid(10, "bid-event-10", "bid-10", 5, 2, 2_000);
        let merge = cache.merge_page(&page(vec![baseline], 10));
        assert!(merge.bids_changed);
        assert!(!merge.degraded);

        // 同一 lot 的后续事实仍须严格连续。
        let regression = bid(11, "bid-event-11", "bid-11", 6, 4, 2_500);
        assert!(cache.merge_page(&page(vec![regression], 11)).degraded);
    }

    #[test]
    fn malformed_bid_payload_or_unknown_event_fails_closed() {
        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        let malformed = event(1, "broken", "lot.bid_accepted", json!({"lot_id": "lot-1"}));
        assert!(cache.merge_page(&page(vec![malformed], 1)).degraded);

        let mut cache = RealtimeCache::new("auction-1".to_owned(), 0);
        let unknown = event(1, "unknown", "auction.unknown", json!({"auction_id": "auction-1"}));
        assert!(cache.merge_page(&page(vec![unknown], 1)).degraded);
    }
}
