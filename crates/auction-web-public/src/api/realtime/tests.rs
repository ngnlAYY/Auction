//! `RealtimeCache` 的防重、版本栅栏与事实基线回归测试。
//! 这些用例只覆盖纯数据合并语义，不建立 HTTP/WebSocket 连接。

use serde_json::{Value, json};

use super::{RealtimeCache, RealtimeEvent, RealtimePage};

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
    let merge = cache.merge_page(&page(vec![event(3, "stale", "auction.created", json!({}))], 5));

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
        cache.merge_page(&page(vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1)).bids_changed
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
        cache.merge_page(&page(vec![bid(1, "bid-event-1", "bid-1", 1, 1, 1_500)], 1)).bids_changed
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
