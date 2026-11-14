//! 恢复后外部核对的规划与回执回归用例。

use super::*;
use auction_application::operations::{
    WorkerRecoveryInboundEntry, WorkerRecoverySentEntry, WorkerSentLookupCandidate,
};
use auction_application::recovery::{
    ExternalRecoveryDeliveryExpectation, ExternalRecoverySendEvidence,
};
use chrono::{TimeDelta, Utc};
use uuid::Uuid;

fn expectation(id: u128, subject: &str) -> ExternalRecoveryDeliveryExpectation {
    ExternalRecoveryDeliveryExpectation {
        delivery_id: Uuid::from_u128(id),
        fencing_version: 2,
        evidence: ExternalRecoverySendEvidence::Frozen,
        world_id: "main".to_owned(),
        recipient: "Restia﹒Ashdoll".to_owned(),
        subject: Some(subject.to_owned()),
        credits: 0,
        cod: 0,
        equipment_ids: Some(vec!["317632875".to_owned()]),
    }
}

/// 恢复库没有冻结尝试：只能按世界、收件人与资产核对，没有可证明的标题。
fn payload_expectation(id: u128) -> ExternalRecoveryDeliveryExpectation {
    ExternalRecoveryDeliveryExpectation {
        delivery_id: Uuid::from_u128(id),
        fencing_version: 2,
        evidence: ExternalRecoverySendEvidence::RestoredPayload,
        world_id: "main".to_owned(),
        recipient: "Restia﹒Ashdoll".to_owned(),
        subject: None,
        credits: 300_000,
        cod: 0,
        equipment_ids: Some(Vec::new()),
    }
}

fn entry(mid: &str, subject: &str) -> WorkerRecoverySentEntry {
    WorkerRecoverySentEntry {
        world: "main".to_owned(),
        external_mail_id: mid.to_owned(),
        recipient: "Restia".to_owned(),
        subject: subject.to_owned(),
        sent_at: Utc::now() - TimeDelta::minutes(10),
    }
}

fn detail(mid: &str, subject: &str, collected: bool) -> WorkerSentLookupCandidate {
    WorkerSentLookupCandidate {
        external_mail_id: mid.to_owned(),
        recipient: "Restia﹒Ashdoll".to_owned(),
        subject: subject.to_owned(),
        sent_at: Some(Utc::now() - TimeDelta::minutes(10)),
        remaining_credits: 0,
        cod_pending: 0,
        remaining_equipment_ids: if collected { Vec::new() } else { vec!["317632875".to_owned()] },
        collected,
        observed_at: Utc::now(),
    }
}

fn inbound(world: &str, mid: &str, received_at: DateTime<Utc>) -> WorkerRecoveryInboundEntry {
    WorkerRecoveryInboundEntry {
        world: world.to_owned(),
        external_mail_id: mid.to_owned(),
        sender: "Clo16".to_owned(),
        subject: "[Auction] submission".to_owned(),
        received_at,
    }
}

#[test]
fn apply_requires_complete_sent_scan_and_details() {
    assert!(apply_scan_blockers(true, true, &[], &[], false, false).is_empty());
    assert_eq!(
        apply_scan_blockers(false, true, &[], &[], false, false),
        vec!["scan_pause_resumed"]
    );
    assert_eq!(
        apply_scan_blockers(true, false, &[], &[], false, false),
        vec!["sent_scan_incomplete"]
    );
    assert_eq!(
        apply_scan_blockers(true, true, &["sent_timeout".to_owned()], &[], false, false),
        vec!["sent_scan_errors"]
    );
    assert_eq!(
        apply_scan_blockers(true, true, &[], &["detail_timeout".to_owned()], false, false),
        vec!["sent_detail_errors"]
    );
    assert_eq!(
        apply_scan_blockers(true, true, &[], &[], true, true),
        vec!["sent_detail_unavailable", "pending_claimed_truncated"]
    );
}

