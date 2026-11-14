use std::collections::HashSet;

use chrono::{DateTime, TimeZone, Utc};

use super::*;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 4, 12, minute, 0).unwrap()
}

fn watermark(id: &str, minute: u32) -> MailScanWatermark {
    MailScanWatermark { mail_id: id.to_owned(), observed_at: at(minute) }
}

fn checkpoint(id: &str, minute: u32, full_minute: u32) -> MailScanCheckpoint {
    MailScanCheckpoint {
        watermark: Some(watermark(id, minute)),
        last_full_scan_at: Some(at(full_minute)),
    }
}

#[test]
fn daily_full_verification_is_forced_by_cadence_or_missing_checkpoint() {
    let now = at(30);
    let daily = Duration::hours(24);
    assert_eq!(plan_mail_scan(None, now, daily).mode, MailScanMode::Full, "首次运行必须完整扫描");
    assert_eq!(plan_mail_scan(None, now, daily).reason, MailScanReason::NoCheckpoint);
    let fresh = checkpoint("100", 0, 0);
    let plan = plan_mail_scan(Some(&fresh), now, daily);
    assert_eq!(plan.mode, MailScanMode::Incremental, "当日内走前缀扫描");
    assert_eq!(plan.after.unwrap().mail_id, "100");
    let almost = MailScanCheckpoint {
        watermark: Some(watermark("100", 0)),
        last_full_scan_at: Some(now - Duration::hours(23) - Duration::minutes(59)),
    };
    assert_eq!(
        plan_mail_scan(Some(&almost), now, daily).mode,
        MailScanMode::Incremental,
        "未满每日间隔仍走前缀"
    );
    let stale = MailScanCheckpoint {
        watermark: Some(watermark("100", 0)),
        last_full_scan_at: Some(now - Duration::hours(24)),
    };
    let plan = plan_mail_scan(Some(&stale), now, daily);
    assert_eq!(plan.mode, MailScanMode::Full, "满 24 小时必须完整校验");
    assert_eq!(plan.reason, MailScanReason::DailyVerification);
    let future = MailScanCheckpoint {
        watermark: Some(watermark("100", 0)),
        last_full_scan_at: Some(now + Duration::hours(1)),
    };
    assert_eq!(
        plan_mail_scan(Some(&future), now, daily).reason,
        MailScanReason::DailyVerification,
        "时钟回拨不能推迟校验"
    );
    let empty = MailScanCheckpoint { watermark: None, last_full_scan_at: Some(now) };
    assert_eq!(plan_mail_scan(Some(&empty), now, daily).reason, MailScanReason::NoWatermark);
    let unverified =
        MailScanCheckpoint { watermark: Some(watermark("100", 0)), last_full_scan_at: None };
    assert_eq!(
        plan_mail_scan(Some(&unverified), now, daily).reason,
        MailScanReason::NoFullVerification
    );
    let invalid = MailScanCheckpoint {
        watermark: Some(MailScanWatermark { mail_id: "0100".to_owned(), observed_at: at(0) }),
        last_full_scan_at: Some(at(0)),
    };
    assert_eq!(
        plan_mail_scan(Some(&invalid), now, daily).reason,
        MailScanReason::InvalidCheckpoint
    );
}

