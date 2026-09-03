//! Public 持久化实时事件 API 客户端。
//!
//! 浏览器只解码经过服务端事件注册表校验的安全信封。WebSocket 沿用旧版逐事件 wire；
//! 浏览器先建立连接再执行 HTTP catch-up，并在序列跳号时重新补齐，不在本地猜测权威状态。

pub(crate) use auction_contracts::PublicRealtimeEventResponse as RealtimeEvent;
#[cfg(target_arch = "wasm32")]
use futures_util::{Sink, StreamExt, future::poll_fn};
use std::collections::BTreeMap;
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

/// 页面消费的统一实时页；wire 上的旧版单拍卖页和显式发现页在完成目标校验后才投影到这里。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RealtimePage {
    pub events: Vec<RealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

const SEEN_REALTIME_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RealtimePageMerge {
    pub refresh_required: bool,
    pub continue_catch_up: bool,
    pub degraded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastBidFact {
    fact_index: i64,
    state_version: i64,
}

/// 旧版单拍卖浏览器维护的有界实时防重与版本栅栏。
///
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
    seen: Vec<String>,
    seen_facts: Vec<String>,
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
            seen: Vec::new(),
            seen_facts: Vec::new(),
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
                continue_catch_up: false,
                degraded: true,
            };
        }

        let mut refresh_required = false;
        for event in &page.events {
            match self.merge_event(event) {
                EventMerge::Ignored => {}
                EventMerge::Applied { refresh } => refresh_required |= refresh,
                EventMerge::Gap => {
                    self.reset_after(page.next_after);
                    return RealtimePageMerge {
                        refresh_required: true,
                        continue_catch_up: false,
                        degraded: true,
                    };
                }
            }
        }
        if page.next_after > self.after {
            self.after = page.next_after;
        }
        RealtimePageMerge { refresh_required, continue_catch_up: true, degraded: false }
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
        remember(&mut self.seen, event.event_id.clone());
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
        let mut seen = self.seen.clone();
        remember(&mut seen, event.event_id.clone());
        if let Some(stored_identity) = self.fact_identities.get(bid_id) {
            if stored_identity != &identity {
                return EventMerge::Gap;
            }
            self.after = event.stream_sequence;
            self.seen = seen;
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

        remember(&mut self.seen_facts, bid_id.to_owned());
        self.fact_identities.retain(|known_bid_id, _| self.seen_facts.contains(known_bid_id));
        self.fact_identities.insert(bid_id.to_owned(), identity);
        self.last_facts.insert(lot_id.to_owned(), LastBidFact { fact_index, state_version });
        self.versions.insert(lot_id.to_owned(), state_version);
        self.after = event.stream_sequence;
        self.seen = seen;
        EventMerge::Applied { refresh: true }
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

fn remember(values: &mut Vec<String>, value: String) {
    values.push(value);
    if values.len() > SEEN_REALTIME_LIMIT {
        values.drain(..values.len() - SEEN_REALTIME_LIMIT);
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_realtime_page(after: i64, auction_id: Option<&str>) -> Result<RealtimePage, String> {
    if after < 0 {
        return Err("实时事件游标无效".to_owned());
    }
    let canonical_auction_id = auction_id.map(canonical_auction_id).transpose()?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = canonical_auction_id.as_deref() {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    query.append_pair("after", &after.to_string()).append_pair("page_size", "100");
    let url = format!("/api/v1/events?{}", query.finish());
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let page = if let Some(auction_id) = canonical_auction_id.as_deref() {
        let payload = super::decoder::decode_auction_realtime_page(response, auction_id).await?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    } else {
        let payload = super::decoder::decode_discovery_realtime_page(response).await?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    };
    validate_realtime_page_sequence(&page, after)?;
    Ok(page)
}

/// 订阅 Public 持久化实时流，并把服务端事件页交给页面替换当前安全快照。
///
/// 浏览器 WebSocket 自动携带同源 HttpOnly session Cookie；匿名客户端由服务端拒绝。
/// 连接建立后先走 HTTP catch-up，期间到达的单事件帧由 WebSocket 缓冲，避免补发与订阅间隙。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_realtime<F>(after: i64, on_page: F) -> Result<(), String>
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
) -> Result<(), String>
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
) -> Result<(), String>
where
    F: FnMut(RealtimePage) -> bool,
{
    if after < 0 {
        return Err("实时事件游标无效".to_owned());
    }
    let canonical_auction_id = auction_id.map(canonical_auction_id).transpose()?;
    let auction_id = canonical_auction_id.as_deref();
    let url = websocket_url(auction_id)?;
    let mut socket = WebSocket::open(&url).map_err(|error| error.to_string())?;
    poll_fn(|context| Sink::<Message>::poll_ready(Pin::new(&mut socket), context))
        .await
        .map_err(|error| error.to_string())?;
    if !matches!(socket.state(), WebSocketState::Open) {
        return Err("实时 WebSocket 建立失败".to_owned());
    }
    let (_write, mut read) = socket.split();
    let mut expected_after = after;
    catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
    while let Some(message) = read.next().await {
        let message = message.map_err(|error| error.to_string())?;
        let Message::Text(message) = message else {
            return Err("实时 WebSocket 返回了不支持的二进制消息".to_owned());
        };
        let event = super::decoder::decode_realtime_event(&message, auction_id, world)?;
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err("实时事件序列超过可表示范围".to_owned());
        };
        if event.stream_sequence != next_sequence {
            catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
        }
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err("实时事件序列超过可表示范围".to_owned());
        };
        if event.stream_sequence != next_sequence {
            return Err("实时事件序列出现断档".to_owned());
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
    Err("实时 WebSocket 已关闭".to_owned())
}

#[cfg(target_arch = "wasm32")]
async fn catch_up_realtime<F>(
    auction_id: Option<&str>,
    expected_after: &mut i64,
    on_page: &mut F,
) -> Result<(), String>
where
    F: FnMut(RealtimePage) -> bool,
{
    loop {
        let requested_after = *expected_after;
        let page = fetch_realtime_page(requested_after, auction_id).await?;
        let next_after = page.next_after;
        let has_more = page.has_more;
        let refetch_required = page.refetch_required;
        if has_more && next_after == requested_after {
            return Err("实时事件补发游标未前进".to_owned());
        }
        *expected_after = next_after;
        let continue_catch_up = on_page(page);
        if refetch_required || !continue_catch_up {
            return Ok(());
        }
        if !has_more {
            return Ok(());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_realtime<F>(_after: i64, _on_page: F) -> Result<(), String>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err("实时 WebSocket 仅在 wasm32 浏览器目标可用".to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_auction_realtime<F>(
    _after: i64,
    _auction_id: String,
    _world: String,
    _on_page: F,
) -> Result<(), String>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err("实时 WebSocket 仅在 wasm32 浏览器目标可用".to_owned())
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
