//! 收件列表窗口选择的纯函数：游标边界、最旧优先上限与恢复锚点。

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use auction_application::MailScanAnchor;

use super::super::super::client::{MoogleMailIncomingList, MoogleMailIncomingPrefix};
use super::*;

/// 与完整枚举共用的窗口选择：游标之前的行不进入窗口；超过上限时保留最旧窗口，
/// 让未处理的旧邮件在下一轮优先提交，而不是被新邮件挤出游标。
pub(super) fn incoming_window_rows(
    rows: &[MoogleMailSummary],
    cursor: Option<&str>,
    limit: usize,
) -> VecDeque<MoogleMailSummary> {
    let mut window = VecDeque::with_capacity(limit.min(rows.len()));
    for summary in rows {
        if cursor.is_some_and(|value| value == summary.external_mail_id) {
            break;
        }
        if window.len() == limit {
            window.pop_front();
        }
        window.push_back(summary.clone());
    }
    window
}

/// 恢复窗口使用的列表观测：命中已知最低待读 MID 的前缀，或完整枚举回退。
pub(super) enum RecoveryRows {
    Prefix(MoogleMailIncomingPrefix),
    Full(Arc<MoogleMailIncomingList>),
}

impl RecoveryRows {
    pub(super) fn summaries(&self) -> &[MoogleMailSummary] {
        match self {
            Self::Prefix(scan) => &scan.summaries,
            Self::Full(list) => &list.summaries,
        }
    }
}

/// 目标已知的最小 MID 是自然下界：读到它即可停止，更新邮件仍在本前缀中。
pub(super) fn pending_anchor(window: &VecDeque<MoogleMailSummary>) -> Option<MailScanAnchor> {
    window
        .iter()
        .filter_map(|summary| {
            auction_application::numeric_mail_scan_id(&summary.external_mail_id)
                .map(|value| (value, summary.external_mail_id.as_str()))
        })
        .min_by_key(|(value, _)| *value)
        .and_then(|(_, mail_id)| MailScanAnchor::from_cursor(mail_id))
}

/// 比较完整枚举与前缀的“新增”MID 集合；游标之前的历史行不参与。
pub(super) fn new_mail_ids(rows: &[MoogleMailSummary], cursor: Option<&str>) -> HashSet<String> {
    let anchor = cursor.and_then(auction_application::numeric_mail_scan_id);
    rows.iter()
        .filter(|summary| {
            anchor.is_none_or(|anchor| {
                auction_application::numeric_mail_scan_id(&summary.external_mail_id)
                    .is_some_and(|value| value > anchor)
            })
        })
        .map(|summary| summary.external_mail_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn summary(id: &str, minute: u32) -> MoogleMailSummary {
        MoogleMailSummary {
            external_mail_id: id.to_owned(),
            world: MoogleMailWorld::Main,
            sender: "sender".to_owned(),
            subject: format!("subject-{id}"),
            received_at: Utc.with_ymd_and_hms(2026, 11, 4, 12, minute, 0).unwrap(),
        }
    }

    fn rows() -> Vec<MoogleMailSummary> {
        vec![
            summary("105", 9),
            summary("104", 8),
            summary("103", 7),
            summary("100", 6),
            summary("99", 5),
        ]
    }

    #[test]
    fn window_stops_at_known_cursor_without_reading_older_rows() {
        let window = incoming_window_rows(&rows(), Some("103"), 10);
        assert_eq!(
            window.iter().map(|summary| summary.external_mail_id.as_str()).collect::<Vec<_>>(),
            vec!["105", "104"],
            "游标之前的行不进入窗口"
        );
    }

    #[test]
    fn window_limits_keep_oldest_backlog_for_next_round() {
        let window = incoming_window_rows(&rows(), Some("99"), 2);
        assert_eq!(
            window.iter().map(|summary| summary.external_mail_id.as_str()).collect::<Vec<_>>(),
            vec!["103", "100"],
            "超过上限时保留最旧窗口；未进入窗口的新邮件仍大于游标，下一轮重新可见"
        );
    }

    #[test]
    fn new_mail_ids_exclude_cursor_and_older_rows() {
        let ids = new_mail_ids(&rows(), Some("100"));
        assert_eq!(ids, HashSet::from(["105".to_owned(), "104".to_owned(), "103".to_owned()]));
        let all = new_mail_ids(&rows(), None);
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn prefix_scan_early_stops_on_cursor() {
        let mut scan =
            auction_application::MailPrefixScan::new(MailScanAnchor::from_cursor("103").unwrap());
        let mut seen = Vec::new();
        for summary in rows() {
            match scan.observe(&summary.external_mail_id, summary.received_at) {
                Ok(auction_application::MailPrefixPage::Continue) => seen.push(summary),
                Ok(auction_application::MailPrefixPage::Done) => {
                    assert!(scan.anchor_found());
                    break;
                }
                Err(reason) => panic!("前缀不应失败：{reason:?}"),
            }
        }
        assert_eq!(seen.len(), 2, "只读取锚点之前的 105 与 104");
        assert_eq!(seen[0].external_mail_id, "105");
    }

    #[test]
    fn pending_anchor_uses_lowest_known_target() {
        let window = VecDeque::from(vec![summary("105", 9), summary("100", 6), summary("103", 7)]);
        assert_eq!(pending_anchor(&window).unwrap().mail_id, "100");
        assert_eq!(pending_anchor(&VecDeque::new()), None);
        let invalid = VecDeque::from(vec![summary("not-a-mid", 1)]);
        assert_eq!(pending_anchor(&invalid), None, "无效 MID 不能成为锚点");
    }

    #[test]
    fn prefix_scan_falls_back_when_cursor_disappears() {
        let mut scan =
            auction_application::MailPrefixScan::new(MailScanAnchor::from_cursor("101").unwrap());
        assert_eq!(
            scan.observe("105", Utc.with_ymd_and_hms(2026, 11, 4, 12, 9, 0).unwrap()),
            Ok(auction_application::MailPrefixPage::Continue)
        );
        assert_eq!(
            scan.observe("100", Utc.with_ymd_and_hms(2026, 11, 4, 12, 6, 0).unwrap()),
            Err(auction_application::MailScanReason::AnchorMissing),
            "跨过锚点却未见到锚点行时必须回退完整扫描"
        );
    }
}