#[test]
fn sent_cutoff_keeps_overlapping_minute() {
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:30Z")
        .expect("cutoff")
        .with_timezone(&Utc);
    let mut sent = entry("100", "[Auction] settlement payment 11111111");
    sent.sent_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
        .expect("sent")
        .with_timezone(&Utc);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(&[expectation], cutoff);
    plan.scan_entries(&[sent]);
    assert_eq!(plan.detail_refs.len(), 1, "同一分钟的 Sent 不能被含秒 cutoff 丢弃");
    let mut detail = detail("100", "[Auction] settlement payment 11111111", false);
    detail.sent_at = Some(
        chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .expect("detail sent")
            .with_timezone(&Utc),
    );
    plan.apply_details(&[detail]);
    assert!(plan.quarantine_matches().is_empty(), "重叠分钟不能宣称精确匹配");
    assert_eq!(plan.unresolved_blocks().len(), 1, "重叠分钟必须进入未决阻塞");
}

#[test]
fn overlapping_unrelated_detail_does_not_block() {
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:30Z")
        .expect("cutoff")
        .with_timezone(&Utc);
    let mut sent = entry("100", "[Auction] settlement payment 11111111");
    sent.sent_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
        .expect("sent")
        .with_timezone(&Utc);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(&[expectation], cutoff);
    plan.scan_entries(&[sent]);
    let mut detail = detail("100", "[Auction] settlement payment 11111111", false);
    detail.recipient = "Other".to_owned();
    detail.sent_at = Some(
        chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .expect("detail sent")
            .with_timezone(&Utc),
    );
    plan.apply_details(&[detail]);
    assert!(plan.unresolved_blocks().is_empty(), "明确不匹配的消息不能阻塞投递");
    assert!(plan.quarantine_matches().is_empty());
}

#[test]
fn sent_cutoff_next_minute_is_exact() {
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:30Z")
        .expect("cutoff")
        .with_timezone(&Utc);
    let mut sent = entry("100", "[Auction] settlement payment 11111111");
    sent.sent_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:01:00Z")
        .expect("sent")
        .with_timezone(&Utc);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(&[expectation], cutoff);
    plan.scan_entries(&[sent]);
    let mut detail = detail("100", "[Auction] settlement payment 11111111", false);
    detail.sent_at = Some(
        chrono::DateTime::parse_from_rfc3339("2026-01-01T12:01:00Z")
            .expect("detail sent")
            .with_timezone(&Utc),
    );
    plan.apply_details(&[detail]);
    assert_eq!(plan.quarantine_matches().len(), 1);
    assert!(plan.unresolved_blocks().is_empty());
}

#[test]
fn plan_requires_unique_list_candidate_and_exact_detail() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    plan.scan_entries(&[entry("100", "[Auction] settlement payment 11111111")]);
    assert_eq!(plan.detail_refs.len(), 1);
    plan.apply_details(&[detail("100", expectation.subject.as_deref().unwrap_or_default(), false)]);
    assert_eq!(plan.exact.len(), 1);
    assert_eq!(plan.quarantine_matches()[0].external_mail_id, "100");
}

#[test]
fn duplicate_list_candidates_are_blocked_not_quarantined() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    plan.scan_entries(&[
        entry("100", "[Auction] settlement payment 11111111"),
        entry("101", "[Auction] settlement payment 11111111"),
    ]);
    assert!(plan.detail_refs.is_empty());
    assert_eq!(plan.ambiguous, 1);
    assert!(plan.quarantine_matches().is_empty());
    // 无法唯一归属 Sent 时不能留在 pending：必须显式阻塞。
    let blocks = plan.unresolved_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].delivery_id, Uuid::from_u128(7));
    assert_eq!(blocks[0].candidate_mail_ids, vec!["100".to_owned(), "101".to_owned()]);
}

#[test]
fn missing_detail_is_unconfirmed_not_exact() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let expectation = expectation(7, "[Auction] settlement payment 11111111");
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    plan.scan_entries(&[entry("100", "[Auction] settlement payment 11111111")]);
    plan.apply_details(&[]);
    assert_eq!(plan.unconfirmed, 1);
    assert!(plan.quarantine_matches().is_empty());
    let blocks = plan.unresolved_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
}

