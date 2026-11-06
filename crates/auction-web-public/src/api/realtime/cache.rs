//! 浏览器侧有界实时防重与版本栅栏。
//! `RealtimeCache` 只消费已经通过 wire 解码与序列校验的事件页，不接触网络；
//! 它决定页面的资源失效范围，并在事实冲突、版本回退或序列缺口时要求全量重读。

use std::collections::{BTreeMap, HashSet, VecDeque};

use super::protocol::{RealtimeEvent, RealtimePage, RealtimePageMerge};

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