#[test]
fn prefix_stops_at_anchor_and_keeps_early_stop_target_known() {
    let anchor = MailScanAnchor::from_cursor("100").unwrap();
    let mut scan = MailPrefixScan::new(anchor);
    assert_eq!(scan.observe("103", at(9)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("102", at(8)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("101", at(7)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("100", at(6)).unwrap(), MailPrefixPage::Done);
    assert!(scan.anchor_found());
    let outcome = scan.finish(false, false);
    assert_eq!(
        outcome,
        MailPrefixOutcome::Complete { newest: Some(watermark("103", 9)) },
        "最高 MID 是前缀首行，且遇到已知锚点即停"
    );
}

#[test]
fn prefix_page_after_anchor_stays_complete() {
    let mut scan = MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap());
    assert_eq!(scan.observe("102", at(9)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("100", at(8)).unwrap(), MailPrefixPage::Done);
    // 同一页锚点之后的更旧行仍须通过顺序与重复校验，但不改变结论。
    assert_eq!(scan.observe("99", at(7)).unwrap(), MailPrefixPage::Done);
    assert_eq!(
        scan.finish(false, false),
        MailPrefixOutcome::Complete { newest: Some(watermark("102", 9)) }
    );
}

#[test]
fn prefix_without_new_rows_keeps_previous_watermark() {
    let anchor = watermark("100", 6).anchor();
    let mut scan = MailPrefixScan::new(anchor);
    assert_eq!(scan.observe("100", at(6)).unwrap(), MailPrefixPage::Done);
    assert_eq!(scan.finish(false, false), MailPrefixOutcome::Complete { newest: None });
}

#[test]
fn prefix_falls_back_when_anchor_is_missing_before_crossing() {
    let mut scan = MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap());
    assert_eq!(scan.observe("105", at(9)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("99", at(8)), Err(MailScanReason::AnchorMissing));
    assert_eq!(
        MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap()).finish(false, true),
        MailPrefixOutcome::Fallback(MailScanReason::AnchorMissing)
    );
}

#[test]
fn prefix_falls_back_on_page_limit_and_duplicate_rows() {
    let mut scan = MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap());
    assert_eq!(scan.observe("110", at(9)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.finish(true, false), MailPrefixOutcome::Fallback(MailScanReason::PageLimit));
    let mut scan = MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap());
    assert_eq!(scan.observe("110", at(9)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("110", at(9)), Err(MailScanReason::PageDiscontinuity));
}

#[test]
fn prefix_rejects_time_reversal_and_anchor_changes() {
    let mut scan = MailPrefixScan::new(MailScanAnchor::from_cursor("100").unwrap());
    assert_eq!(scan.observe("102", at(8)).unwrap(), MailPrefixPage::Continue);
    assert_eq!(scan.observe("101", at(9)), Err(MailScanReason::OrderViolation));
    let mut scan = MailPrefixScan::new(watermark("100", 6).anchor());
    assert_eq!(scan.observe("100", at(5)), Err(MailScanReason::AnchorChanged));
    let mut scan =
        MailPrefixScan::new(MailScanAnchor { mail_id: "abc".to_owned(), observed_at: None });
    assert_eq!(scan.observe("101", at(9)), Err(MailScanReason::InvalidMid));
}

#[test]
fn watermark_never_regresses() {
    let previous = watermark("100", 6);
    assert_eq!(
        advance_mail_scan_watermark(Some(&previous), &watermark("99", 7)),
        previous,
        "较旧的观察不能覆盖已有水位"
    );
    assert_eq!(
        advance_mail_scan_watermark(Some(&previous), &watermark("101", 7)),
        watermark("101", 7)
    );
    assert_eq!(advance_mail_scan_watermark(None, &watermark("1", 1)), watermark("1", 1));
}

#[test]
fn shadow_comparison_reports_set_differences() {
    let legacy = HashSet::from(["1".to_owned(), "2".to_owned(), "3".to_owned()]);
    let same = HashSet::from(["3".to_owned(), "2".to_owned(), "1".to_owned()]);
    let shadow = compare_mail_scan_mids(&legacy, &same);
    assert!(shadow.is_match());
    assert_eq!(shadow.label(), "match");
    let candidate = HashSet::from(["2".to_owned(), "3".to_owned(), "4".to_owned()]);
    let shadow = compare_mail_scan_mids(&legacy, &candidate);
    assert!(!shadow.is_match());
    assert_eq!(shadow.legacy_only, 1);
    assert_eq!(shadow.candidate_only, 1);
    assert_eq!(shadow.label(), "mismatch");
}

#[test]
fn reason_stream_and_outcome_sets_are_closed() {
    assert_eq!(MailScanReason::ALL.len(), 15);
    assert_eq!(MailScanStream::ALL.len(), 3);
    assert_eq!(MailScanOutcome::ALL.len(), 4);
}