/// 恢复库没有冻结尝试时仍然必须进入核对；否则备份后已寄出的投递会再次领取。
#[test]
fn restored_payload_delivery_is_matched_without_frozen_attempt() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let expectation = payload_expectation(8);
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    assert_eq!(plan.restored_payload_count, 1);
    assert_eq!(plan.frozen_count, 0);
    plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
    assert_eq!(plan.detail_refs.len(), 1);
    let mut payout = detail("200", "[Auction] settlement seller payout", false);
    payout.remaining_credits = 300_000;
    payout.remaining_equipment_ids = Vec::new();
    plan.apply_details(&[payout]);
    assert_eq!(plan.exact.len(), 1);
    assert_eq!(plan.quarantine_matches()[0].expected_equipment_ids, Some(Vec::new()));
}

/// pending 默认 fence=0；隔离证据表要求 fence>=1，因此精确匹配也必须走未决阻塞。
#[test]
fn pending_fence_zero_exact_match_is_blocked_not_quarantined() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let mut expectation = payload_expectation(10);
    expectation.fencing_version = 0;
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
    let mut payout = detail("200", "[Auction] settlement seller payout", false);
    payout.remaining_credits = 300_000;
    payout.remaining_equipment_ids = Vec::new();
    plan.apply_details(&[payout]);
    assert!(plan.quarantine_matches().is_empty());
    let blocks = plan.unresolved_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
    assert_eq!(blocks[0].fencing_version, 0);
    assert_eq!(blocks[0].candidate_mail_ids, vec!["200".to_owned()]);
}

/// payload 重建没有标题；已领取详情无法归因，必须进入未决阻塞而不是留在 pending。
#[test]
fn collected_restored_payload_detail_is_blocked() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let expectation = payload_expectation(11);
    let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
    plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
    plan.apply_details(&[detail("200", "[Auction] settlement seller payout", true)]);
    assert!(plan.quarantine_matches().is_empty());
    let blocks = plan.unresolved_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
    assert_eq!(blocks[0].candidate_mail_ids, vec!["200".to_owned()]);
}

/// 同一 Sent MID 不能证明两条投递；两者都进入未决阻塞而不是隔离。
#[test]
fn shared_sent_mid_blocks_both_deliveries() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let first = payload_expectation(8);
    let second = payload_expectation(9);
    let mut plan = SentMatchPlan::new(&[first, second], cutoff);
    plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
    let mut payout = detail("200", "[Auction] settlement seller payout", false);
    payout.remaining_credits = 300_000;
    payout.remaining_equipment_ids = Vec::new();
    plan.apply_details(&[payout]);
    assert!(plan.quarantine_matches().is_empty());
    let mut blocked =
        plan.unresolved_blocks().into_iter().map(|block| block.delivery_id).collect::<Vec<_>>();
    blocked.sort_unstable();
    assert_eq!(blocked, vec![Uuid::from_u128(8), Uuid::from_u128(9)]);
}

/// Read Mail 没有持久读取时间：备份前到达、备份后才读取的邮件也必须报告缺失。
#[test]
fn read_mail_reconciliation_ignores_arrival_time() {
    let cutoff = Utc::now() - TimeDelta::hours(1);
    let entries = vec![
        inbound("main", "300", cutoff - TimeDelta::hours(5)),
        inbound("main", "301", cutoff + TimeDelta::minutes(1)),
        inbound("isekai", "302", cutoff - TimeDelta::minutes(1)),
    ];
    let known = [(String::from("main"), String::from("301"))].into_iter().collect::<BTreeSet<_>>();
    let missing = missing_inbound(&entries, &known);
    let mids = missing
        .iter()
        .map(|entry| entry["external_mail_id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(mids, vec!["300".to_owned(), "302".to_owned()]);
}

#[test]
fn apply_requires_confirmation_and_actor() {
    assert!(resolve_apply_authorization(false, None, None).unwrap().is_none());
    assert!(resolve_apply_authorization(true, None, Some("ops")).is_err());
    assert!(resolve_apply_authorization(true, Some("WRONG"), Some("ops")).is_err());
    assert!(resolve_apply_authorization(true, Some(APPLY_CONFIRMATION), None).is_err());
    assert_eq!(
        resolve_apply_authorization(true, Some(APPLY_CONFIRMATION), Some("ops"))
            .unwrap()
            .as_deref(),
        Some("ops")
    );
}
