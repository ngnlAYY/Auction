//! MoogleMail 列表解析的合成边界：收件人截断、7 位 hex 标题、分钟精度时间与坏页面失败关闭。

use std::collections::HashSet;

use chrono::{TimeZone, Utc};

use super::*;
use crate::clients::mooglemail::{MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld};

fn at(value: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
}

/// 生成符合冻结结构的列表页：唯一表头、按列数严格校验的数据行和一个分页器。
fn list_page(header: [&str; 4], rows: &str, next: Option<&str>) -> String {
    let header_cells = header.iter().map(|cell| format!("<th>{cell}</th>")).collect::<String>();
    let pager = match next {
        Some(href) => format!(
            r#"<div id="mmail_pager"><div>Page 1</div><div><a href="{href}">Next &gt;</a></div></div>"#
        ),
        None => r#"<div id="mmail_pager"><div>Page 1</div><div><span>Next &gt;</span></div></div>"#
            .to_owned(),
    };
    format!(
        r#"<html><body><table id="mmail_list"><tr>{header_cells}</tr>{rows}</table>{pager}</body></html>"#
    )
}

fn row(mid: &str, sender: &str, subject: &str, time: &str, state: &str, filter: &str) -> String {
    format!(
        r#"<tr onclick="location.href='/?s=Bazaar&amp;ss=mm&amp;filter={filter}&amp;mid={mid}'"><td>{sender}</td><td>{subject}</td><td>{time}</td><td>{state}</td></tr>"#
    )
}

fn parser() -> MoogleMailParser {
    MoogleMailParser
}

#[test]
fn inbox_keeps_minute_time_subject_hex_and_skips_duplicate_mids() {
    let first =
        row("3120869", "Sender One", "Re: One01 ab12cd3", "2026-11-02 01:14", "Unread", "inbox");
    let duplicate = first.clone();
    let second = row("3120870", "Sender Two", "Read Mail", "2026-11-02 02:00", "Read", "read");
    let document = list_page(
        ["From", "Subject", "Sent", "Read"],
        &format!("{first}{duplicate}{second}"),
        None,
    );

    let page = parser()
        .scan_inbox_page(document.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
        .expect("合法 inbox 结构必须解析");

    assert_eq!(page.summaries.len(), 2);
    assert_eq!(page.summaries[0].external_mail_id, "3120869");
    assert_eq!(page.summaries[0].sender, "Sender One");
    // 标题按原字节保留，7 位 hex 片段不参与任何解码。
    assert_eq!(page.summaries[0].subject, "Re: One01 ab12cd3");
    // 列表时间按 UTC 分钟精度展示，不能按本地时区换算。
    assert_eq!(page.summaries[0].received_at, at("2026-11-02T01:14:00Z"));
    assert_eq!(
        page.summaries[1].received_at,
        Utc.with_ymd_and_hms(2026, 11, 2, 2, 0, 0).single().unwrap()
    );
    assert!(!page.has_next_page);
}

#[test]
fn empty_inbox_is_a_valid_page_but_only_with_the_empty_marker_row() {
    let document = list_page(
        ["From", "Subject", "Sent", "Read"],
        r#"<tr><td id="mmail_nnm">No new mail</td></tr>"#,
        None,
    );
    let page = parser()
        .scan_inbox_page(document.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
        .expect("空 inbox 是合法列表页");
    assert!(page.summaries.is_empty());
    assert!(!page.has_next_page);
}

#[test]
fn inbox_pager_requires_the_exact_same_origin_filter_and_page() {
    let document = list_page(
        ["From", "Subject", "Sent", "Read"],
        &row("3120869", "Sender", "Subject", "2026-11-02 01:14", "Unread", "inbox"),
        Some("/?s=Bazaar&amp;ss=mm&amp;filter=inbox&amp;page=2"),
    );
    let page = parser()
        .scan_inbox_page(document.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
        .expect("规范分页链接必须被接受");
    assert!(page.has_next_page);

    let tampered = list_page(
        ["From", "Subject", "Sent", "Read"],
        &row("3120869", "Sender", "Subject", "2026-11-02 01:14", "Unread", "inbox"),
        Some("https://example.test/?s=Bazaar&amp;ss=mm&amp;filter=inbox&amp;page=2"),
    );
    assert_eq!(
        parser()
            .scan_inbox_page(tampered.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
            .unwrap_err()
            .category,
        MoogleMailErrorCategory::ProtocolDrift
    );
}

#[test]
fn inbox_rejects_seconds_timestamps_missing_headers_and_wrong_column_counts() {
    let seconds = list_page(
        ["From", "Subject", "Sent", "Read"],
        &row("3120869", "Sender", "Subject", "2026-11-02 01:14:23", "Unread", "inbox"),
        None,
    );
    assert!(
        parser()
            .scan_inbox_page(seconds.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
            .is_err(),
        "列表时间只接受分钟精度，秒字段必须失败关闭"
    );

    let missing_header = r#"<html><body><table id="mmail_list"><tr><td>From</td><td>Subject</td><td>Sent</td><td>Read</td></tr><tr onclick="location.href='/?mid=3120869'"><td>Sender</td><td>Subject</td><td>2026-11-02 01:14</td><td>Read</td></tr></table><div id="mmail_pager"><div>Page 1</div><div><span>Next &gt;</span></div></div></body></html>"#;
    assert!(
        parser()
            .scan_inbox_page(
                missing_header.as_bytes(),
                MoogleMailWorld::Main,
                1,
                &mut HashSet::new()
            )
            .is_err()
    );

    let three_cells = list_page(
        ["From", "Subject", "Sent", "Read"],
        r#"<tr onclick="location.href='/?mid=3120869'"><td>Sender</td><td>Subject</td><td>2026-11-02 01:14</td></tr>"#,
        None,
    );
    assert!(
        parser()
            .scan_inbox_page(three_cells.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
            .is_err()
    );

    let duplicate_mid = list_page(
        ["From", "Subject", "Sent", "Read"],
        &row("3120869", "Sender", "Subject", "2026-11-02 01:14", "Unread", "inbox")
            .replace("mid=3120869", "mid=3120869&amp;mid=3120870"),
        None,
    );
    assert!(
        parser()
            .scan_inbox_page(
                duplicate_mid.as_bytes(),
                MoogleMailWorld::Main,
                1,
                &mut HashSet::new()
            )
            .is_err()
    );

    assert!(parser().scan_inbox_page(b"", MoogleMailWorld::Main, 1, &mut HashSet::new()).is_err());
    assert!(
        parser()
            .scan_inbox_page(&[0xff, 0xfe, 0x00], MoogleMailWorld::Main, 1, &mut HashSet::new())
            .is_err()
    );
}

#[test]
fn sent_list_preserves_the_truncated_recipient_verbatim() {
    let document = list_page(
        ["To", "Subject", "Sent", "Read"],
        &row(
            "3125345",
            "Restia\u{fe52}Ashdol",
            "Settlement ab12cd3",
            "2026-11-02 01:14",
            "Read",
            "sent",
        ),
        None,
    );
    let page = parser()
        .scan_sent_page(document.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
        .expect("合法 Sent 列表必须解析");

    assert_eq!(page.summaries.len(), 1);
    // 列表可能截断到 15 个字符；解析器不得猜测完整身份，也不得把截断值当完整收件人。
    assert_eq!(page.summaries[0].recipient, "Restia\u{fe52}Ashdol");
    assert_eq!(page.summaries[0].subject, "Settlement ab12cd3");
    assert_eq!(page.summaries[0].sent_at, at("2026-11-02T01:14:00Z"));
    assert!(!page.has_next_page);
}

#[test]
fn sent_list_rejects_empty_recipient_and_overlong_subject() {
    let empty_recipient = list_page(
        ["To", "Subject", "Sent", "Read"],
        &row("3125345", "", "Settlement", "2026-11-02 01:14", "Read", "sent"),
        None,
    );
    assert!(
        parser()
            .scan_sent_page(
                empty_recipient.as_bytes(),
                MoogleMailWorld::Main,
                1,
                &mut HashSet::new()
            )
            .is_err()
    );

    let long_subject = "a".repeat(MAX_SUMMARY_SUBJECT_BYTES + 1);
    let overlong = list_page(
        ["To", "Subject", "Sent", "Read"],
        &row("3125345", "Recipient", &long_subject, "2026-11-02 01:14", "Read", "sent"),
        None,
    );
    assert!(
        parser()
            .scan_sent_page(overlong.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
            .is_err()
    );
}

#[test]
fn page_time_is_utc_minute_precision() {
    assert_eq!(
        parse_page_time("2026-11-02 01:14", MoogleMailOperation::Inbox, MoogleMailWorld::Main)
            .unwrap(),
        at("2026-11-02T01:14:00Z")
    );
    assert!(
        parse_page_time("2026-11-02 01:14:23", MoogleMailOperation::Inbox, MoogleMailWorld::Main)
            .is_err()
    );
    assert!(
        parse_page_time("2026-13-02 01:14", MoogleMailOperation::Sent, MoogleMailWorld::Isekai)
            .is_err()
    );
}

#[test]
fn mid_extraction_rejects_ambiguity_and_non_page_parameters() {
    assert_eq!(extract_mid("location.href='/?s=Bazaar&mid=3120869'").as_deref(), Some("3120869"));
    assert_eq!(extract_mid("mid=3120869").as_deref(), Some("3120869"));
    assert_eq!(extract_mid("/?mid=%33%31%32%30%38%36%39").as_deref(), Some("3120869"));
    // 多值或非法边界必须拒绝，不能选第一个猜测目标。
    assert_eq!(extract_mid("/?mid=1&mid=2"), None);
    assert_eq!(extract_mid("/?mid="), None);
    assert_eq!(extract_mid("xmid=3120869"), None);
    assert_eq!(extract_mid("/?mid=a%2"), None);
}

#[test]
fn list_subject_projection_only_accepts_provable_detail_prefixes() {
    assert!(subject_is_list_projection(
        "Re: [One01] Settlement",
        "Re: [One01] Settlement completed"
    ));
    assert!(subject_is_list_projection(
        "Re: [One01] Settle\u{fffd}",
        "Re: [One01] Settlement completed"
    ));
    assert!(subject_is_list_projection(
        "Re: [One01] Settle\u{2026}",
        "Re: [One01] Settlement completed"
    ));
    assert!(!subject_is_list_projection("Re: [One01] Other", "Re: [One01] Settlement completed"));
    assert!(!subject_is_list_projection("", "Re: [One01] Settlement"));
    assert!(!subject_is_list_projection("\u{2026}", "Re: [One01] Settlement"));
}

#[test]
fn sent_strict_page_rejects_duplicate_mid_while_legacy_skips() {
    let duplicate = list_page(
        ["To", "Subject", "Sent", "Read"],
        &format!(
            "{}{}",
            row("3125345", "Recipient", "Subject", "2026-11-02 01:14", "Never", "sent"),
            row("3125345", "Recipient", "Subject", "2026-11-02 01:14", "Never", "sent")
        ),
        None,
    );
    let legacy = parser()
        .scan_sent_page(duplicate.as_bytes(), MoogleMailWorld::Main, 1, &mut HashSet::new())
        .expect("旧解析仍跳过重复 MID");
    assert_eq!(legacy.summaries.len(), 1);
    assert!(
        parser()
            .scan_sent_page_strict(
                duplicate.as_bytes(),
                MoogleMailWorld::Main,
                1,
                &mut HashSet::new()
            )
            .is_err(),
        "严格维护扫描必须拒绝重复 MID"
    );
}
